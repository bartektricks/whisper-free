//! macOS text insertion: clipboard plus a synthetic Cmd+V.

use std::time::Duration;

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

use crate::text_insertion::{ClipboardOutcome, InsertError, InsertOutcome, TextInserter};

/// Virtual key code for `V` on macOS (`kVK_ANSI_V`).
const KEY_V: u16 = 9;

/// How long to let the paste land before putting the old clipboard back.
///
/// The receiving app reads the pasteboard asynchronously, so restoring
/// immediately can snatch the text away before it is read.
const PASTE_SETTLE: Duration = Duration::from_millis(150);

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

pub struct MacOSTextInserter;

impl MacOSTextInserter {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for MacOSTextInserter {
    fn default() -> Self {
        Self::new()
    }
}

impl TextInserter for MacOSTextInserter {
    fn can_insert(&self) -> bool {
        // Safety: a parameterless system query with no arguments to get wrong.
        unsafe { AXIsProcessTrusted() }
    }

    fn request_permission(&self) {
        // Opening the pane is more reliable than the system prompt, which only
        // appears once per app and is easy to miss.
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn();
    }

    fn insert(&self, text: &str) -> Result<InsertOutcome, InsertError> {
        if !self.can_insert() {
            return Err(InsertError::PermissionDenied);
        }

        let mut clipboard =
            arboard::Clipboard::new().map_err(|e| InsertError::Clipboard(e.to_string()))?;

        // Remember what was there. A read failure is not necessarily an empty
        // clipboard — it may hold an image or files, which we cannot preserve.
        let previous = clipboard.get_text().ok();
        let had_non_text = previous.is_none() && clipboard.get_image().is_ok();

        clipboard
            .set_text(text.to_string())
            .map_err(|e| InsertError::Clipboard(e.to_string()))?;

        // On failure the text stays on the clipboard: the user can still paste
        // it themselves, which beats losing the transcription entirely.
        send_paste()?;

        std::thread::sleep(PASTE_SETTLE);

        let outcome = match previous {
            Some(previous) => match clipboard.set_text(previous) {
                Ok(()) => ClipboardOutcome::Restored,
                Err(e) => {
                    tracing::warn!(error = %e, "could not restore the clipboard");
                    ClipboardOutcome::RestoreFailed
                }
            },
            None if had_non_text => ClipboardOutcome::NonTextReplaced,
            None => ClipboardOutcome::NothingToRestore,
        };

        Ok(InsertOutcome { clipboard: outcome })
    }
}

/// Synthesise Cmd+V at the HID level, so the frontmost app sees a real paste.
fn send_paste() -> Result<(), InsertError> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|()| InsertError::Keystroke("could not create an event source".into()))?;

    let down = CGEvent::new_keyboard_event(source.clone(), KEY_V, true)
        .map_err(|()| InsertError::Keystroke("could not create the key-down event".into()))?;
    down.set_flags(CGEventFlags::CGEventFlagCommand);
    down.post(CGEventTapLocation::HID);

    let up = CGEvent::new_keyboard_event(source, KEY_V, false)
        .map_err(|()| InsertError::Keystroke("could not create the key-up event".into()))?;
    up.set_flags(CGEventFlags::CGEventFlagCommand);
    up.post(CGEventTapLocation::HID);

    Ok(())
}
