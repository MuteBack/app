use std::error::Error;
use std::fmt::{Display, Formatter};

pub trait Ducker {
    fn duck(&mut self, level: f32) -> Result<(), DuckError>;
    fn restore(&mut self) -> Result<(), DuckError>;
    fn refresh(&mut self) -> Result<(), DuckError>;
}

#[derive(Debug, Clone)]
pub enum DuckError {
    InvalidLevel(f32),
    BackendUnavailable(&'static str),
    Message(String),
}

impl Display for DuckError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLevel(level) => {
                write!(f, "ducking level must be between 0.0 and 1.0, got {level}")
            }
            Self::BackendUnavailable(message) => write!(f, "{message}"),
            Self::Message(message) => f.write_str(message),
        }
    }
}

impl Error for DuckError {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AppliedDucking {
    Restored,
    Ducked(f32),
}

#[derive(Debug, Default)]
pub struct NoopDucker {
    current: Option<f32>,
}

impl NoopDucker {
    pub fn current(&self) -> AppliedDucking {
        match self.current {
            Some(level) => AppliedDucking::Ducked(level),
            None => AppliedDucking::Restored,
        }
    }
}

impl Ducker for NoopDucker {
    fn duck(&mut self, level: f32) -> Result<(), DuckError> {
        if !(0.0..=1.0).contains(&level) {
            return Err(DuckError::InvalidLevel(level));
        }

        self.current = Some(level);
        Ok(())
    }

    fn restore(&mut self) -> Result<(), DuckError> {
        self.current = None;
        Ok(())
    }

    fn refresh(&mut self) -> Result<(), DuckError> {
        Ok(())
    }
}
