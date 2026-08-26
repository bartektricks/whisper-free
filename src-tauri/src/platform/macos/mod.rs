//! The macOS backend.
//!
//! Reached only as `platform::backend`; nothing outside `platform/` may name
//! this module. Every item here is part of the backend contract documented in
//! `platform/mod.rs`.

pub mod focus;
pub mod output;
pub mod permissions;
pub mod replay;
pub mod text;
pub mod window;

pub use window::float_above_other_windows;

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

/// macOS withholds synthetic input behind Accessibility, so the permission
/// is a step the user has to take before dictation can paste anything.
pub const INPUT_PERMISSION_REQUIRED: bool = true;

pub mod strings {
    pub const PASTE_SHORTCUT: &str = "Cmd+V";

    pub const EXAMPLE_CHORD: &str = "Cmd+K then C";

    pub const MICROPHONE_SETTINGS: &str = "System Settings › Privacy & Security › Microphone";
    pub const MICROPHONE_SETTINGS_URL: &str =
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";

    pub const LOGIN_ITEMS_SETTINGS: &str = "System Settings › General › Login Items";

    pub const INSTALL_LOCATION: &str = "the Applications folder";

    pub const INPUT_PERMISSION_SETTINGS_URL: Option<&str> =
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility");

    pub const INSERT_PERMISSION_DENIED: &str = "WhisperFree needs Accessibility permission to paste text. Grant it in System Settings › Privacy & Security › Accessibility, then try again.";
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

/// Settle the activation the windowing layer asks for while the app launches,
/// before anything is on screen for it to interrupt.
///
/// tao ends `applicationDidFinishLaunching` with
/// `activateIgnoringOtherApps:`. A menu-bar app has no window on screen at that
/// point, and macOS holds such a request until the app first displays one —
/// which here is the tray menu, roughly 250 ms after the user opens it. The
/// activation lands, and the menu shuts itself the first time it is opened in
/// every run of the app.
///
/// Asking a second time once the event loop is running settles it: measured
/// over five launches each, the first click keeps its menu, and no
/// `AXApplicationActivated` reaches the app at all — so this does not make the
/// app steal focus at startup, and the frontmost application is the same one
/// before and after launch.
///
/// The deprecated call is the one that works. `activate`, its replacement, was
/// measured over three launches and leaves the pending activation exactly where
/// it was, menu flash included.
pub fn settle_launch_activation() {
    // AppKit only from the main thread, and this is called from the event loop.
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        tracing::error!("launch activation can only be settled on the main thread");
        return;
    };

    #[allow(deprecated)]
    objc2_app_kit::NSApp(mtm).activateIgnoringOtherApps(true);
    tracing::debug!(event = "launch_activation_settled");
}
