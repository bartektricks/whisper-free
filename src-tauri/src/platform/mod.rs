//! Platform-specific behaviour, isolated so Windows and Linux can be added
//! later without touching application logic (plan §22, §23.6).
//!
//! Application code calls the free functions in this module. It never imports
//! anything from `platform::macos` directly.

#[cfg(target_os = "macos")]
pub mod macos;

/// Hide the app from the Dock and the app switcher, so it lives only in the
/// menu bar.
///
/// A no-op on platforms without the concept.
pub fn become_menu_bar_app(app: &mut tauri::App) {
    #[cfg(target_os = "macos")]
    macos::set_accessory_activation_policy(app);

    #[cfg(not(target_os = "macos"))]
    let _ = app;
}
