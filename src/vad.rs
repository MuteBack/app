use std::convert::TryFrom;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;

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
        let rms = normalized_rms(frame);

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
pub struct NearFieldGateConfig {
    pub minimum_noise_floor: f32,
    pub maximum_noise_floor: f32,
    pub start_multiplier: f32,
    pub continue_multiplier: f32,
    pub minimum_start_rms: f32,
    pub minimum_continue_rms: f32,
    pub adaptation_rate: f32,
    pub blocked_speech_adaptation_rate: f32,
    pub calibration_time: Duration,
}

impl Default for NearFieldGateConfig {
    fn default() -> Self {
        Self {
            minimum_noise_floor: 0.005,
            maximum_noise_floor: 0.022,
            start_multiplier: 2.1,
            continue_multiplier: 1.35,
            minimum_start_rms: 0.026,
            minimum_continue_rms: 0.009,
            adaptation_rate: 0.035,
            blocked_speech_adaptation_rate: 0.045,
            calibration_time: Duration::from_millis(700),
        }
    }
}

pub struct NearFieldVad<V> {
    inner: V,
    config: NearFieldGateConfig,
    noise_floor: f32,
    calibration_frames: u32,
    calibration_frames_left: u32,
}

impl<V> NearFieldVad<V> {
    pub fn new(inner: V, frame_duration: Duration) -> Self {
        Self::with_config(inner, frame_duration, NearFieldGateConfig::default())
    }

    pub fn with_config(inner: V, frame_duration: Duration, config: NearFieldGateConfig) -> Self {
        let frame_ms = frame_duration.as_millis().max(1) as u32;
        let calibration_ms = config.calibration_time.as_millis() as u32;
        let calibration_frames = calibration_ms.div_ceil(frame_ms);

        Self {
            inner,
            noise_floor: config.minimum_noise_floor,
            config,
            calibration_frames,
            calibration_frames_left: calibration_frames,
        }
    }

    pub fn noise_floor(&self) -> f32 {
        self.noise_floor
    }

    pub fn into_inner(self) -> V {
        self.inner
    }

    fn adapt_noise_floor(&mut self, rms: f32, rate: f32) {
        let target = rms
            .clamp(
                self.config.minimum_noise_floor,
                self.config.maximum_noise_floor,
            )
            .max(self.config.minimum_noise_floor);
        self.noise_floor += (target - self.noise_floor) * rate.clamp(0.0, 1.0);
        self.noise_floor = self.noise_floor.clamp(
            self.config.minimum_noise_floor,
            self.config.maximum_noise_floor,
        );
    }

    fn passes_near_field_gate(&self, decision: VadDecision, rms: f32) -> bool {
        let threshold = match decision {
            VadDecision::Speech => {
                (self.noise_floor * self.config.start_multiplier).max(self.config.minimum_start_rms)
            }
            VadDecision::MaybeSpeech => (self.noise_floor * self.config.continue_multiplier)
                .max(self.config.minimum_continue_rms),
            VadDecision::Silence => return false,
        };

        rms >= threshold
    }
}

impl<V> VadEngine for NearFieldVad<V>
where
    V: VadEngine,
{
    fn reset(&mut self) {
        self.inner.reset();
        self.noise_floor = self.config.minimum_noise_floor;
        self.calibration_frames_left = self.calibration_frames;
    }

    fn process_frame(&mut self, frame: &[i16]) -> VadDecision {
        let rms = normalized_rms(frame);
        let decision = self.inner.process_frame(frame);

        if self.calibration_frames_left > 0 {
            self.calibration_frames_left -= 1;
            self.adapt_noise_floor(rms, self.config.blocked_speech_adaptation_rate);
            return VadDecision::Silence;
        }

        match decision {
            VadDecision::Silence => {
                self.adapt_noise_floor(rms, self.config.adaptation_rate);
                VadDecision::Silence
            }
            VadDecision::MaybeSpeech | VadDecision::Speech
                if self.passes_near_field_gate(decision, rms) =>
            {
                decision
            }
            VadDecision::MaybeSpeech | VadDecision::Speech => {
                self.adapt_noise_floor(rms, self.config.blocked_speech_adaptation_rate);
                VadDecision::Silence
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReferenceAudioConfig {
    pub minimum_reference_rms: f32,
    pub stale_after: Duration,
    pub initial_bleed_ratio: f32,
    pub bleed_margin_ratio: f32,
    pub minimum_bleed_margin_rms: f32,
    pub loud_near_field_rms: f32,
    pub max_learned_bleed_ratio: f32,
    pub leak_attack_rate: f32,
    pub leak_release_rate: f32,
}

impl Default for ReferenceAudioConfig {
    fn default() -> Self {
        Self {
            minimum_reference_rms: 0.012,
            stale_after: Duration::from_millis(250),
            initial_bleed_ratio: 0.30,
            bleed_margin_ratio: 0.06,
            minimum_bleed_margin_rms: 0.008,
            loud_near_field_rms: 0.090,
            max_learned_bleed_ratio: 0.70,
            leak_attack_rate: 0.18,
            leak_release_rate: 0.015,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReferenceAudioSnapshot {
    pub rms: f32,
    pub age: Duration,
    pub learned_bleed_ratio: f32,
}

impl ReferenceAudioSnapshot {
    pub fn is_active(&self, config: &ReferenceAudioConfig) -> bool {
        self.rms >= config.minimum_reference_rms && self.age <= config.stale_after
    }
}

#[derive(Debug)]
struct ReferenceAudioState {
    rms: f32,
    last_update: Option<Instant>,
    learned_bleed_ratio: f32,
}

#[derive(Debug, Clone)]
pub struct SharedReferenceAudio {
    state: Arc<Mutex<ReferenceAudioState>>,
}

impl SharedReferenceAudio {
    pub fn new() -> Self {
        Self::with_config(&ReferenceAudioConfig::default())
    }

    pub fn with_config(config: &ReferenceAudioConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(ReferenceAudioState {
                rms: 0.0,
                last_update: None,
                learned_bleed_ratio: config.initial_bleed_ratio,
            })),
        }
    }

    pub fn update_frame(&self, frame: &[i16]) {
        self.update_rms(normalized_rms(frame));
    }

    pub fn update_rms(&self, rms: f32) {
        if let Ok(mut state) = self.state.lock() {
            let rms = rms.clamp(0.0, 1.0);
            state.rms = state.rms.mul_add(0.75, rms * 0.25).max(rms * 0.70);
            state.last_update = Some(Instant::now());
        }
    }

    pub fn snapshot(&self) -> Option<ReferenceAudioSnapshot> {
        let state = self.state.lock().ok()?;
        let last_update = state.last_update?;

        Some(ReferenceAudioSnapshot {
            rms: state.rms,
            age: last_update.elapsed(),
            learned_bleed_ratio: state.learned_bleed_ratio,
        })
    }

    fn learn_bleed_ratio(&self, reference_rms: f32, mic_rms: f32, config: &ReferenceAudioConfig) {
        if reference_rms < config.minimum_reference_rms {
            return;
        }

        if let Ok(mut state) = self.state.lock() {
            let observed = (mic_rms / reference_rms).clamp(0.0, config.max_learned_bleed_ratio);
            let rate = if observed > state.learned_bleed_ratio {
                config.leak_attack_rate
            } else {
                config.leak_release_rate
            };
            state.learned_bleed_ratio += (observed - state.learned_bleed_ratio) * rate;
            state.learned_bleed_ratio = state
                .learned_bleed_ratio
                .clamp(config.initial_bleed_ratio, config.max_learned_bleed_ratio);
        }
    }
}

impl Default for SharedReferenceAudio {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ReferenceRejectingVad<V> {
    inner: V,
    reference: SharedReferenceAudio,
    config: ReferenceAudioConfig,
}

impl<V> ReferenceRejectingVad<V> {
    pub fn new(inner: V, reference: SharedReferenceAudio) -> Self {
        Self::with_config(inner, reference, ReferenceAudioConfig::default())
    }

    pub fn with_config(
        inner: V,
        reference: SharedReferenceAudio,
        config: ReferenceAudioConfig,
    ) -> Self {
        Self {
            inner,
            reference,
            config,
        }
    }

    pub fn into_inner(self) -> V {
        self.inner
    }

    fn should_reject(&self, mic_rms: f32, snapshot: &ReferenceAudioSnapshot) -> bool {
        if !snapshot.is_active(&self.config) || mic_rms >= self.config.loud_near_field_rms {
            return false;
        }

        let bleed_margin = (snapshot.rms * self.config.bleed_margin_ratio)
            .max(self.config.minimum_bleed_margin_rms);
        let learned_bleed_ceiling = snapshot.rms * snapshot.learned_bleed_ratio;

        mic_rms <= learned_bleed_ceiling + bleed_margin
    }
}

impl<V> VadEngine for ReferenceRejectingVad<V>
where
    V: VadEngine,
{
    fn reset(&mut self) {
        self.inner.reset();
    }

    fn process_frame(&mut self, frame: &[i16]) -> VadDecision {
        let decision = self.inner.process_frame(frame);
        let Some(snapshot) = self.reference.snapshot() else {
            return decision;
        };

        if !snapshot.is_active(&self.config) {
            return decision;
        }

        let mic_rms = normalized_rms(frame);

        if decision == VadDecision::Silence {
            self.reference
                .learn_bleed_ratio(snapshot.rms, mic_rms, &self.config);
            return decision;
        }

        if self.should_reject(mic_rms, &snapshot) {
            self.reference
                .learn_bleed_ratio(snapshot.rms, mic_rms, &self.config);
            VadDecision::Silence
        } else {
            decision
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        NearFieldGateConfig, NearFieldVad, ReferenceAudioConfig, ReferenceRejectingVad,
        SharedReferenceAudio, VadDecision, VadEngine,
    };

    struct SequenceVad {
        decisions: Vec<VadDecision>,
        cursor: usize,
        resets: u32,
    }

    impl SequenceVad {
        fn new(decisions: Vec<VadDecision>) -> Self {
            Self {
                decisions,
                cursor: 0,
                resets: 0,
            }
        }
    }

    impl VadEngine for SequenceVad {
        fn reset(&mut self) {
            self.cursor = 0;
            self.resets += 1;
        }

        fn process_frame(&mut self, _frame: &[i16]) -> VadDecision {
            let decision = self
                .decisions
                .get(self.cursor)
                .copied()
                .unwrap_or(VadDecision::Silence);
            self.cursor += 1;
            decision
        }
    }

    fn gate_config() -> NearFieldGateConfig {
        NearFieldGateConfig {
            minimum_noise_floor: 0.006,
            maximum_noise_floor: 0.022,
            start_multiplier: 3.0,
            continue_multiplier: 1.5,
            minimum_start_rms: 0.020,
            minimum_continue_rms: 0.010,
            adaptation_rate: 0.10,
            blocked_speech_adaptation_rate: 0.50,
            calibration_time: Duration::ZERO,
        }
    }

    #[test]
    fn near_field_gate_blocks_low_energy_speech_like_audio() {
        let inner = SequenceVad::new(vec![VadDecision::Speech]);
        let mut vad = NearFieldVad::with_config(inner, Duration::from_millis(32), gate_config());

        let decision = vad.process_frame(&[300; 512]);

        assert_eq!(decision, VadDecision::Silence);
        assert!(vad.noise_floor() > 0.006);
    }

    #[test]
    fn near_field_gate_allows_loud_confirmed_speech() {
        let inner = SequenceVad::new(vec![VadDecision::Speech]);
        let mut vad = NearFieldVad::with_config(inner, Duration::from_millis(32), gate_config());

        let decision = vad.process_frame(&[1_600; 512]);

        assert_eq!(decision, VadDecision::Speech);
    }

    #[test]
    fn near_field_gate_calibrates_before_reporting_speech() {
        let mut config = gate_config();
        config.calibration_time = Duration::from_millis(64);
        let inner = SequenceVad::new(vec![
            VadDecision::Speech,
            VadDecision::Speech,
            VadDecision::Speech,
        ]);
        let mut vad = NearFieldVad::with_config(inner, Duration::from_millis(32), config);

        assert_eq!(vad.process_frame(&[1_600; 512]), VadDecision::Silence);
        assert_eq!(vad.process_frame(&[1_600; 512]), VadDecision::Silence);
        assert_eq!(vad.process_frame(&[6_000; 512]), VadDecision::Speech);
    }

    #[test]
    fn near_field_gate_resets_inner_vad() {
        let inner = SequenceVad::new(vec![VadDecision::Speech]);
        let mut vad = NearFieldVad::with_config(inner, Duration::from_millis(32), gate_config());

        vad.reset();

        assert_eq!(vad.into_inner().resets, 1);
    }

    #[test]
    fn near_field_gate_caps_playback_bleed_noise_floor() {
        let mut config = gate_config();
        config.start_multiplier = 2.1;
        config.minimum_start_rms = 0.016;
        let inner =
            SequenceVad::new([vec![VadDecision::Silence; 24], vec![VadDecision::Speech]].concat());
        let mut vad = NearFieldVad::with_config(inner, Duration::from_millis(32), config);

        for _ in 0..24 {
            assert_eq!(vad.process_frame(&[2_200; 512]), VadDecision::Silence);
        }

        assert!(vad.noise_floor() <= 0.022);
        assert_eq!(vad.process_frame(&[1_800; 512]), VadDecision::Speech);
    }

    fn reference_config() -> ReferenceAudioConfig {
        ReferenceAudioConfig {
            minimum_reference_rms: 0.010,
            stale_after: Duration::from_secs(5),
            initial_bleed_ratio: 0.10,
            bleed_margin_ratio: 0.05,
            minimum_bleed_margin_rms: 0.004,
            loud_near_field_rms: 0.090,
            max_learned_bleed_ratio: 0.50,
            leak_attack_rate: 0.20,
            leak_release_rate: 0.02,
        }
    }

    #[test]
    fn reference_rejecting_vad_blocks_speech_that_matches_playback_bleed() {
        let config = reference_config();
        let reference = SharedReferenceAudio::with_config(&config);
        reference.update_rms(0.30);
        let inner = SequenceVad::new(vec![VadDecision::Speech]);
        let mut vad = ReferenceRejectingVad::with_config(inner, reference, config);

        let decision = vad.process_frame(&[600; 512]);

        assert_eq!(decision, VadDecision::Silence);
    }

    #[test]
    fn reference_rejecting_vad_allows_loud_near_field_speech_over_playback() {
        let config = reference_config();
        let reference = SharedReferenceAudio::with_config(&config);
        reference.update_rms(0.30);
        let inner = SequenceVad::new(vec![VadDecision::Speech]);
        let mut vad = ReferenceRejectingVad::with_config(inner, reference, config);

        let decision = vad.process_frame(&[4_000; 512]);

        assert_eq!(decision, VadDecision::Speech);
    }

    #[test]
    fn reference_rejecting_vad_allows_speech_without_active_playback_reference() {
        let config = reference_config();
        let reference = SharedReferenceAudio::with_config(&config);
        let inner = SequenceVad::new(vec![VadDecision::Speech]);
        let mut vad = ReferenceRejectingVad::with_config(inner, reference, config);

        let decision = vad.process_frame(&[600; 512]);

        assert_eq!(decision, VadDecision::Speech);
    }
}
