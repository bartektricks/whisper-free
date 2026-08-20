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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardOutcome {
    /// Previous text was put back.
    Restored,
    /// The clipboard was empty to begin with, so there was nothing to restore.
    NothingToRestore,
    /// The clipboard held something that is not text (an image, files). It
    /// could not be preserved across the paste.
    NonTextReplaced,
    /// Restoration was attempted and failed.
    RestoreFailed,
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
