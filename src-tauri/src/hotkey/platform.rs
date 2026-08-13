//! OS registration for global hotkeys.
//!
//! The rest of the app talks to [`GlobalHotkeys`], never to the plugin, so a
//! Windows or Linux implementation can be dropped in later (plan §10, §22).

use std::str::FromStr;

use tauri::AppHandle;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

pub use tauri_plugin_global_shortcut::Shortcut;

use super::HotkeyError;

/// Registering and releasing system-wide shortcuts.
///
/// More than one can be live at a time, because the second step of a chord is
/// registered alongside the prefix for as long as the chord window is open.
pub trait GlobalHotkeys: Send + Sync {
    /// Make `accelerator` the only registered shortcut.
    ///
    /// # Errors
    ///
    /// [`HotkeyError::Invalid`] when the accelerator does not parse, or
    /// [`HotkeyError::AlreadyTaken`] when another application owns it.
    fn register(&self, accelerator: &str) -> Result<(), HotkeyError>;

    /// Register `accelerator` as well as whatever is already registered.
    ///
    /// # Errors
    ///
    /// As [`Self::register`].
    fn add(&self, accelerator: &str) -> Result<(), HotkeyError>;

    /// Release one accelerator, leaving the rest registered.
    ///
    /// # Errors
    ///
    /// [`HotkeyError::Invalid`] when the accelerator does not parse, or
    /// [`HotkeyError::Registration`] when the OS refuses to release it.
    fn remove(&self, accelerator: &str) -> Result<(), HotkeyError>;

    /// Release every shortcut we hold.
    ///
    /// # Errors
    ///
    /// [`HotkeyError::Registration`] when the OS refuses to release them.
    fn unregister_all(&self) -> Result<(), HotkeyError>;
}

/// The key that abandons a dictation in progress.
///
/// Held from the OS only while a dictation is actually running. Registering it
/// permanently would swallow Escape system-wide, which is not a trade anyone
/// would accept for a dictation app.
pub const CANCEL_ACCELERATOR: &str = "Escape";

/// Is `fired` the cancel key, pressed on its own?
///
/// Modifiers matter: `Cmd+Escape` is somebody else's shortcut, and we only ever
/// register the bare key.
#[must_use]
pub fn is_cancel(fired: &Shortcut) -> bool {
    parse(CANCEL_ACCELERATOR).is_ok_and(|cancel| cancel == *fired)
}

/// Parse an accelerator string such as `"Alt+Space"`.
///
/// # Errors
///
/// [`HotkeyError::Invalid`] when the string is empty or is not an accelerator.
pub fn parse(accelerator: &str) -> Result<Shortcut, HotkeyError> {
    if accelerator.trim().is_empty() {
        return Err(HotkeyError::Invalid(accelerator.to_string()));
    }
    Shortcut::from_str(accelerator).map_err(|_| HotkeyError::Invalid(accelerator.to_string()))
}

pub struct TauriGlobalHotkeys {
    app: AppHandle,
}

impl TauriGlobalHotkeys {
    #[must_use]
    pub const fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl GlobalHotkeys for TauriGlobalHotkeys {
    fn register(&self, accelerator: &str) -> Result<(), HotkeyError> {
        // Whatever was live belongs to the previous hotkey, including a chord
        // second step left registered by a window that is now irrelevant.
        // Ignore failures: there may be nothing registered yet.
        let _ = self.app.global_shortcut().unregister_all();
        self.add(accelerator)
    }

    fn add(&self, accelerator: &str) -> Result<(), HotkeyError> {
        let shortcut = parse(accelerator)?;

        self.app.global_shortcut().register(shortcut).map_err(|e| {
            let text = e.to_string();
            // Another app already owns it — worth telling the user plainly
            // rather than leaving them with a shortcut that does nothing.
            if text.to_lowercase().contains("already") {
                HotkeyError::AlreadyTaken(accelerator.to_string())
            } else {
                HotkeyError::Registration(text)
            }
        })
    }

    fn remove(&self, accelerator: &str) -> Result<(), HotkeyError> {
        let shortcut = parse(accelerator)?;
        self.app
            .global_shortcut()
            .unregister(shortcut)
            .map_err(|e| HotkeyError::Registration(e.to_string()))
    }

    fn unregister_all(&self) -> Result<(), HotkeyError> {
        self.app
            .global_shortcut()
            .unregister_all()
            .map_err(|e| HotkeyError::Registration(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cancel_accelerator_parses() {
        assert!(parse(CANCEL_ACCELERATOR).is_ok());
    }

    #[test]
    fn a_bare_escape_is_the_cancel_key() {
        let escape = parse(CANCEL_ACCELERATOR).unwrap();
        assert!(is_cancel(&escape));
    }

    /// Escape with a modifier belongs to someone else, and is never something
    /// we registered.
    #[test]
    fn escape_with_a_modifier_is_not_the_cancel_key() {
        for accelerator in ["Cmd+Escape", "Ctrl+Escape", "Alt+Escape", "Shift+Escape"] {
            let shortcut = parse(accelerator).unwrap();
            assert!(!is_cancel(&shortcut), "{accelerator} matched");
        }
    }

    #[test]
    fn an_ordinary_hotkey_is_not_the_cancel_key() {
        for accelerator in ["Alt+Space", "Cmd+K", "Escape+A"] {
            if let Ok(shortcut) = parse(accelerator) {
                assert!(!is_cancel(&shortcut), "{accelerator} matched");
            }
        }
    }
}
