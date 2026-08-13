//! The macOS backend.
//!
//! Reached only as `platform::backend`; nothing outside `platform/` may name
//! this module. Every item here is part of the backend contract documented in
//! `platform/mod.rs`.

pub mod replay;
pub mod text;

use super::TrayAccelerators;

/// `Option+Space`, written in the accelerator syntax Tauri understands.
pub const DEFAULT_HOTKEY: &str = "Alt+Space";

/// macOS shows working shortcuts in menu-bar menus, and these two are the ones
/// a Mac user will try without being told.
pub const TRAY_ACCELERATORS: TrayAccelerators = TrayAccelerators {
    settings: Some("Cmd+,"),
    quit: Some("Cmd+Q"),
};

/// Template rendering lets macOS tint the icon for light and dark menu bars.
pub const TRAY_ICON_IS_TEMPLATE: bool = true;

/// Menu-bar items open their menu on either button.
pub const TRAY_MENU_ON_LEFT_CLICK: bool = true;

pub mod strings {
    pub const PASTE_SHORTCUT: &str = "Cmd+V";

    pub const EXAMPLE_CHORD: &str = "Cmd+K then C";

    pub const MICROPHONE_SETTINGS: &str = "System Settings › Privacy & Security › Microphone";
    pub const MICROPHONE_SETTINGS_URL: &str =
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";

    pub const LOGIN_ITEMS_SETTINGS: &str = "System Settings › General › Login Items";

    pub const INPUT_PERMISSION_SETTINGS_URL: Option<&str> =
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility");

    pub const INSERT_PERMISSION_DENIED: &str = "LocalDictation needs Accessibility permission to paste text. Grant it in System Settings › Privacy & Security › Accessibility, then try again.";
}

/// Switch the process to `NSApplicationActivationPolicyAccessory`.
///
/// An Accessory app has no Dock icon and no application menu bar, which is what
/// makes this feel like a menu-bar utility rather than a windowed app. It can
/// still open windows and receive keyboard input.
pub fn become_menu_bar_app(app: &mut tauri::App) {
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    tracing::debug!("activation policy set to accessory");
}
