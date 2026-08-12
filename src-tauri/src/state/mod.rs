//! Authoritative application state (plan §15).
//!
//! Rust owns the state; the UI subscribes to `state_changed` events and only
//! renders what it is told. Transitions are validated so an out-of-order event
//! (a stray hotkey release, a late transcription result) can never leave the
//! app in an inconsistent state.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppState {
    /// Started, but no model is loaded yet — dictation is not possible.
    Uninitialized,
    /// Idle and able to record.
    Ready,
    /// Capturing microphone audio.
    Recording,
    /// Running inference on captured audio.
    Transcribing,
    /// Placing text into the focused application.
    Inserting,
    /// Something failed; carries a user-facing message alongside.
    Error,
}

impl fmt::Display for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Uninitialized => "uninitialized",
            Self::Ready => "ready",
            Self::Recording => "recording",
            Self::Transcribing => "transcribing",
            Self::Inserting => "inserting",
            Self::Error => "error",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid state transition: {from} -> {to}")]
pub struct InvalidTransition {
    pub from: AppState,
    pub to: AppState,
}

/// Is `from -> to` a transition the dictation pipeline can actually make?
///
/// The happy path is Ready -> Recording -> Transcribing -> Inserting -> Ready.
/// Every state may fail into `Error`, and any state may re-enter itself as a
/// no-op so that repeated events are harmless.
fn is_valid(from: AppState, to: AppState) -> bool {
    use AppState::{Error, Inserting, Ready, Recording, Transcribing, Uninitialized};

    if from == to {
        return true; // idempotent: repeated events must not be errors
    }
    if to == Error {
        return true; // anything can fail
    }

    match from {
        // Uninitialized has nowhere to go but Ready once a model loads;
        // Inserting is the last pipeline step and returns there too.
        Uninitialized | Inserting => matches!(to, Ready),
        // Back to Uninitialized when the model is unloaded or removed.
        Ready => matches!(to, Recording | Uninitialized),
        // Ready covers a cancelled or empty recording.
        Recording => matches!(to, Transcribing | Ready),
        // Ready covers a transcription that produced no text.
        Transcribing => matches!(to, Inserting | Ready),
        // Recording is reachable from Error because starting a new dictation is
        // how a user retries. Requiring a Ready hop first created a race: the
        // pipeline reports failures from a worker thread, so an error can land
        // between the check and the transition.
        Error => matches!(to, Ready | Uninitialized | Recording),
    }
}

/// The current state plus the message describing the most recent failure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateSnapshot {
    pub state: AppState,
    /// User-facing text. Never a raw Rust or model error (plan §17).
    pub message: Option<String>,
}

#[derive(Debug)]
pub struct StateMachine {
    state: AppState,
    message: Option<String>,
}

impl Default for StateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl StateMachine {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: AppState::Uninitialized,
            message: None,
        }
    }

    #[must_use]
    pub const fn state(&self) -> AppState {
        self.state
    }

    #[must_use]
    pub fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            state: self.state,
            message: self.message.clone(),
        }
    }

    /// Move to `next`, rejecting transitions the pipeline cannot make.
    ///
    /// Returns the new snapshot on success so the caller can emit it.
    ///
    /// # Errors
    ///
    /// [`InvalidTransition`] when `next` is not reachable from the current
    /// state. The machine is left untouched.
    pub fn transition_to(&mut self, next: AppState) -> Result<StateSnapshot, InvalidTransition> {
        if !is_valid(self.state, next) {
            return Err(InvalidTransition {
                from: self.state,
                to: next,
            });
        }
        self.state = next;
        if next != AppState::Error {
            self.message = None;
        }
        Ok(self.snapshot())
    }

    /// Enter `Error` with a message meant for a human to read.
    pub fn fail(&mut self, message: impl Into<String>) -> StateSnapshot {
        self.state = AppState::Error;
        self.message = Some(message.into());
        self.snapshot()
    }

    /// Recover from an error back to a usable state.
    ///
    /// # Errors
    ///
    /// [`InvalidTransition`] when `to` is not reachable from the current state.
    pub fn clear_error(&mut self, to: AppState) -> Result<StateSnapshot, InvalidTransition> {
        self.message = None;
        self.transition_to(to)
    }
}

#[cfg(test)]
mod tests {
    use super::AppState::*;
    use super::*;

    #[test]
    fn starts_uninitialized() {
        let sm = StateMachine::new();
        assert_eq!(sm.state(), Uninitialized);
        assert_eq!(sm.snapshot().message, None);
    }

    #[test]
    fn walks_the_happy_path() {
        let mut sm = StateMachine::new();
        for next in [Ready, Recording, Transcribing, Inserting, Ready] {
            assert!(sm.transition_to(next).is_ok(), "should allow -> {next}");
            assert_eq!(sm.state(), next);
        }
    }

    #[test]
    fn rejects_recording_before_a_model_is_loaded() {
        let mut sm = StateMachine::new();
        let err = sm.transition_to(Recording).unwrap_err();
        assert_eq!(err.from, Uninitialized);
        assert_eq!(err.to, Recording);
        // Rejected transitions must not move the machine.
        assert_eq!(sm.state(), Uninitialized);
    }

    #[test]
    fn rejects_skipping_the_recording_step() {
        let mut sm = StateMachine::new();
        sm.transition_to(Ready).unwrap();
        assert!(sm.transition_to(Transcribing).is_err());
        assert!(sm.transition_to(Inserting).is_err());
        assert_eq!(sm.state(), Ready);
    }

    #[test]
    fn repeated_events_are_harmless() {
        // A key-repeat storm or a duplicated event must not error out.
        let mut sm = StateMachine::new();
        sm.transition_to(Ready).unwrap();
        sm.transition_to(Recording).unwrap();
        assert!(sm.transition_to(Recording).is_ok());
        assert_eq!(sm.state(), Recording);
    }

    #[test]
    fn cancelled_recording_returns_to_ready() {
        let mut sm = StateMachine::new();
        sm.transition_to(Ready).unwrap();
        sm.transition_to(Recording).unwrap();
        assert!(sm.transition_to(Ready).is_ok());
    }

    #[test]
    fn empty_transcription_returns_to_ready_without_inserting() {
        // The Parakeet empty-output failure mode from decision 0001: we must be
        // able to abandon the pipeline after transcribing without inserting.
        let mut sm = StateMachine::new();
        sm.transition_to(Ready).unwrap();
        sm.transition_to(Recording).unwrap();
        sm.transition_to(Transcribing).unwrap();
        assert!(sm.transition_to(Ready).is_ok());
    }

    #[test]
    fn any_state_can_fail_and_carries_a_message() {
        for start in [Uninitialized, Ready, Recording, Transcribing, Inserting] {
            let mut sm = StateMachine::new();
            // Drive to `start` along the happy path.
            for next in [Ready, Recording, Transcribing, Inserting] {
                if sm.state() == start {
                    break;
                }
                sm.transition_to(next).unwrap();
            }
            let snap = sm.fail("Microphone is unavailable");
            assert_eq!(snap.state, Error);
            assert_eq!(snap.message.as_deref(), Some("Microphone is unavailable"));
        }
    }

    #[test]
    fn recovering_from_error_clears_the_message() {
        let mut sm = StateMachine::new();
        sm.transition_to(Ready).unwrap();
        sm.fail("Transcription failed");
        assert_eq!(sm.state(), Error);

        let snap = sm.clear_error(Ready).unwrap();
        assert_eq!(snap.state, Ready);
        assert_eq!(snap.message, None);
    }

    #[test]
    fn retrying_after_an_error_starts_recording_and_clears_the_message() {
        // Pressing the hotkey again is the natural retry, and the failure
        // message must not survive into the new attempt.
        let mut sm = StateMachine::new();
        sm.transition_to(Ready).unwrap();
        sm.fail("No audio was captured");

        let snap = sm.transition_to(Recording).unwrap();
        assert_eq!(snap.state, Recording);
        assert_eq!(snap.message, None);
    }

    #[test]
    fn error_still_cannot_skip_into_the_middle_of_the_pipeline() {
        // Allowing Error -> Recording must not open up Error -> Transcribing.
        let mut sm = StateMachine::new();
        sm.transition_to(Ready).unwrap();
        sm.fail("boom");
        assert!(sm.transition_to(Transcribing).is_err());
        assert!(sm.transition_to(Inserting).is_err());
    }

    #[test]
    fn a_new_dictation_can_start_after_a_failure() {
        // The pipeline clears the error before recording again; without that
        // step the mic would open while the state machine refused to follow.
        let mut sm = StateMachine::new();
        sm.transition_to(Ready).unwrap();
        sm.fail("No audio was captured");

        sm.clear_error(Ready).unwrap();
        assert!(sm.transition_to(Recording).is_ok());
        assert_eq!(sm.state(), Recording);
    }

    #[test]
    fn successful_transition_clears_a_stale_message() {
        let mut sm = StateMachine::new();
        sm.transition_to(Ready).unwrap();
        sm.fail("old failure");
        let snap = sm.transition_to(Ready).unwrap();
        assert_eq!(snap.message, None);
    }
}
