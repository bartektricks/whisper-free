//! Getting transcribed text into whatever app has focus (plan §11).
//!
//! There are no per-application integrations and there will not be: the app
//! being typed into is none of our business. We put text on the clipboard and
//! send a paste, which is the one mechanism every text field understands.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InsertError {
    #[error("accessibility permission has not been granted")]
    PermissionDenied,
    #[error("could not use the clipboard: {0}")]
    Clipboard(String),
    #[error("could not send the paste keystroke: {0}")]
    Keystroke(String),
}

impl InsertError {
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            // Whole sentences per platform: macOS withholds a permission
            // before anything happens, while Windows refuses input to a more
            // privileged window once the text is already on the clipboard.
            Self::PermissionDenied => crate::platform::strings::INSERT_PERMISSION_DENIED.into(),
            Self::Clipboard(_) => {
                "The text could not be placed on the clipboard. Try again.".into()
            }
            Self::Keystroke(_) => {
                format!(
                    "The text was copied to the clipboard, but pasting failed. Press {} to paste it.",
                    crate::platform::strings::PASTE_SHORTCUT
                )
            }
        }
    }
}

/// What happened to the user's previous clipboard contents.
///
/// Reported so the caller can tell the user when something was lost rather
/// than letting it vanish silently (plan §11).
///
/// Every flavour of the previous clipboard is captured and put back, not just
/// the text, so a rich clipboard survives a dictation. There used to be a
/// `NonTextReplaced` variant covering "not text, so not preserved"; it was
/// inferred from a failed text read, which conflates *no text was there* with
/// *the text there could not be read*, and macOS produces the second far more
/// often than the first. See `docs/decisions/0010-preserving-the-clipboard.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardOutcome {
    /// Everything that was there was put back.
    Restored,
    /// The clipboard was empty to begin with, so there was nothing to restore.
    NothingToRestore,
    /// The clipboard was put back, but a flavour the system had listed could
    /// not be read at all, so what came back is poorer than what was there.
    PartlyRestored,
    /// Restoration was attempted and failed.
    RestoreFailed,
}

impl ClipboardOutcome {
    /// What to report after putting a captured clipboard back.
    ///
    /// Pure, so the rule can be checked without a pasteboard, the same split as
    /// `hotkey::decide` and `state::is_valid`. Both backends share it: the two
    /// platforms capture very different things, but what the user is owed about
    /// the result is the same on each.
    #[must_use]
    pub const fn after_restore(succeeded: bool, was_empty: bool, was_incomplete: bool) -> Self {
        if !succeeded {
            return Self::RestoreFailed;
        }
        // Emptiness first: an empty clipboard cannot be partly anything, and
        // "nothing to restore" is the more honest of the two.
        if was_empty {
            Self::NothingToRestore
        } else if was_incomplete {
            Self::PartlyRestored
        } else {
            Self::Restored
        }
    }

    /// Whether the user is measurably worse off than before the insertion.
    ///
    /// [`Self::PartlyRestored`] is deliberately not: a flavour that could not be
    /// read is nearly always one the system derives from another it *could*
    /// read, and it reappears of its own accord once that one is back. Saying
    /// so would be the false alarm decision 0010 set out to remove.
    #[must_use]
    pub const fn lost_the_clipboard(self) -> bool {
        matches!(self, Self::RestoreFailed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct InsertOutcome {
    pub clipboard: ClipboardOutcome,
}

/// Places text into the focused application.
pub trait TextInserter: Send + Sync {
    /// Whether the OS will currently let us synthesise keystrokes.
    fn can_insert(&self) -> bool;

    /// Ask the OS for whatever permission insertion needs.
    ///
    /// On macOS this shows the Accessibility prompt. Safe to call repeatedly.
    fn request_permission(&self);

    /// Put `text` into the focused application.
    ///
    /// # Errors
    ///
    /// [`InsertError::PermissionDenied`] when the OS refuses synthetic input,
    /// [`InsertError::Clipboard`] when the clipboard cannot be written, and
    /// [`InsertError::Keystroke`] when the paste cannot be sent — in which case
    /// the text is still on the clipboard.
    fn insert(&self, text: &str) -> Result<InsertOutcome, InsertError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_message_says_what_the_user_can_do_about_it() {
        let msg = InsertError::PermissionDenied.user_message();
        // The wording differs per platform, but it must always name a concrete
        // next step rather than just reporting the refusal.
        assert!(msg.contains("WhisperFree"));
        assert!(
            msg.contains("System Settings") || msg.contains(crate::platform::strings::PASTE_SHORTCUT),
            "no actionable next step: {msg}"
        );
    }

    #[test]
    fn a_failed_paste_tells_the_user_the_text_is_still_recoverable() {
        // The transcription is on the clipboard at that point, so losing the
        // keystroke should not mean losing the words.
        let msg = InsertError::Keystroke("CGEvent creation failed".into()).user_message();
        assert!(msg.contains(crate::platform::strings::PASTE_SHORTCUT));
        assert!(!msg.contains("CGEvent"), "leaked internals: {msg}");
    }

    #[test]
    fn clipboard_errors_stay_readable() {
        let msg = InsertError::Clipboard("NSPasteboard error -25300".into()).user_message();
        assert!(!msg.contains("NSPasteboard"), "leaked internals: {msg}");
    }

    #[test]
    fn a_clipboard_that_came_back_whole_is_not_reported_as_a_loss() {
        let outcome = ClipboardOutcome::after_restore(true, false, false);
        assert_eq!(outcome, ClipboardOutcome::Restored);
        assert!(!outcome.lost_the_clipboard());
    }

    #[test]
    fn an_underivable_flavour_is_recorded_but_never_surfaced() {
        // The case that used to be reported as "your clipboard held an image":
        // the system lists a flavour it can no longer produce, so the snapshot
        // is incomplete even though restoring the flavour it *was* derived from
        // brings it back. Worth a log line, never worth an error.
        let outcome = ClipboardOutcome::after_restore(true, false, true);
        assert_eq!(outcome, ClipboardOutcome::PartlyRestored);
        assert!(!outcome.lost_the_clipboard());
    }

    #[test]
    fn an_empty_clipboard_is_never_called_partly_restored() {
        // Nothing was captured, so "incomplete" says nothing the user needs.
        let outcome = ClipboardOutcome::after_restore(true, true, true);
        assert_eq!(outcome, ClipboardOutcome::NothingToRestore);
        assert!(!outcome.lost_the_clipboard());
    }

    #[test]
    fn only_a_clipboard_that_could_not_be_written_back_is_surfaced() {
        let outcome = ClipboardOutcome::after_restore(false, false, false);
        assert_eq!(outcome, ClipboardOutcome::RestoreFailed);
        assert!(outcome.lost_the_clipboard());

        // A failure outranks everything else it could be called.
        assert_eq!(
            ClipboardOutcome::after_restore(false, true, true),
            ClipboardOutcome::RestoreFailed
        );
    }

    #[test]
    fn user_messages_never_leak_platform_internals() {
        let errors = [
            InsertError::PermissionDenied,
            InsertError::Clipboard("NSPasteboard error -25300".into()),
            InsertError::Keystroke("SendInput accepted 0 of 4 events: HRESULT 0x5".into()),
        ];
        for e in errors {
            let msg = e.user_message();
            for internal in ["CGEvent", "NSPasteboard", "SendInput", "HRESULT", "clipboard-win"] {
                assert!(!msg.contains(internal), "leaked internals: {msg}");
            }
        }
    }
}
