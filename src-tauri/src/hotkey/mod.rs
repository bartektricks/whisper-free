//! Global hotkey handling (plan §10).
//!
//! The decision of what a key event *means* is pure logic and lives in
//! [`decide`], separate from the OS registration in [`platform`]. That keeps
//! the tricky part — key repeat, out-of-order events, mode switches — testable
//! without a window server.

pub mod chord;
pub mod platform;

use serde::{Deserialize, Serialize};

use crate::settings::RecordingMode;

pub use chord::{Arming, Chord, ChordStep, CHORD_TIMEOUT};
pub use platform::{is_cancel, GlobalHotkeys, Shortcut, TauriGlobalHotkeys, CANCEL_ACCELERATOR};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HotkeyError {
    #[error("\"{0}\" is not a valid shortcut")]
    Invalid(String),
    #[error("\"{0}\" is already used by another application")]
    AlreadyTaken(String),
    #[error("could not register the shortcut: {0}")]
    Registration(String),
    #[error("\"{0}\" has more than two steps")]
    TooManySteps(String),
    #[error("\"{0}\" uses the same combination for both steps")]
    RepeatedStep(String),
}

impl HotkeyError {
    /// Message for the user (plan §17).
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::Invalid(accel) => {
                format!("\"{accel}\" isn't a shortcut LocalDictation understands. Try one with a modifier, like Option+Space.")
            }
            Self::AlreadyTaken(accel) => {
                format!("{accel} is already taken by another app. Pick a different shortcut in Settings › General.")
            }
            Self::Registration(_) => {
                "That shortcut could not be registered. Pick a different one in Settings › General."
                    .into()
            }
            Self::TooManySteps(_) => {
                "A shortcut can be one combination, or two pressed in sequence — not more."
                    .into()
            }
            Self::RepeatedStep(_) => {
                "Both steps of that shortcut are the same combination. Make the second one different, like Cmd+K then C."
                    .into()
            }
        }
    }
}

/// A raw key transition from the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyEvent {
    Pressed,
    Released,
}

/// What the dictation pipeline should do about a key transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Start,
    Stop,
    /// Nothing to do — a key repeat, or an edge this mode does not use.
    Ignore,
}

/// Map a key transition to an action, given the mode and whether we are
/// already recording.
///
/// `recording` is what makes this safe against the messy cases: macOS repeats
/// `Pressed` while a key is held, and a release can arrive after the recording
/// was already stopped some other way.
#[must_use]
pub const fn decide(mode: RecordingMode, event: HotkeyEvent, recording: bool) -> HotkeyAction {
    match (mode, event, recording) {
        // A press with nothing running starts, in either mode.
        (_, HotkeyEvent::Pressed, false) => HotkeyAction::Start,
        // Hold to talk stops when the key goes up; toggle stops on the second
        // press.
        (RecordingMode::HoldToTalk, HotkeyEvent::Released, true)
        | (RecordingMode::Toggle, HotkeyEvent::Pressed, true) => HotkeyAction::Stop,
        // Everything else is noise: a hold-to-talk key repeat (macOS resends
        // Pressed while the key is down), a release arriving after the
        // recording already stopped, or a toggle-mode release.
        _ => HotkeyAction::Ignore,
    }
}

/// Check that a shortcut string is one the OS layer will accept, before we try
/// to store it.
///
/// Accepts one combination or a two-step chord, so this is where a malformed
/// chord is caught rather than at registration time, when only the prefix would
/// be looked at.
///
/// # Errors
///
/// Whatever [`Chord::parse`] rejects it for.
pub fn validate(accelerator: &str) -> Result<(), HotkeyError> {
    Chord::parse(accelerator).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::HotkeyAction::*;
    use super::HotkeyEvent::*;
    use super::*;
    use crate::settings::RecordingMode::*;

    #[test]
    fn hold_to_talk_starts_on_press_and_stops_on_release() {
        assert_eq!(decide(HoldToTalk, Pressed, false), Start);
        assert_eq!(decide(HoldToTalk, Released, true), Stop);
    }

    #[test]
    fn hold_to_talk_ignores_key_repeat() {
        // macOS repeats Pressed while the key is held down. Without this the
        // recording would restart dozens of times a second.
        assert_eq!(decide(HoldToTalk, Pressed, true), Ignore);
    }

    #[test]
    fn hold_to_talk_ignores_a_release_we_did_not_ask_for() {
        // e.g. the recording already stopped because the mic disappeared.
        assert_eq!(decide(HoldToTalk, Released, false), Ignore);
    }

    #[test]
    fn toggle_flips_on_each_press() {
        assert_eq!(decide(Toggle, Pressed, false), Start);
        assert_eq!(decide(Toggle, Pressed, true), Stop);
    }

    #[test]
    fn toggle_ignores_releases() {
        // Otherwise letting go of the key would immediately stop the recording
        // the press just started.
        assert_eq!(decide(Toggle, Released, false), Ignore);
        assert_eq!(decide(Toggle, Released, true), Ignore);
    }

    #[test]
    fn a_full_hold_to_talk_cycle_records_exactly_once() {
        let mut recording = false;
        let events = [Pressed, Pressed, Pressed, Released];
        let mut starts = 0;
        let mut stops = 0;

        for event in events {
            match decide(HoldToTalk, event, recording) {
                Start => {
                    starts += 1;
                    recording = true;
                }
                Stop => {
                    stops += 1;
                    recording = false;
                }
                Ignore => {}
            }
        }
        assert_eq!((starts, stops), (1, 1));
        assert!(!recording);
    }

    #[test]
    fn rapid_toggle_presses_alternate_cleanly() {
        let mut recording = false;
        let mut log = Vec::new();
        for _ in 0..4 {
            // Each press is followed by a release that must not interfere.
            for event in [Pressed, Released] {
                match decide(Toggle, event, recording) {
                    Start => {
                        log.push("start");
                        recording = true;
                    }
                    Stop => {
                        log.push("stop");
                        recording = false;
                    }
                    Ignore => {}
                }
            }
        }
        assert_eq!(log, ["start", "stop", "start", "stop"]);
    }

    #[test]
    fn common_shortcuts_validate() {
        for accel in ["Alt+Space", "CmdOrCtrl+Shift+D", "F5", "Ctrl+Alt+K"] {
            assert!(validate(accel).is_ok(), "{accel} should be valid");
        }
    }

    #[test]
    fn nonsense_shortcuts_are_rejected() {
        for accel in ["", "NotAKey", "Alt+", "++"] {
            assert!(validate(accel).is_err(), "{accel:?} should be rejected");
        }
    }

    #[test]
    fn invalid_shortcut_message_suggests_a_working_one() {
        let msg = HotkeyError::Invalid("qq".into()).user_message();
        assert!(msg.contains("Option+Space"));
    }
}
