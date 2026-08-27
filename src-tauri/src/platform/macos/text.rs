//! macOS text insertion: clipboard plus a synthetic Cmd+V.

use std::time::Duration;

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;

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

pub struct Inserter {
    app: AppHandle,
}

impl Inserter {
    #[must_use]
    pub const fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl TextInserter for Inserter {
    fn can_insert(&self) -> bool {
        // Safety: a parameterless system query with no arguments to get wrong.
        unsafe { AXIsProcessTrusted() }
    }

    fn request_permission(&self) {
        // Opening the pane is more reliable than the system prompt, which only
        // appears once per app and is easy to miss.
        crate::platform::open_input_permission_settings();
    }

    fn insert(
        &self,
        text: &str,
        keep_on_clipboard: bool,
    ) -> Result<InsertOutcome, InsertError> {
        if !self.can_insert() {
            return Err(InsertError::PermissionDenied);
        }

        let clipboard = self.app.clipboard();

        // Every flavour, not just the text: what a user copies out of a
        // spreadsheet or a design tool is several flavours of one thing, and
        // putting back only the text would discard the rest. Taking the
        // snapshot is also what rescues text macOS had only promised, which is
        // the case that used to be reported as an image (decision 0010).
        // Nothing is captured when the user has asked to keep the
        // transcription: the snapshot exists to be put back, and reading every
        // flavour costs real work for something about to be discarded.
        let previous = (!keep_on_clipboard).then(super::clipboard::capture);

        clipboard
            .write_text(text)
            .map_err(|e| InsertError::Clipboard(e.to_string()))?;

        // On failure the text stays on the clipboard: the user can still paste
        // it themselves, which beats losing the transcription entirely.
        send_paste()?;

        std::thread::sleep(PASTE_SETTLE);

        let outcome = previous.map_or(ClipboardOutcome::Kept, |previous| {
            let restored = super::clipboard::restore(&previous);
            if let Err(e) = &restored {
                tracing::warn!(error = %e, "could not restore the clipboard");
            }
            ClipboardOutcome::after_restore(
                restored.is_ok(),
                previous.is_empty(),
                previous.is_incomplete(),
            )
        });

        Ok(InsertOutcome { clipboard: outcome })
    }
}

/// Synthesise Cmd+V at the HID level, so the frontmost app sees a real paste.
///
/// The flags are set on the event rather than by pressing Command, so a
/// modifier the user happens to be holding cannot leak into the shortcut. The
/// Windows backend has no equivalent and has to clear them by hand.
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
