//! OS registration for global hotkeys.
//!
//! The rest of the app talks to [`GlobalHotkeys`], never to the plugin, so a
//! Windows or Linux implementation can be dropped in later (plan §10, §22).

use std::str::FromStr;

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use super::HotkeyError;

/// Registering and releasing the system-wide shortcut.
pub trait GlobalHotkeys: Send + Sync {
    /// Make `accelerator` the only registered shortcut.
    ///
    /// # Errors
    ///
    /// [`HotkeyError::Invalid`] when the accelerator does not parse, or
    /// [`HotkeyError::AlreadyTaken`] when another application owns it.
    fn register(&self, accelerator: &str) -> Result<(), HotkeyError>;

    /// Release every shortcut we hold.
    ///
    /// # Errors
    ///
    /// [`HotkeyError::Registration`] when the OS refuses to release them.
    fn unregister_all(&self) -> Result<(), HotkeyError>;
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
        let shortcut = parse(accelerator)?;
        let manager = self.app.global_shortcut();

        // Exactly one dictation shortcut is live at a time, so clear first.
        // Ignore failures here: there may be nothing registered yet.
        let _ = manager.unregister_all();

        manager.register(shortcut).map_err(|e| {
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

    fn unregister_all(&self) -> Result<(), HotkeyError> {
        self.app
            .global_shortcut()
            .unregister_all()
            .map_err(|e| HotkeyError::Registration(e.to_string()))
    }
}
