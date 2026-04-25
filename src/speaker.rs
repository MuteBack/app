use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

use ndarray::Array3;
use ort::{inputs, session::Session, value::Tensor};

use crate::config::SpeakerProfile;
use crate::vad::{VadDecision, VadEngine};

const TARGET_SAMPLE_RATE: u32 = 16_000;
const NUM_MEL_BINS: usize = 80;
const FRAME_LENGTH_MS: u32 = 25;
const FRAME_SHIFT_MS: u32 = 10;
const N_FFT: usize = 512;
const PREEMPHASIS: f32 = 0.97;
const MODEL_ID: &str = "wespeaker-ecapa-tdnn512-lm";

#[derive(Debug, Clone)]
pub enum SpeakerError {
    ModelUnavailable(String),
    InvalidAudio(String),
    Inference(String),
}

impl Display for SpeakerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelUnavailable(message) => f.write_str(message),
            Self::InvalidAudio(message) => f.write_str(message),
            Self::Inference(message) => f.write_str(message),
        }
    }
}

impl Error for SpeakerError {}

pub trait SpeakerEmbeddingEngine {
    fn embed(&mut self, samples_16khz: &[i16]) -> Result<Vec<f32>, SpeakerError>;
}

pub struct OnnxSpeakerEmbeddingEngine {
    session: Session,
}

impl OnnxSpeakerEmbeddingEngine {
    pub fn new() -> Result<Self, SpeakerError> {
        Self::from_path(default_speaker_model_path())
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, SpeakerError> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(SpeakerError::ModelUnavailable(format!(
                "speaker embedding model not found at {}",
                path.display()
            )));
        }

        let session = Session::builder()
            .map_err(|error| {
                SpeakerError::Inference(format!("failed to create speaker ONNX session: {error}"))
            })?
            .with_intra_threads(1)
            .map_err(|error| {
                SpeakerError::Inference(format!(
                    "failed to configure speaker ONNX session: {error}"
                ))
            })?
            .commit_from_file(path)
            .map_err(|error| {
                SpeakerError::ModelUnavailable(format!(
                    "failed to load speaker model from {}: {error}",
                    path.display()
                ))
            })?;

        Ok(Self { session })
    }
}

impl SpeakerEmbeddingEngine for OnnxSpeakerEmbeddingEngine {
    fn embed(&mut self, samples_16khz: &[i16]) -> Result<Vec<f32>, SpeakerError> {
        let features = log_mel_features(samples_16khz)?;
        let feature_tensor = Tensor::from_array(features).map_err(|error| {
            SpeakerError::Inference(format!("failed to build feature tensor: {error}"))
        })?;
        let outputs = self
            .session
            .run(inputs!["feats" => feature_tensor])
            .map_err(|error| {
                SpeakerError::Inference(format!("speaker embedding inference failed: {error}"))
            })?;
        let embedding = outputs
            .get("embs")
            .and_then(|output| output.try_extract_tensor().ok())
            .map(|(_, data): (_, &[f32])| data.to_vec())
            .ok_or_else(|| {
                SpeakerError::Inference(
                    "speaker embedding output tensor 'embs' was not produced".to_string(),
                )
            })?;

        normalize_embedding(embedding)
    }
}

pub fn default_speaker_model_path() -> PathBuf {
    std::env::var_os("SPEAKER_MODEL_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join("vendor")
                .join("voxceleb_ECAPA512_LM.onnx")
        })
}

pub fn model_id() -> &'static str {
    MODEL_ID
}

pub fn resample_f32_to_i16(input: &[f32], source_rate: u32, target_rate: u32) -> Vec<i16> {
    if input.is_empty() || source_rate == 0 || target_rate == 0 {
        return Vec::new();
    }

    if source_rate == target_rate {
        return input.iter().map(|sample| f32_to_i16(*sample)).collect();
    }

    let step = source_rate as f64 / target_rate as f64;
    let output_len = ((input.len() as f64) / step).floor().max(0.0) as usize;
    let mut output = Vec::with_capacity(output_len);
    let mut position = 0.0_f64;

    while position + 1.0 < input.len() as f64 {
        let index = position.floor() as usize;
        let frac = (position - index as f64) as f32;
        let left = input[index];
        let right = input[index + 1];
        output.push(f32_to_i16(left + (right - left) * frac));
        position += step;
    }

    output
}

pub fn build_voice_profile(embeddings: &[Vec<f32>]) -> Result<SpeakerProfile, SpeakerError> {
    if embeddings.is_empty() {
        return Err(SpeakerError::InvalidAudio(
            "no speaker embeddings were produced".to_string(),
        ));
    }

    let dimension = embeddings[0].len();
    if dimension == 0
        || embeddings
            .iter()
            .any(|embedding| embedding.len() != dimension)
    {
        return Err(SpeakerError::InvalidAudio(
            "speaker embeddings have inconsistent dimensions".to_string(),
        ));
    }

    let mut averaged = vec![0.0; dimension];
    for embedding in embeddings {
        for (target, value) in averaged.iter_mut().zip(embedding) {
            *target += *value;
        }
    }
    for value in &mut averaged {
        *value /= embeddings.len() as f32;
    }

    let embedding = normalize_embedding(averaged)?;
    let threshold = threshold_from_embeddings(embeddings);

    Ok(SpeakerProfile {
        embedding,
        threshold,
        sample_rate: TARGET_SAMPLE_RATE,
        model_id: MODEL_ID.to_string(),
    })
}

pub fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.is_empty() || left.len() != right.len() {
        return None;
    }

    let mut dot = 0.0;
    let mut left_norm = 0.0;
    let mut right_norm = 0.0;

    for (left, right) in left.iter().zip(right) {
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }

    if left_norm <= f32::EPSILON || right_norm <= f32::EPSILON {
        return None;
    }

    Some(dot / (left_norm.sqrt() * right_norm.sqrt()))
}

fn threshold_from_embeddings(embeddings: &[Vec<f32>]) -> f32 {
    let mut similarities = Vec::new();

    for left in 0..embeddings.len() {
        for right in (left + 1)..embeddings.len() {
            if let Some(similarity) = cosine_similarity(&embeddings[left], &embeddings[right]) {
                similarities.push(similarity);
            }
        }
    }

    if similarities.is_empty() {
        return 0.50;
    }

    let mean = similarities.iter().sum::<f32>() / similarities.len() as f32;
    (mean - 0.08).clamp(0.42, 0.62)
}

fn normalize_embedding(mut embedding: Vec<f32>) -> Result<Vec<f32>, SpeakerError> {
    let norm = embedding
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    if norm <= f32::EPSILON {
        return Err(SpeakerError::InvalidAudio(
            "speaker embedding had zero magnitude".to_string(),
        ));
    }

    for value in &mut embedding {
        *value /= norm;
    }

    Ok(embedding)
}

fn log_mel_features(samples: &[i16]) -> Result<Array3<f32>, SpeakerError> {
    let frame_length = (TARGET_SAMPLE_RATE * FRAME_LENGTH_MS / 1000) as usize;
    let frame_shift = (TARGET_SAMPLE_RATE * FRAME_SHIFT_MS / 1000) as usize;

    if samples.len() < frame_length {
        return Err(SpeakerError::InvalidAudio(
            "voice sample is too short for speaker embedding".to_string(),
        ));
    }

    let frame_count = 1 + (samples.len() - frame_length) / frame_shift;
    let filters = mel_filterbank(NUM_MEL_BINS, N_FFT, TARGET_SAMPLE_RATE);
    let window = hamming_window(frame_length);
    let mut features = vec![0.0; frame_count * NUM_MEL_BINS];

    for frame_index in 0..frame_count {
        let start = frame_index * frame_shift;
        let mut frame = vec![0.0; N_FFT];

        for index in 0..frame_length {
            let current = samples[start + index] as f32 / 32768.0;
            let previous = if start + index == 0 {
                0.0
            } else {
                samples[start + index - 1] as f32 / 32768.0
            };
            frame[index] = (current - PREEMPHASIS * previous) * window[index];
        }

        let power = power_spectrum(&frame);

        for mel_index in 0..NUM_MEL_BINS {
            let energy = filters[mel_index]
                .iter()
                .zip(&power)
                .map(|(weight, power)| weight * power)
                .sum::<f32>()
                .max(1.0e-10);
            features[frame_index * NUM_MEL_BINS + mel_index] = energy.ln();
        }
    }

    apply_cepstral_mean_normalization(&mut features, frame_count, NUM_MEL_BINS);

    Array3::from_shape_vec((1, frame_count, NUM_MEL_BINS), features).map_err(|error| {
        SpeakerError::Inference(format!("failed to shape speaker feature tensor: {error}"))
    })
}

fn apply_cepstral_mean_normalization(features: &mut [f32], frames: usize, bins: usize) {
    for bin in 0..bins {
        let mut mean = 0.0;
        for frame in 0..frames {
            mean += features[frame * bins + bin];
        }
        mean /= frames as f32;

        for frame in 0..frames {
            features[frame * bins + bin] -= mean;
        }
    }
}

fn hamming_window(length: usize) -> Vec<f32> {
    if length <= 1 {
        return vec![1.0; length];
    }

    (0..length)
        .map(|index| {
            0.54 - 0.46 * ((2.0 * std::f32::consts::PI * index as f32) / (length - 1) as f32).cos()
        })
        .collect()
}

fn power_spectrum(frame: &[f32]) -> Vec<f32> {
    let bins = N_FFT / 2 + 1;
    let mut spectrum = Vec::with_capacity(bins);

    for bin in 0..bins {
        let mut real = 0.0;
        let mut imag = 0.0;

        for (index, sample) in frame.iter().enumerate() {
            let angle = 2.0 * std::f32::consts::PI * bin as f32 * index as f32 / N_FFT as f32;
            real += sample * angle.cos();
            imag -= sample * angle.sin();
        }

        spectrum.push((real * real + imag * imag) / N_FFT as f32);
    }

    spectrum
}

fn mel_filterbank(mel_bins: usize, n_fft: usize, sample_rate: u32) -> Vec<Vec<f32>> {
    let spectrum_bins = n_fft / 2 + 1;
    let min_mel = hz_to_mel(20.0);
    let max_mel = hz_to_mel(sample_rate as f32 / 2.0);
    let mel_points = (0..mel_bins + 2)
        .map(|index| {
            let fraction = index as f32 / (mel_bins + 1) as f32;
            mel_to_hz(min_mel + (max_mel - min_mel) * fraction)
        })
        .collect::<Vec<_>>();
    let bin_points = mel_points
        .iter()
        .map(|hz| (((n_fft + 1) as f32 * hz) / sample_rate as f32).floor() as usize)
        .collect::<Vec<_>>();

    let mut filters = vec![vec![0.0; spectrum_bins]; mel_bins];

    for mel_index in 0..mel_bins {
        let left = bin_points[mel_index].min(spectrum_bins - 1);
        let center = bin_points[mel_index + 1].min(spectrum_bins - 1);
        let right = bin_points[mel_index + 2].min(spectrum_bins - 1);

        if center > left {
            for bin in left..center {
                filters[mel_index][bin] = (bin - left) as f32 / (center - left) as f32;
            }
        }

        if right > center {
            for bin in center..right {
                filters[mel_index][bin] = (right - bin) as f32 / (right - center) as f32;
            }
        }
    }

    filters
}

fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10_f32.powf(mel / 2595.0) - 1.0)
}

fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

pub struct SpeakerVerifiedVad<V, E> {
    inner: V,
    engine: Option<E>,
    profile: Option<SpeakerProfile>,
    candidate: Vec<i16>,
    verified: bool,
    rejected: bool,
    silence_frames: u32,
    samples_until_next_check: usize,
}

impl<V, E> SpeakerVerifiedVad<V, E> {
    pub fn new(inner: V, engine: Option<E>, profile: Option<SpeakerProfile>) -> Self {
        Self {
            inner,
            engine,
            profile,
            candidate: Vec::new(),
            verified: false,
            rejected: false,
            silence_frames: 0,
            samples_until_next_check: TARGET_SAMPLE_RATE as usize,
        }
    }

    pub fn set_profile(&mut self, profile: Option<SpeakerProfile>) {
        self.profile = profile;
        self.reset_candidate();
    }

    pub fn into_inner(self) -> V {
        self.inner
    }

    fn enabled(&self) -> bool {
        self.engine.is_some() && self.profile.is_some()
    }

    fn reset_candidate(&mut self) {
        self.candidate.clear();
        self.verified = false;
        self.rejected = false;
        self.silence_frames = 0;
        self.samples_until_next_check = TARGET_SAMPLE_RATE as usize;
    }
}

impl<V, E> VadEngine for SpeakerVerifiedVad<V, E>
where
    V: VadEngine,
    E: SpeakerEmbeddingEngine,
{
    fn reset(&mut self) {
        self.inner.reset();
        self.reset_candidate();
    }

    fn process_frame(&mut self, frame: &[i16]) -> VadDecision {
        let decision = self.inner.process_frame(frame);

        if !self.enabled() {
            return decision;
        }

        if decision == VadDecision::Silence {
            self.silence_frames = self.silence_frames.saturating_add(1);
            if self.silence_frames >= 16 {
                self.reset_candidate();
            }
            return VadDecision::Silence;
        }

        self.silence_frames = 0;

        if self.verified {
            return decision;
        }

        if self.rejected {
            return VadDecision::Silence;
        }

        self.candidate.extend_from_slice(frame);
        let max_samples = (TARGET_SAMPLE_RATE as usize * 3).min(TARGET_SAMPLE_RATE as usize * 4);
        if self.candidate.len() > max_samples {
            let excess = self.candidate.len() - max_samples;
            self.candidate.drain(..excess);
        }

        let min_samples = TARGET_SAMPLE_RATE as usize;
        if self.candidate.len() < min_samples
            || self.candidate.len() < self.samples_until_next_check
        {
            return VadDecision::Silence;
        }

        self.samples_until_next_check = self.candidate.len() + TARGET_SAMPLE_RATE as usize / 4;
        let Some(profile) = self.profile.clone() else {
            return decision;
        };
        let Some(engine) = self.engine.as_mut() else {
            return decision;
        };

        match engine.embed(&self.candidate) {
            Ok(embedding) => {
                let similarity =
                    cosine_similarity(&embedding, &profile.embedding).unwrap_or(f32::NEG_INFINITY);
                if similarity >= profile.threshold {
                    self.verified = true;
                    VadDecision::Speech
                } else if self.candidate.len() >= max_samples {
                    self.rejected = true;
                    VadDecision::Silence
                } else {
                    VadDecision::Silence
                }
            }
            Err(_) => VadDecision::Silence,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_voice_profile, cosine_similarity, default_speaker_model_path, resample_f32_to_i16,
        OnnxSpeakerEmbeddingEngine, SpeakerEmbeddingEngine, SpeakerError, SpeakerProfile,
        SpeakerVerifiedVad,
    };
    use crate::vad::{VadDecision, VadEngine};

    struct ConstantVad(VadDecision);

    impl VadEngine for ConstantVad {
        fn reset(&mut self) {}

        fn process_frame(&mut self, _frame: &[i16]) -> VadDecision {
            self.0
        }
    }

    struct FakeSpeakerEngine {
        embedding: Vec<f32>,
    }

    impl SpeakerEmbeddingEngine for FakeSpeakerEngine {
        fn embed(&mut self, _samples_16khz: &[i16]) -> Result<Vec<f32>, SpeakerError> {
            Ok(self.embedding.clone())
        }
    }

    fn profile(embedding: Vec<f32>, threshold: f32) -> SpeakerProfile {
        SpeakerProfile {
            embedding,
            threshold,
            sample_rate: 16_000,
            model_id: "test".to_string(),
        }
    }

    #[test]
    fn cosine_similarity_scores_matching_vectors() {
        let score = cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]).unwrap();

        assert!((score - 1.0).abs() < 0.001);
    }

    #[test]
    fn voice_profile_averages_and_normalizes_embeddings() {
        let profile = build_voice_profile(&[vec![1.0, 0.0], vec![0.8, 0.2]]).unwrap();

        assert!(cosine_similarity(&profile.embedding, &[1.0, 0.0]).unwrap() > 0.95);
    }

    #[test]
    fn resampler_changes_sample_count() {
        let samples = vec![0.0; 48_000];
        let resampled = resample_f32_to_i16(&samples, 48_000, 16_000);

        assert!((15_990..=16_000).contains(&resampled.len()));
    }

    #[test]
    fn speaker_verified_vad_suppresses_until_voice_matches_profile() {
        let engine = FakeSpeakerEngine {
            embedding: vec![1.0, 0.0],
        };
        let mut vad = SpeakerVerifiedVad::new(
            ConstantVad(VadDecision::Speech),
            Some(engine),
            Some(profile(vec![1.0, 0.0], 0.9)),
        );
        let frame = vec![1_000; 512];

        assert_eq!(vad.process_frame(&frame), VadDecision::Silence);

        for _ in 0..31 {
            let decision = vad.process_frame(&frame);
            if decision == VadDecision::Speech {
                return;
            }
        }

        panic!("speaker verifier did not release a matching voice");
    }

    #[test]
    fn speaker_verified_vad_passes_through_without_profile() {
        let engine = FakeSpeakerEngine {
            embedding: vec![1.0, 0.0],
        };
        let mut vad = SpeakerVerifiedVad::new(ConstantVad(VadDecision::Speech), Some(engine), None);

        assert_eq!(vad.process_frame(&[1_000; 512]), VadDecision::Speech);
    }

    #[test]
    #[ignore]
    fn onnx_speaker_model_smoke_test() {
        if !default_speaker_model_path().exists() {
            return;
        }

        let mut engine = OnnxSpeakerEmbeddingEngine::new().unwrap();
        let samples = (0..32_000)
            .map(|index| {
                let phase = 2.0 * std::f32::consts::PI * 220.0 * index as f32 / 16_000.0;
                (phase.sin() * 6_000.0) as i16
            })
            .collect::<Vec<_>>();
        let embedding = engine.embed(&samples).unwrap();

        assert!(embedding.len() >= 128);
        assert!(embedding.iter().all(|value| value.is_finite()));
    }
}
