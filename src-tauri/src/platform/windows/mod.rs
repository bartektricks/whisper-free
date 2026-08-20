//! The Windows backend.
//!
//! Reached only as `platform::backend`; nothing outside `platform/` may name
//! this module. Every item here is part of the backend contract documented in
//! `platform/mod.rs`.

pub mod focus;
pub mod replay;
pub mod text;

use super::TrayAccelerators;

/// `Alt+Space` — the macOS default — opens the system window menu on Windows,
/// so it cannot be used here. `Ctrl+Alt+Space` is the nearest combination that
/// neither the shell nor common applications claim.
pub const DEFAULT_HOTKEY: &str = "Ctrl+Alt+Space";

/// Notification-area menus do not carry working shortcuts on Windows, and
/// showing one would advertise a key combination that does nothing.
pub const TRAY_ACCELERATORS: TrayAccelerators = TrayAccelerators {
    settings: None,
    quit: None,
};

/// Template tinting is a macOS concept; Windows draws the icon as supplied.
pub const TRAY_ICON_IS_TEMPLATE: bool = false;

/// Windows convention: left click performs the primary action, right click
/// opens the menu. `tray.rs` opens Settings on left click instead.
pub const TRAY_MENU_ON_LEFT_CLICK: bool = false;

pub mod strings {
    pub const PASTE_SHORTCUT: &str = "Ctrl+V";

    pub const EXAMPLE_CHORD: &str = "Ctrl+K then C";

    pub const MICROPHONE_SETTINGS: &str = "Settings › Privacy & security › Microphone";
    pub const MICROPHONE_SETTINGS_URL: &str = "ms-settings:privacy-microphone";

    pub const LOGIN_ITEMS_SETTINGS: &str = "Settings › Apps › Startup";

    /// Windows does not gate synthetic input behind a permission, so there is
    /// no page to send the user to.
    pub const INPUT_PERMISSION_SETTINGS_URL: Option<&str> = None;

    /// Windows has no permission to grant here. The one thing that can refuse
    /// synthetic input is a more privileged window, and by the time that is
    /// discovered the text is already on the clipboard — so the message says
    /// so rather than naming a settings page that does not exist.
    pub const INSERT_PERMISSION_DENIED: &str = "Windows would not let WhisperFree paste into that window, which usually means the app is running as administrator. The text is on your clipboard — press Ctrl+V to paste it.";
}

/// Nothing to do: the app has no window visible at startup, so it never claims
/// a taskbar button until the user opens Settings — and once Settings is open,
/// a taskbar button is what a Windows user expects.
pub fn become_menu_bar_app(_app: &mut tauri::App) {
    tracing::debug!("no activation policy to set on Windows");
}

/// Nothing to do: Windows has no deferred activation to settle. Foreground
/// ownership is decided per window as each one is shown, and a notification-area
/// menu is a popup of the tray window rather than the app's first window.
pub fn settle_launch_activation() {
    tracing::debug!("no launch activation to settle on Windows");
}

/// Nothing to do: `HWND_TOPMOST`, which is what Tauri's `always_on_top` sets,
/// already puts the window above every other application. Windows has
/// no equivalent of a full-screen Space to be excluded from.
///
/// The exception is a game holding exclusive fullscreen through DXGI, which
/// owns the display outright. Nothing a normal window does can appear over
/// that, so there is nothing to attempt here.
pub fn float_above_other_windows(_window: &tauri::WebviewWindow) {
    tracing::debug!("topmost already covers full-screen windows on Windows");
}
