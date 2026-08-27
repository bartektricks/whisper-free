//! Windows text insertion: clipboard plus a synthetic Ctrl+V.
//!
//! The shape mirrors the macOS backend deliberately, but one step has no
//! counterpart there: `SendInput` cannot state its modifiers absolutely, so the
//! ones the user is physically holding have to be dealt with first. See
//! [`modifiers_to_clear`].

use std::time::{Duration, Instant};

use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;
use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    VK_V,
};

use crate::text_insertion::{ClipboardOutcome, InsertError, InsertOutcome, TextInserter};

/// How long to let the paste land before putting the old clipboard back.
///
/// The receiving app reads the clipboard asynchronously, so restoring
/// immediately can snatch the text away before it is read.
const PASTE_SETTLE: Duration = Duration::from_millis(150);

/// Modifiers that would corrupt a synthetic Ctrl+V if the user were still
/// holding them.
///
/// `VK_CONTROL` is deliberately absent: Ctrl held is Ctrl we were about to
/// press anyway.
const STRAY_MODIFIERS: [VIRTUAL_KEY; 4] = [VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN];

/// How long to wait for the user to let go of the hotkey before forcing it.
///
/// Normally nothing is held by now — in hold-to-talk the release *is* what
/// started transcription — so this loop usually exits on its first check.
const MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_millis(200);
const MODIFIER_POLL: Duration = Duration::from_millis(10);

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
        // Windows grants nothing up front. The only thing that refuses
        // synthetic input is a more privileged target window, and that cannot
        // be known until the paste is attempted — see `send`.
        true
    }

    fn request_permission(&self) {
        // Nothing to request: there is no Windows permission for synthetic
        // input, and no settings page to open.
    }

    fn insert(
        &self,
        text: &str,
        keep_on_clipboard: bool,
    ) -> Result<InsertOutcome, InsertError> {
        let clipboard = self.app.clipboard();

        // Every format, not just the text: what a user copies out of a
        // spreadsheet or a design tool is several formats of one thing, and
        // putting back only the text would discard the rest. Taking the
        // snapshot is also what renders formats the owner had only advertised
        // (decision 0010).
        // Nothing is captured when the user has asked to keep the
        // transcription: the snapshot exists to be put back, and reading every
        // format costs real work for something about to be discarded.
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

/// Which of [`STRAY_MODIFIERS`] are down and so need releasing before a paste.
///
/// Takes the key-state query as an argument so the rule can be tested without a
/// keyboard, the same split as `hotkey::decide` and `state::is_valid`.
fn modifiers_to_clear(is_down: impl Fn(VIRTUAL_KEY) -> bool) -> Vec<VIRTUAL_KEY> {
    STRAY_MODIFIERS
        .into_iter()
        .filter(|key| is_down(*key))
        .collect()
}

/// Whether a key is physically down right now.
fn key_is_down(key: VIRTUAL_KEY) -> bool {
    // Safety: a pure query taking one virtual-key code.
    let state = unsafe { GetAsyncKeyState(i32::from(key)) };
    // The high bit means "currently down". The low bit is a "pressed since the
    // last call" latch, which would report keys the user has already let go of.
    state < 0
}

/// Build one key-down or key-up event.
fn key_event(key: VIRTUAL_KEY, up: bool) -> INPUT {
    // Fill in the scan code as well as the virtual key: some applications read
    // `wScan` and ignore `wVk`. `KEYEVENTF_SCANCODE` is deliberately not set,
    // which would make Windows ignore `wVk` instead.
    // Safety: a pure lookup; an unmapped key returns 0, which we pass through.
    let scan = unsafe { MapVirtualKeyW(u32::from(key), MAPVK_VK_TO_VSC) };

    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: u16::try_from(scan).unwrap_or(0),
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Post a batch of key events, mapping the one failure worth naming.
fn send(inputs: &[INPUT]) -> Result<(), InsertError> {
    let count = u32::try_from(inputs.len())
        .map_err(|_| InsertError::Keystroke("too many key events".into()))?;
    let size = i32::try_from(std::mem::size_of::<INPUT>())
        .map_err(|_| InsertError::Keystroke("unexpected input size".into()))?;

    // Safety: `inputs` points at `count` valid `INPUT` values, and `size` is
    // the size of that element type, which is what SendInput documents.
    let sent = unsafe { SendInput(count, inputs.as_ptr(), size) };
    if sent == count {
        return Ok(());
    }

    let error = std::io::Error::last_os_error();
    // UIPI: a normal-integrity process may not send input to an elevated
    // window. That is the one refusal a user can act on, so it gets its own
    // variant rather than being reported as a generic keystroke failure.
    if error.raw_os_error().and_then(|c| u32::try_from(c).ok()) == Some(ERROR_ACCESS_DENIED) {
        return Err(InsertError::PermissionDenied);
    }

    Err(InsertError::Keystroke(format!(
        "SendInput accepted {sent} of {count} events: {error}"
    )))
}

/// Synthesise Ctrl+V, first making sure nothing else is being held down.
///
/// A macOS `CGEvent` carries its modifier flags absolutely, so a held Option
/// never reaches the target. `SendInput` has no such concept — the receiving
/// app reads the real keyboard state — so an Alt still down from the hotkey
/// would turn this into Ctrl+Alt+V.
fn send_paste() -> Result<(), InsertError> {
    release_stray_modifiers()?;

    send(&[
        key_event(VK_CONTROL, false),
        key_event(VK_V, false),
        key_event(VK_V, true),
        key_event(VK_CONTROL, true),
    ])
}

/// Give the user a moment to let go, then release whatever is still held.
fn release_stray_modifiers() -> Result<(), InsertError> {
    let started = Instant::now();
    loop {
        let held = modifiers_to_clear(key_is_down);
        if held.is_empty() {
            return Ok(());
        }

        if started.elapsed() >= MODIFIER_RELEASE_TIMEOUT {
            // Waiting longer would delay the paste more than the stray
            // modifier costs. Forcing the key up is the lesser problem: the
            // user's own release will follow harmlessly.
            tracing::debug!(
                event = "forcing_modifier_release",
                count = held.len(),
                "hotkey modifiers still held at paste time"
            );
            let ups: Vec<INPUT> = held.iter().map(|key| key_event(*key, true)).collect();
            return send(&ups);
        }

        std::thread::sleep(MODIFIER_POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_needs_clearing_when_no_modifier_is_held() {
        assert!(modifiers_to_clear(|_| false).is_empty());
    }

    #[test]
    fn a_held_alt_is_cleared_before_pasting() {
        // The hold-to-talk case that would otherwise paste as Ctrl+Alt+V.
        assert_eq!(modifiers_to_clear(|key| key == VK_MENU), vec![VK_MENU]);
    }

    #[test]
    fn control_is_never_cleared_because_the_paste_needs_it() {
        assert!(modifiers_to_clear(|key| key == VK_CONTROL).is_empty());
    }

    #[test]
    fn every_held_modifier_is_reported_at_once() {
        let held = modifiers_to_clear(|_| true);
        assert_eq!(held.len(), STRAY_MODIFIERS.len());
    }

    #[test]
    fn both_windows_keys_are_watched_since_there_is_no_combined_code() {
        assert!(STRAY_MODIFIERS.contains(&VK_LWIN));
        assert!(STRAY_MODIFIERS.contains(&VK_RWIN));
    }
}
