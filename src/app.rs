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

    pub fn vad_mut(&mut self) -> &mut V {
        &mut self.vad
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
    use crate::session::SessionAction;
    use crate::vad::{
        NearFieldVad, ReferenceRejectingVad, SharedReferenceAudio, VadDecision, VadEngine,
    };

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

    #[derive(Debug)]
    struct DuckingQualityReport {
        duck_actions: u32,
        first_duck_after_background_start: Option<Duration>,
        first_duck_after_speech_start: Option<Duration>,
    }

    #[derive(Debug, Clone, Copy)]
    struct PlaybackScenario {
        frames: usize,
        reference_rms: f32,
        bleed_ratio: f32,
        speech_start_frame: Option<usize>,
        speech_rms: f32,
    }

    fn constant_frame(rms: f32) -> Vec<i16> {
        let sample = (rms.clamp(0.0, 1.0) * i16::MAX as f32) as i16;
        vec![sample; 512]
    }

    fn combined_rms(a: f32, b: f32) -> f32 {
        (a.mul_add(a, b * b)).sqrt().clamp(0.0, 1.0)
    }

    fn measure_playback_ducking_quality(scenario: PlaybackScenario) -> DuckingQualityReport {
        let frame_duration = Duration::from_millis(32);
        let calibration_frames = 24;
        let total_frames = calibration_frames + scenario.frames;
        let reference = SharedReferenceAudio::new();
        let inner = SequenceVad::new(vec![VadDecision::Speech; total_frames]);
        let near_field = NearFieldVad::new(inner, frame_duration);
        let vad = ReferenceRejectingVad::new(near_field, reference.clone());
        let mut app = MuteBackApp::new(AppConfig::default(), vad, NoopDucker::default());
        let tick = AudioTick {
            elapsed: frame_duration,
            hotkey_pressed: false,
            explicit_stop: false,
        };
        let silence = constant_frame(0.0);
        let mut duck_actions = 0;
        let mut first_duck_after_background_start = None;
        let mut first_duck_after_speech_start = None;

        for _ in 0..calibration_frames {
            let _ = app.process_audio_frame(&silence, tick).unwrap();
        }

        for frame_index in 0..scenario.frames {
            reference.update_rms(scenario.reference_rms);

            let bleed_rms = scenario.reference_rms * scenario.bleed_ratio;
            let speech_rms = scenario
                .speech_start_frame
                .filter(|speech_start| frame_index >= *speech_start)
                .map(|_| scenario.speech_rms)
                .unwrap_or(0.0);
            let mic_frame = constant_frame(combined_rms(bleed_rms, speech_rms));
            let update = app.process_audio_frame(&mic_frame, tick).unwrap();

            if update.action == Some(SessionAction::Duck) {
                duck_actions += 1;
                first_duck_after_background_start
                    .get_or_insert(frame_duration * frame_index as u32);

                if let Some(speech_start) = scenario.speech_start_frame {
                    if frame_index >= speech_start {
                        first_duck_after_speech_start
                            .get_or_insert(frame_duration * (frame_index - speech_start) as u32);
                    }
                }
            }
        }

        DuckingQualityReport {
            duck_actions,
            first_duck_after_background_start,
            first_duck_after_speech_start,
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

    #[test]
    fn quality_does_not_duck_on_music_playback_bleed() {
        let report = measure_playback_ducking_quality(PlaybackScenario {
            frames: 120,
            reference_rms: 0.22,
            bleed_ratio: 0.22,
            speech_start_frame: None,
            speech_rms: 0.0,
        });

        assert_eq!(report.duck_actions, 0, "{report:?}");
        assert_eq!(report.first_duck_after_background_start, None);
    }

    #[test]
    fn quality_does_not_duck_on_vlog_playback_bleed() {
        let report = measure_playback_ducking_quality(PlaybackScenario {
            frames: 120,
            reference_rms: 0.18,
            bleed_ratio: 0.30,
            speech_start_frame: None,
            speech_rms: 0.0,
        });

        assert_eq!(report.duck_actions, 0, "{report:?}");
        assert_eq!(report.first_duck_after_background_start, None);
    }

    #[test]
    fn quality_ducks_quickly_for_near_field_speech_over_music() {
        let report = measure_playback_ducking_quality(PlaybackScenario {
            frames: 120,
            reference_rms: 0.22,
            bleed_ratio: 0.22,
            speech_start_frame: Some(40),
            speech_rms: 0.090,
        });

        assert_eq!(report.duck_actions, 1, "{report:?}");
        assert!(
            report.first_duck_after_speech_start <= Some(Duration::from_millis(260)),
            "{report:?}"
        );
    }
}
