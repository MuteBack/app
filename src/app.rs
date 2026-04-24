use std::time::Duration;

use crate::config::AppConfig;
use crate::ducking::{DuckError, Ducker};
use crate::session::{SessionAction, SessionController, SessionInput, SessionState};
use crate::vad::{VadDecision, VadEngine};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioTick {
    pub elapsed: Duration,
    pub hotkey_pressed: bool,
    pub explicit_stop: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppUpdate {
    pub vad: VadDecision,
    pub state: SessionState,
    pub action: Option<SessionAction>,
}

pub struct MuteBackApp<V, D> {
    controller: SessionController,
    vad: V,
    ducker: D,
}

impl<V, D> MuteBackApp<V, D>
where
    V: VadEngine,
    D: Ducker,
{
    pub fn new(config: AppConfig, vad: V, ducker: D) -> Self {
        Self {
            controller: SessionController::new(config),
            vad,
            ducker,
        }
    }

    pub fn process_audio_frame(
        &mut self,
        pcm_frame: &[i16],
        tick: AudioTick,
    ) -> Result<AppUpdate, DuckError> {
        let vad = self.vad.process_frame(pcm_frame);
        let update = self.controller.update(SessionInput {
            elapsed: tick.elapsed,
            vad,
            hotkey_pressed: tick.hotkey_pressed,
            explicit_stop: tick.explicit_stop,
        });

        match update.action {
            Some(SessionAction::Duck) => {
                let level = self.controller.config().normalized_ducking_level();
                self.ducker.duck(level)?;
            }
            Some(SessionAction::Restore) => {
                self.ducker.restore()?;
            }
            None => {}
        }

        Ok(AppUpdate {
            vad,
            state: update.state,
            action: update.action,
        })
    }

    pub fn force_restore(&mut self) -> Result<Option<SessionAction>, DuckError> {
        let update = self.controller.update(SessionInput {
            elapsed: Duration::ZERO,
            vad: VadDecision::Silence,
            hotkey_pressed: false,
            explicit_stop: true,
        });

        if let Some(SessionAction::Restore) = update.action {
            self.ducker.restore()?;
        }

        Ok(update.action)
    }

    pub fn set_config(&mut self, config: AppConfig) {
        self.controller.set_config(config);
    }

    pub fn refresh_audio_backend(&mut self) -> Result<(), DuckError> {
        self.ducker.refresh()
    }

    pub fn state(&self) -> SessionState {
        self.controller.state()
    }

    pub fn into_parts(self) -> (V, D) {
        (self.vad, self.ducker)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{AudioTick, MuteBackApp};
    use crate::config::AppConfig;
    use crate::ducking::{AppliedDucking, NoopDucker};
    use crate::vad::{VadDecision, VadEngine};

    struct SequenceVad {
        decisions: Vec<VadDecision>,
        cursor: usize,
    }

    impl SequenceVad {
        fn new(decisions: Vec<VadDecision>) -> Self {
            Self {
                decisions,
                cursor: 0,
            }
        }
    }

    impl VadEngine for SequenceVad {
        fn reset(&mut self) {
            self.cursor = 0;
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

    #[test]
    fn app_applies_duck_and_restore_to_backend() {
        let vad = SequenceVad::new(vec![
            VadDecision::Speech,
            VadDecision::Speech,
            VadDecision::Silence,
            VadDecision::Silence,
        ]);
        let ducker = NoopDucker::default();
        let mut app = MuteBackApp::new(AppConfig::default(), vad, ducker);

        let tick = AudioTick {
            elapsed: Duration::from_millis(150),
            hotkey_pressed: false,
            explicit_stop: false,
        };

        let _ = app.process_audio_frame(&[100; 320], tick).unwrap();
        let _ = app.process_audio_frame(&[100; 320], tick).unwrap();
        let _ = app
            .process_audio_frame(
                &[0; 320],
                AudioTick {
                    elapsed: Duration::from_millis(500),
                    ..tick
                },
            )
            .unwrap();
        let _ = app
            .process_audio_frame(
                &[0; 320],
                AudioTick {
                    elapsed: Duration::from_secs(5),
                    ..tick
                },
            )
            .unwrap();

        let (_, ducker) = app.into_parts();
        assert_eq!(ducker.current(), AppliedDucking::Restored);
    }
}
