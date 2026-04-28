use std::time::Duration;
use std::{
    error::Error,
    fmt::{Display, Formatter},
};

#[cfg(feature = "engine")]
use crate::vad::{NearFieldGateConfig, ReferenceAudioConfig, SileroVadConfig};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub ducking_level: f32,
    pub voice_detection_sensitivity: f32,
    pub smooth_ducking: bool,
    pub manual_restore: bool,
    pub voice_match_enabled: bool,
    pub speaker_profile: Option<SpeakerProfile>,
    pub microphone_name: Option<String>,
    pub duck_fade: Duration,
    pub restore_fade: Duration,
    pub speech_start_confirmation: Duration,
    pub stop_candidate_window: Duration,
    pub hold_timeout: Duration,
    pub hotkey_release_grace: Duration,
}

#[derive(Debug, Clone)]
pub struct SpeakerProfile {
    pub embedding: Vec<f32>,
    pub threshold: f32,
    pub sample_rate: u32,
    pub model_id: String,
}

impl AppConfig {
    pub fn normalized_ducking_level(&self) -> f32 {
        self.ducking_level.clamp(0.0, 1.0)
    }

    pub fn normalized_voice_detection_sensitivity(&self) -> f32 {
        self.voice_detection_sensitivity.clamp(0.0, 1.0)
    }

    pub fn restore_delay(&self) -> Duration {
        self.stop_candidate_window
            .checked_add(self.hold_timeout)
            .unwrap_or(Duration::MAX)
    }

    pub fn set_restore_delay(&mut self, delay: Duration) {
        let stop_candidate_window = self.stop_candidate_window.min(delay);
        self.stop_candidate_window = stop_candidate_window;
        self.hold_timeout = delay.saturating_sub(stop_candidate_window);
    }

    #[cfg(feature = "engine")]
    pub fn silero_vad_config(&self) -> SileroVadConfig {
        let sensitivity = self.normalized_voice_detection_sensitivity();

        SileroVadConfig {
            start_probability: sensitivity_adjusted(0.72, 0.10, -0.16, sensitivity)
                .clamp(0.50, 0.90),
            continue_probability: sensitivity_adjusted(0.45, 0.08, -0.12, sensitivity)
                .clamp(0.25, 0.70),
        }
    }

    #[cfg(feature = "engine")]
    pub fn near_field_gate_config(&self) -> NearFieldGateConfig {
        let sensitivity = self.normalized_voice_detection_sensitivity();

        NearFieldGateConfig {
            minimum_noise_floor: sensitivity_adjusted(0.005, 0.001, -0.001, sensitivity)
                .clamp(0.003, 0.008),
            maximum_noise_floor: 0.022,
            start_multiplier: sensitivity_adjusted(2.1, 0.50, -0.85, sensitivity).clamp(1.25, 3.0),
            continue_multiplier: sensitivity_adjusted(1.35, 0.25, -0.35, sensitivity)
                .clamp(1.0, 2.0),
            minimum_start_rms: sensitivity_adjusted(0.026, 0.012, -0.020, sensitivity)
                .clamp(0.010, 0.050),
            minimum_continue_rms: sensitivity_adjusted(0.009, 0.004, -0.005, sensitivity)
                .clamp(0.004, 0.020),
            adaptation_rate: 0.035,
            blocked_speech_adaptation_rate: 0.045,
            calibration_time: Duration::from_millis(700),
        }
    }

    #[cfg(feature = "engine")]
    pub fn reference_audio_config(&self) -> ReferenceAudioConfig {
        let sensitivity = self.normalized_voice_detection_sensitivity();

        ReferenceAudioConfig {
            minimum_reference_rms: 0.012,
            stale_after: Duration::from_millis(250),
            initial_bleed_ratio: 0.30,
            bleed_margin_ratio: 0.06,
            minimum_bleed_margin_rms: 0.008,
            loud_near_field_rms: sensitivity_adjusted(0.090, 0.040, -0.100, sensitivity)
                .clamp(0.045, 0.140),
            max_learned_bleed_ratio: 0.70,
            leak_attack_rate: 0.18,
            leak_release_rate: 0.015,
        }
    }

    pub fn from_cli_args<I, S>(args: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let args = args
            .into_iter()
            .map(|arg| arg.as_ref().to_string())
            .collect::<Vec<_>>();
        let mut config = Self::default();
        let mut cursor = 0;

        while cursor < args.len() {
            match args[cursor].as_str() {
                "--duck-level" => {
                    let value = read_value(&args, &mut cursor, "--duck-level")?;
                    config.ducking_level = parse_percent("--duck-level", value)? / 100.0;
                }
                "--sensitivity" => {
                    let value = read_value(&args, &mut cursor, "--sensitivity")?;
                    config.voice_detection_sensitivity =
                        parse_percent("--sensitivity", value)? / 100.0;
                }
                "--transition" => {
                    let value = read_value(&args, &mut cursor, "--transition")?;
                    config.smooth_ducking = match value {
                        "smooth" => true,
                        "instant" => false,
                        _ => {
                            return Err(ConfigError::InvalidValue {
                                flag: "--transition",
                                value: value.to_string(),
                                expected: "smooth or instant",
                            });
                        }
                    };
                }
                "--duck-fade-ms" => {
                    let value = read_value(&args, &mut cursor, "--duck-fade-ms")?;
                    config.duck_fade =
                        Duration::from_millis(parse_millis("--duck-fade-ms", value)?);
                }
                "--restore-fade-ms" => {
                    let value = read_value(&args, &mut cursor, "--restore-fade-ms")?;
                    config.restore_fade =
                        Duration::from_millis(parse_millis("--restore-fade-ms", value)?);
                }
                "--restore-delay-ms" => {
                    let value = read_value(&args, &mut cursor, "--restore-delay-ms")?;
                    config.set_restore_delay(Duration::from_millis(parse_millis(
                        "--restore-delay-ms",
                        value,
                    )?));
                }
                "--restore-mode" => {
                    let value = read_value(&args, &mut cursor, "--restore-mode")?;
                    config.manual_restore = match value {
                        "automatic" => false,
                        "manual" => true,
                        _ => {
                            return Err(ConfigError::InvalidValue {
                                flag: "--restore-mode",
                                value: value.to_string(),
                                expected: "automatic or manual",
                            });
                        }
                    };
                }
                unknown => return Err(ConfigError::UnknownArgument(unknown.to_string())),
            }

            cursor += 1;
        }

        Ok(config)
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ducking_level: 0.10,
            voice_detection_sensitivity: 0.65,
            smooth_ducking: true,
            manual_restore: false,
            voice_match_enabled: false,
            speaker_profile: None,
            microphone_name: None,
            duck_fade: Duration::from_millis(180),
            restore_fade: Duration::from_millis(260),
            speech_start_confirmation: Duration::from_millis(220),
            stop_candidate_window: Duration::from_millis(320),
            hold_timeout: Duration::from_millis(1_800),
            hotkey_release_grace: Duration::from_millis(700),
        }
    }
}

#[cfg(feature = "engine")]
fn sensitivity_adjusted(
    base: f32,
    strict_delta: f32,
    sensitive_delta: f32,
    sensitivity: f32,
) -> f32 {
    let sensitivity = sensitivity.clamp(0.0, 1.0);

    if sensitivity >= 0.5 {
        base + ((sensitivity - 0.5) / 0.5) * sensitive_delta
    } else {
        base + ((0.5 - sensitivity) / 0.5) * strict_delta
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    MissingValue(&'static str),
    InvalidValue {
        flag: &'static str,
        value: String,
        expected: &'static str,
    },
    UnknownArgument(String),
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingValue(flag) => write!(f, "missing value for {flag}"),
            Self::InvalidValue {
                flag,
                value,
                expected,
            } => {
                write!(f, "invalid value for {flag}: {value}; expected {expected}")
            }
            Self::UnknownArgument(argument) => write!(f, "unknown argument: {argument}"),
        }
    }
}

impl Error for ConfigError {}

fn read_value<'a>(
    args: &'a [String],
    cursor: &mut usize,
    flag: &'static str,
) -> Result<&'a str, ConfigError> {
    *cursor += 1;
    args.get(*cursor)
        .map(String::as_str)
        .ok_or(ConfigError::MissingValue(flag))
}

fn parse_percent(flag: &'static str, value: &str) -> Result<f32, ConfigError> {
    let percent = value
        .parse::<f32>()
        .map_err(|_| ConfigError::InvalidValue {
            flag,
            value: value.to_string(),
            expected: "a number from 0 to 100",
        })?;

    if (0.0..=100.0).contains(&percent) {
        Ok(percent)
    } else {
        Err(ConfigError::InvalidValue {
            flag,
            value: value.to_string(),
            expected: "a number from 0 to 100",
        })
    }
}

fn parse_millis(flag: &'static str, value: &str) -> Result<u64, ConfigError> {
    value.parse::<u64>().map_err(|_| ConfigError::InvalidValue {
        flag,
        value: value.to_string(),
        expected: "a whole number of milliseconds",
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::AppConfig;

    #[test]
    fn defaults_to_smooth_ducking_at_ten_percent() {
        let config = AppConfig::default();

        assert_eq!(config.normalized_ducking_level(), 0.10);
        assert_eq!(config.normalized_voice_detection_sensitivity(), 0.65);
        assert!(config.smooth_ducking);
    }

    #[test]
    fn parses_ducking_settings_from_cli_args() {
        let config = AppConfig::from_cli_args([
            "--duck-level",
            "25",
            "--sensitivity",
            "72",
            "--transition",
            "instant",
            "--duck-fade-ms",
            "120",
            "--restore-fade-ms",
            "300",
            "--restore-delay-ms",
            "2500",
            "--restore-mode",
            "manual",
        ])
        .unwrap();

        assert_eq!(config.normalized_ducking_level(), 0.25);
        assert_eq!(config.normalized_voice_detection_sensitivity(), 0.72);
        assert!(!config.smooth_ducking);
        assert!(config.manual_restore);
        assert_eq!(config.duck_fade, Duration::from_millis(120));
        assert_eq!(config.restore_fade, Duration::from_millis(300));
        assert_eq!(config.restore_delay(), Duration::from_millis(2500));
        assert_eq!(config.stop_candidate_window, Duration::from_millis(320));
        assert_eq!(config.hold_timeout, Duration::from_millis(2180));
    }

    #[test]
    fn short_restore_delay_reduces_stop_confirmation_window() {
        let mut config = AppConfig::default();

        config.set_restore_delay(Duration::from_millis(120));

        assert_eq!(config.restore_delay(), Duration::from_millis(120));
        assert_eq!(config.stop_candidate_window, Duration::from_millis(120));
        assert_eq!(config.hold_timeout, Duration::ZERO);
    }

    #[cfg(feature = "engine")]
    #[test]
    fn sensitivity_tunes_detector_thresholds() {
        let strict = AppConfig {
            voice_detection_sensitivity: 0.20,
            ..AppConfig::default()
        };
        let sensitive = AppConfig {
            voice_detection_sensitivity: 0.90,
            ..AppConfig::default()
        };

        assert!(
            sensitive.silero_vad_config().start_probability
                < strict.silero_vad_config().start_probability
        );
        assert!(
            sensitive.near_field_gate_config().minimum_start_rms
                < strict.near_field_gate_config().minimum_start_rms
        );
        assert!(
            sensitive.reference_audio_config().loud_near_field_rms
                < strict.reference_audio_config().loud_near_field_rms
        );
    }
}
