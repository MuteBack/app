use std::convert::TryFrom;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ndarray::{Array1, Array2, Array3};
use ort::{inputs, session::Session, value::Tensor};
use webrtc_vad::{SampleRate, Vad as WebRtcVad, VadMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadDecision {
    Silence,
    MaybeSpeech,
    Speech,
}

impl VadDecision {
    pub fn keeps_session_alive(self) -> bool {
        matches!(self, Self::MaybeSpeech | Self::Speech)
    }

    pub fn can_start_session(self) -> bool {
        matches!(self, Self::Speech)
    }
}

pub trait VadEngine {
    fn reset(&mut self);
    fn process_frame(&mut self, frame: &[i16]) -> VadDecision;
}

#[derive(Debug, Clone)]
pub struct EnergyGateConfig {
    pub minimum_noise_floor: f32,
    pub maybe_speech_multiplier: f32,
    pub speech_multiplier: f32,
    pub adaptation_rate: f32,
}

impl Default for EnergyGateConfig {
    fn default() -> Self {
        Self {
            minimum_noise_floor: 0.003,
            maybe_speech_multiplier: 1.8,
            speech_multiplier: 2.6,
            adaptation_rate: 0.08,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnergyGateVad {
    config: EnergyGateConfig,
    noise_floor: f32,
}

impl EnergyGateVad {
    pub fn new(config: EnergyGateConfig) -> Self {
        Self {
            noise_floor: config.minimum_noise_floor,
            config,
        }
    }

    pub fn noise_floor(&self) -> f32 {
        self.noise_floor
    }

    fn normalized_rms(frame: &[i16]) -> f32 {
        if frame.is_empty() {
            return 0.0;
        }

        let sum = frame
            .iter()
            .map(|sample| {
                let normalized = *sample as f32 / i16::MAX as f32;
                normalized * normalized
            })
            .sum::<f32>();

        (sum / frame.len() as f32).sqrt()
    }

    fn adapt_noise_floor(&mut self, rms: f32) {
        let target = rms.max(self.config.minimum_noise_floor);
        self.noise_floor += (target - self.noise_floor) * self.config.adaptation_rate;
        self.noise_floor = self.noise_floor.max(self.config.minimum_noise_floor);
    }
}

impl Default for EnergyGateVad {
    fn default() -> Self {
        Self::new(EnergyGateConfig::default())
    }
}

impl VadEngine for EnergyGateVad {
    fn reset(&mut self) {
        self.noise_floor = self.config.minimum_noise_floor;
    }

    fn process_frame(&mut self, frame: &[i16]) -> VadDecision {
        let rms = Self::normalized_rms(frame);

        // This is a bootstrap detector so we can wire the rest of the product
        // before bringing in a stronger backend such as WebRTC VAD.
        if rms <= self.noise_floor {
            self.adapt_noise_floor(rms);
            return VadDecision::Silence;
        }

        let maybe_threshold = self.noise_floor * self.config.maybe_speech_multiplier;
        let speech_threshold = self.noise_floor * self.config.speech_multiplier;

        if rms >= speech_threshold {
            VadDecision::Speech
        } else if rms >= maybe_threshold {
            VadDecision::MaybeSpeech
        } else {
            self.adapt_noise_floor(rms);
            VadDecision::Silence
        }
    }
}

#[derive(Debug, Clone)]
pub struct AutomaticVadConfig {
    pub energy_gate: EnergyGateConfig,
    pub attack_frames: u32,
    pub calibration_time: Duration,
}

impl Default for AutomaticVadConfig {
    fn default() -> Self {
        Self {
            energy_gate: EnergyGateConfig {
                minimum_noise_floor: 0.003,
                maybe_speech_multiplier: 2.0,
                speech_multiplier: 3.4,
                adaptation_rate: 0.03,
            },
            attack_frames: 3,
            calibration_time: Duration::from_millis(1_000),
        }
    }
}

pub struct AutomaticVad {
    energy_gate: EnergyGateVad,
    webrtc: Option<WebRtcVad>,
    attack_frames: u32,
    speech_run: u32,
    calibration_frames_left: u32,
}

impl AutomaticVad {
    pub fn new(sample_rate: u32, frame_duration: Duration) -> Self {
        Self::with_config(sample_rate, frame_duration, AutomaticVadConfig::default())
    }

    pub fn supports_webrtc(sample_rate: u32) -> bool {
        SampleRate::try_from(sample_rate as i32).is_ok()
    }

    pub fn with_config(
        sample_rate: u32,
        frame_duration: Duration,
        config: AutomaticVadConfig,
    ) -> Self {
        let webrtc = SampleRate::try_from(sample_rate as i32)
            .ok()
            .map(|rate| WebRtcVad::new_with_rate_and_mode(rate, VadMode::VeryAggressive));
        let frame_ms = frame_duration.as_millis().max(1) as u32;
        let calibration_ms = config.calibration_time.as_millis() as u32;
        let calibration_frames_left = calibration_ms.div_ceil(frame_ms);

        Self {
            energy_gate: EnergyGateVad::new(config.energy_gate),
            webrtc,
            attack_frames: config.attack_frames.max(1),
            speech_run: 0,
            calibration_frames_left,
        }
    }

    pub fn uses_webrtc(&self) -> bool {
        self.webrtc.is_some()
    }

    fn classify_frame(&mut self, frame: &[i16]) -> VadDecision {
        let energy = self.energy_gate.process_frame(frame);
        let webrtc = self
            .webrtc
            .as_mut()
            .and_then(|vad| vad.is_voice_segment(frame).ok());

        match webrtc {
            Some(true) => energy,
            Some(false) => VadDecision::Silence,
            None => energy,
        }
    }
}

impl VadEngine for AutomaticVad {
    fn reset(&mut self) {
        self.energy_gate.reset();
        self.speech_run = 0;

        if let Some(webrtc) = &mut self.webrtc {
            webrtc.reset();
        }
    }

    fn process_frame(&mut self, frame: &[i16]) -> VadDecision {
        if self.calibration_frames_left > 0 {
            self.calibration_frames_left -= 1;
            let _ = self.energy_gate.process_frame(frame);
            return VadDecision::Silence;
        }

        match self.classify_frame(frame) {
            VadDecision::Speech => {
                self.speech_run = self.speech_run.saturating_add(1);

                if self.speech_run >= self.attack_frames {
                    VadDecision::Speech
                } else {
                    VadDecision::MaybeSpeech
                }
            }
            VadDecision::MaybeSpeech => {
                self.speech_run = 0;
                VadDecision::MaybeSpeech
            }
            VadDecision::Silence => {
                self.speech_run = 0;
                VadDecision::Silence
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct SileroVadConfig {
    pub start_probability: f32,
    pub continue_probability: f32,
}

impl Default for SileroVadConfig {
    fn default() -> Self {
        Self {
            start_probability: 0.72,
            continue_probability: 0.45,
        }
    }
}

const SILERO_CONTEXT_SAMPLES: usize = 64;
const SILERO_STATE_DIM: usize = 128;

pub struct SileroVadEngine {
    session: Session,
    sample_rate: u32,
    chunk_size: usize,
    state: Array3<f32>,
    context: Vec<f32>,
    config: SileroVadConfig,
}

impl SileroVadEngine {
    pub fn new(sample_rate: u32) -> Result<Self, String> {
        Self::with_config(sample_rate, SileroVadConfig::default())
    }

    pub fn with_config(sample_rate: u32, config: SileroVadConfig) -> Result<Self, String> {
        let chunk_size = match sample_rate {
            8_000 => 256,
            16_000 => 512,
            other => {
                return Err(format!(
                    "Silero VAD supports only 8000 Hz or 16000 Hz, got {other}"
                ))
            }
        };
        let model_path = silero_model_path();
        let session = Session::builder()
            .map_err(|error| format!("failed to create ONNX session builder: {error}"))?
            .with_intra_threads(1)
            .map_err(|error| format!("failed to configure ONNX session threads: {error}"))?
            .commit_from_file(&model_path)
            .map_err(|error| {
                format!(
                    "failed to load Silero model from {}: {error}",
                    model_path.display()
                )
            })?;

        Ok(Self {
            session,
            sample_rate,
            chunk_size,
            state: Array3::<f32>::zeros((2, 1, SILERO_STATE_DIM)),
            context: vec![0.0; SILERO_CONTEXT_SAMPLES],
            config,
        })
    }
}

impl VadEngine for SileroVadEngine {
    fn reset(&mut self) {
        self.state.fill(0.0);
        self.context.fill(0.0);
    }

    fn process_frame(&mut self, frame: &[i16]) -> VadDecision {
        if frame.len() != self.chunk_size {
            return VadDecision::Silence;
        }

        let samples_f32: Vec<f32> = frame
            .iter()
            .map(|&sample| sample as f32 / 32768.0)
            .collect();

        // Silero expects the previous 64 samples plus the current 32 ms frame
        // so the LSTM keeps short-term context between calls.
        let mut input_data = Vec::with_capacity(SILERO_CONTEXT_SAMPLES + self.chunk_size);
        input_data.extend_from_slice(&self.context);
        input_data.extend_from_slice(&samples_f32);

        let input_array =
            match Array2::from_shape_vec((1, SILERO_CONTEXT_SAMPLES + self.chunk_size), input_data)
            {
                Ok(array) => array,
                Err(_) => return VadDecision::Silence,
            };
        let input_tensor = match Tensor::from_array(input_array) {
            Ok(tensor) => tensor,
            Err(_) => return VadDecision::Silence,
        };
        let state_tensor = match Tensor::from_array(self.state.clone()) {
            Ok(tensor) => tensor,
            Err(_) => return VadDecision::Silence,
        };
        let sr_tensor = match Tensor::from_array(Array1::from_vec(vec![self.sample_rate as i64])) {
            Ok(tensor) => tensor,
            Err(_) => return VadDecision::Silence,
        };

        let outputs = match self.session.run(inputs![
            "input" => input_tensor,
            "state" => state_tensor,
            "sr" => sr_tensor,
        ]) {
            Ok(outputs) => outputs,
            Err(_) => return VadDecision::Silence,
        };

        let probability = outputs
            .get("output")
            .and_then(|output| output.try_extract_tensor().ok())
            .and_then(|(_, data): (_, &[f32])| data.first().copied())
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);

        if let Some(new_state) = outputs.get("stateN") {
            if let Ok((_, new_state_data)) = new_state.try_extract_tensor::<f32>() {
                if new_state_data.len() == 2 * SILERO_STATE_DIM {
                    if let Some(state) = self.state.as_slice_mut() {
                        state.copy_from_slice(new_state_data);
                    }
                }
            }
        }

        let context_start = samples_f32.len().saturating_sub(SILERO_CONTEXT_SAMPLES);
        self.context.copy_from_slice(&samples_f32[context_start..]);

        if probability >= self.config.start_probability {
            VadDecision::Speech
        } else if probability >= self.config.continue_probability {
            VadDecision::MaybeSpeech
        } else {
            VadDecision::Silence
        }
    }
}

fn silero_model_path() -> PathBuf {
    std::env::var_os("SILERO_MODEL_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(default_silero_model_path)
}

fn default_silero_model_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("vendor")
        .join("silero_vad.onnx")
}
