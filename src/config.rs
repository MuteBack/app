use std::time::Duration;
use std::{
    error::Error,
    fmt::{Display, Formatter},
};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub ducking_level: f32,
    pub smooth_ducking: bool,
    pub manual_restore: bool,
    pub voice_match_enabled: bool,
    pub speaker_profile: Option<SpeakerProfile>,
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
            smooth_ducking: true,
            manual_restore: false,
            voice_match_enabled: false,
            speaker_profile: None,
            duck_fade: Duration::from_millis(180),
            restore_fade: Duration::from_millis(260),
            speech_start_confirmation: Duration::from_millis(220),
            stop_candidate_window: Duration::from_millis(320),
            hold_timeout: Duration::from_millis(1_800),
            hotkey_release_grace: Duration::from_millis(700),
        }
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
        assert!(config.smooth_ducking);
    }

    #[test]
    fn parses_ducking_settings_from_cli_args() {
        let config = AppConfig::from_cli_args([
            "--duck-level",
            "25",
            "--transition",
            "instant",
            "--duck-fade-ms",
            "120",
            "--restore-fade-ms",
            "300",
            "--restore-mode",
            "manual",
        ])
        .unwrap();

        assert_eq!(config.normalized_ducking_level(), 0.25);
        assert!(!config.smooth_ducking);
        assert!(config.manual_restore);
        assert_eq!(config.duck_fade, Duration::from_millis(120));
        assert_eq!(config.restore_fade, Duration::from_millis(300));
    }
}
