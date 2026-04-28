use std::time::Duration;

use crate::config::AppConfig;
use crate::vad::VadDecision;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Talking,
    Hold,
    AwaitingManualRestore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAction {
    Duck,
    Restore,
}

#[derive(Debug, Clone, Copy)]
pub struct SessionInput {
    pub elapsed: Duration,
    pub vad: VadDecision,
    pub hotkey_pressed: bool,
    pub explicit_stop: bool,
    pub output_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionUpdate {
    pub state: SessionState,
    pub action: Option<SessionAction>,
}

pub struct SessionController {
    config: AppConfig,
    state: SessionState,
    speech_accumulator: Duration,
    silence_accumulator: Duration,
    hold_accumulator: Duration,
    active_hold_timeout: Duration,
    ducked: bool,
    previous_hotkey_pressed: bool,
}

impl SessionController {
    pub fn new(config: AppConfig) -> Self {
        Self {
            active_hold_timeout: config.hold_timeout,
            config,
            state: SessionState::Idle,
            speech_accumulator: Duration::ZERO,
            silence_accumulator: Duration::ZERO,
            hold_accumulator: Duration::ZERO,
            ducked: false,
            previous_hotkey_pressed: false,
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn set_config(&mut self, config: AppConfig) {
        let previous_hold_timeout = self.config.hold_timeout;
        self.config = config;

        if self.state == SessionState::Hold && self.active_hold_timeout == previous_hold_timeout {
            self.active_hold_timeout = self.config.hold_timeout;
        }
    }

    pub fn update(&mut self, input: SessionInput) -> SessionUpdate {
        if input.explicit_stop {
            return self.force_idle();
        }

        if input.hotkey_pressed {
            self.previous_hotkey_pressed = true;
            self.speech_accumulator = Duration::ZERO;
            self.silence_accumulator = Duration::ZERO;
            self.hold_accumulator = Duration::ZERO;
            self.state = SessionState::Talking;
            return self.duck_if_needed(input.output_active);
        }

        if self.previous_hotkey_pressed {
            self.previous_hotkey_pressed = false;

            if self.state == SessionState::Talking {
                // Hotkey release is a stronger "done for now" signal than plain silence,
                // so we keep a shorter grace window before restoring audio.
                self.enter_hold(self.config.hotkey_release_grace);
                return self.snapshot(None);
            }
        }

        let update = match self.state {
            SessionState::Idle => self.update_idle(input),
            SessionState::Talking => self.update_talking(input),
            SessionState::Hold => self.update_hold(input),
            SessionState::AwaitingManualRestore => self.update_manual_restore(input),
        };

        if update.action.is_none()
            && input.output_active
            && !self.ducked
            && matches!(self.state, SessionState::Talking | SessionState::Hold)
        {
            return self.duck_if_needed(input.output_active);
        }

        update
    }

    fn update_idle(&mut self, input: SessionInput) -> SessionUpdate {
        if input.vad.can_start_session() {
            self.speech_accumulator += input.elapsed;

            if self.speech_accumulator >= self.config.speech_start_confirmation {
                self.state = SessionState::Talking;
                self.silence_accumulator = Duration::ZERO;
                self.hold_accumulator = Duration::ZERO;
                return self.duck_if_needed(input.output_active);
            }
        } else {
            self.speech_accumulator = Duration::ZERO;
        }

        self.snapshot(None)
    }

    fn update_talking(&mut self, input: SessionInput) -> SessionUpdate {
        if input.vad.keeps_session_alive() {
            self.silence_accumulator = Duration::ZERO;
            return self.snapshot(None);
        }

        self.silence_accumulator += input.elapsed;

        if self.silence_accumulator >= self.config.stop_candidate_window {
            self.enter_hold(self.config.hold_timeout);
        }

        self.snapshot(None)
    }

    fn update_hold(&mut self, input: SessionInput) -> SessionUpdate {
        if input.vad.keeps_session_alive() {
            self.state = SessionState::Talking;
            self.speech_accumulator = Duration::ZERO;
            self.silence_accumulator = Duration::ZERO;
            self.hold_accumulator = Duration::ZERO;
            return self.snapshot(None);
        }

        self.hold_accumulator += input.elapsed;

        if self.hold_accumulator >= self.active_hold_timeout {
            if self.config.manual_restore && self.ducked {
                self.state = SessionState::AwaitingManualRestore;
                return self.snapshot(None);
            }

            return self.restore_if_needed();
        }

        self.snapshot(None)
    }

    fn update_manual_restore(&mut self, input: SessionInput) -> SessionUpdate {
        if input.vad.keeps_session_alive() {
            self.state = SessionState::Talking;
            self.speech_accumulator = Duration::ZERO;
            self.silence_accumulator = Duration::ZERO;
            self.hold_accumulator = Duration::ZERO;
        }

        self.snapshot(None)
    }

    fn enter_hold(&mut self, timeout: Duration) {
        self.state = SessionState::Hold;
        self.active_hold_timeout = timeout;
        self.speech_accumulator = Duration::ZERO;
        self.silence_accumulator = Duration::ZERO;
        self.hold_accumulator = Duration::ZERO;
    }

    fn duck_if_needed(&mut self, output_active: bool) -> SessionUpdate {
        let action = if self.ducked || !output_active {
            None
        } else {
            self.ducked = true;
            Some(SessionAction::Duck)
        };

        self.snapshot(action)
    }

    fn restore_if_needed(&mut self) -> SessionUpdate {
        self.state = SessionState::Idle;
        self.speech_accumulator = Duration::ZERO;
        self.silence_accumulator = Duration::ZERO;
        self.hold_accumulator = Duration::ZERO;
        self.active_hold_timeout = self.config.hold_timeout;

        let action = if self.ducked {
            self.ducked = false;
            Some(SessionAction::Restore)
        } else {
            None
        };

        self.snapshot(action)
    }

    fn force_idle(&mut self) -> SessionUpdate {
        self.previous_hotkey_pressed = false;
        self.restore_if_needed()
    }

    fn snapshot(&self, action: Option<SessionAction>) -> SessionUpdate {
        SessionUpdate {
            state: self.state,
            action,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{SessionAction, SessionController, SessionInput, SessionState};
    use crate::config::AppConfig;
    use crate::vad::VadDecision;

    fn input(elapsed_ms: u64, vad: VadDecision) -> SessionInput {
        SessionInput {
            elapsed: Duration::from_millis(elapsed_ms),
            vad,
            hotkey_pressed: false,
            explicit_stop: false,
            output_active: true,
        }
    }

    fn input_with_output(elapsed_ms: u64, vad: VadDecision, output_active: bool) -> SessionInput {
        SessionInput {
            output_active,
            ..input(elapsed_ms, vad)
        }
    }

    #[test]
    fn ducks_after_confirmed_speech() {
        let mut controller = SessionController::new(AppConfig::default());

        let update = controller.update(input(150, VadDecision::Speech));
        assert_eq!(update.action, None);
        assert_eq!(update.state, SessionState::Idle);

        let update = controller.update(input(150, VadDecision::Speech));
        assert_eq!(update.action, Some(SessionAction::Duck));
        assert_eq!(update.state, SessionState::Talking);
    }

    #[test]
    fn short_silence_moves_to_hold_without_restoring() {
        let mut controller = SessionController::new(AppConfig::default());
        controller.update(input(300, VadDecision::Speech));

        let update = controller.update(input(500, VadDecision::Silence));
        assert_eq!(update.action, None);
        assert_eq!(update.state, SessionState::Hold);
    }

    #[test]
    fn hold_timeout_restores_audio() {
        let mut controller = SessionController::new(AppConfig::default());
        controller.update(input(300, VadDecision::Speech));
        controller.update(input(500, VadDecision::Silence));

        let update = controller.update(input(5_000, VadDecision::Silence));
        assert_eq!(update.action, Some(SessionAction::Restore));
        assert_eq!(update.state, SessionState::Idle);
    }

    #[test]
    fn custom_restore_delay_controls_silence_before_restore() {
        let mut config = AppConfig::default();
        config.set_restore_delay(Duration::from_millis(700));
        let mut controller = SessionController::new(config);

        controller.update(input(300, VadDecision::Speech));
        controller.update(input(320, VadDecision::Silence));
        let update = controller.update(input(379, VadDecision::Silence));

        assert_eq!(update.action, None);

        let update = controller.update(input(1, VadDecision::Silence));

        assert_eq!(update.action, Some(SessionAction::Restore));
    }

    #[test]
    fn updating_restore_delay_applies_to_active_hold() {
        let mut controller = SessionController::new(AppConfig::default());
        controller.update(input(300, VadDecision::Speech));
        controller.update(input(320, VadDecision::Silence));

        let mut config = AppConfig::default();
        config.set_restore_delay(Duration::from_millis(600));
        controller.set_config(config);

        let update = controller.update(input(279, VadDecision::Silence));

        assert_eq!(update.action, None);

        let update = controller.update(input(1, VadDecision::Silence));

        assert_eq!(update.action, Some(SessionAction::Restore));
    }

    #[test]
    fn hotkey_ducks_immediately_and_uses_shorter_release_grace() {
        let mut controller = SessionController::new(AppConfig::default());

        let update = controller.update(SessionInput {
            elapsed: Duration::from_millis(10),
            vad: VadDecision::Silence,
            hotkey_pressed: true,
            explicit_stop: false,
            output_active: true,
        });
        assert_eq!(update.action, Some(SessionAction::Duck));
        assert_eq!(update.state, SessionState::Talking);

        let update = controller.update(SessionInput {
            elapsed: Duration::from_millis(10),
            vad: VadDecision::Silence,
            hotkey_pressed: false,
            explicit_stop: false,
            output_active: true,
        });
        assert_eq!(update.action, None);
        assert_eq!(update.state, SessionState::Hold);

        let update = controller.update(input(900, VadDecision::Silence));
        assert_eq!(update.action, Some(SessionAction::Restore));
        assert_eq!(update.state, SessionState::Idle);
    }

    #[test]
    fn explicit_stop_restores_immediately() {
        let mut controller = SessionController::new(AppConfig::default());
        controller.update(input(300, VadDecision::Speech));

        let update = controller.update(SessionInput {
            elapsed: Duration::from_millis(1),
            vad: VadDecision::Silence,
            hotkey_pressed: false,
            explicit_stop: true,
            output_active: true,
        });

        assert_eq!(update.action, Some(SessionAction::Restore));
        assert_eq!(update.state, SessionState::Idle);
    }

    #[test]
    fn manual_restore_waits_for_explicit_stop_after_hold_timeout() {
        let mut config = AppConfig::default();
        config.manual_restore = true;
        let mut controller = SessionController::new(config);

        controller.update(input(300, VadDecision::Speech));
        controller.update(input(500, VadDecision::Silence));

        let update = controller.update(input(5_000, VadDecision::Silence));
        assert_eq!(update.action, None);
        assert_eq!(update.state, SessionState::AwaitingManualRestore);

        let update = controller.update(SessionInput {
            elapsed: Duration::from_millis(1),
            vad: VadDecision::Silence,
            hotkey_pressed: false,
            explicit_stop: true,
            output_active: true,
        });
        assert_eq!(update.action, Some(SessionAction::Restore));
        assert_eq!(update.state, SessionState::Idle);
    }

    #[test]
    fn confirmed_speech_without_output_does_not_duck() {
        let mut controller = SessionController::new(AppConfig::default());

        controller.update(input_with_output(150, VadDecision::Speech, false));
        let update = controller.update(input_with_output(150, VadDecision::Speech, false));

        assert_eq!(update.action, None);
        assert_eq!(update.state, SessionState::Talking);
    }

    #[test]
    fn active_output_while_talking_triggers_duck() {
        let mut controller = SessionController::new(AppConfig::default());

        controller.update(input_with_output(150, VadDecision::Speech, false));
        controller.update(input_with_output(150, VadDecision::Speech, false));
        let update = controller.update(input_with_output(32, VadDecision::Speech, true));

        assert_eq!(update.action, Some(SessionAction::Duck));
        assert_eq!(update.state, SessionState::Talking);
    }

    #[test]
    fn manual_restore_does_not_wait_when_nothing_was_ducked() {
        let mut config = AppConfig::default();
        config.manual_restore = true;
        let mut controller = SessionController::new(config);

        controller.update(input_with_output(300, VadDecision::Speech, false));
        controller.update(input_with_output(500, VadDecision::Silence, false));

        let update = controller.update(input_with_output(5_000, VadDecision::Silence, false));
        assert_eq!(update.action, None);
        assert_eq!(update.state, SessionState::Idle);
    }
}
