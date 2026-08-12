//! macOS-specific platform glue.

pub mod text;

/// Switch the process to `NSApplicationActivationPolicyAccessory`.
///
/// An Accessory app has no Dock icon and no application menu bar, which is what
/// makes this feel like a menu-bar utility rather than a windowed app. It can
/// still open windows and receive keyboard input.
pub fn set_accessory_activation_policy(app: &mut tauri::App) {
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    tracing::debug!("activation policy set to accessory");
}
