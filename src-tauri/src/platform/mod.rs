//! Platform-specific behaviour, isolated so application logic never names an
//! operating system API (plan §22, §23.6).
//!
//! Application code calls the free functions and constants in this module, and
//! only those. The backend module is private, so `platform::macos::…` cannot be
//! named from outside — the rule is enforced by the compiler rather than by
//! convention.
//!
//! Adding a platform means adding one directory under `platform/` that supplies
//! every item forwarded to below, plus one line in the `cfg_attr` list. Nothing
//! outside this module should need to change. See
//! `docs/decisions/0002-cross-platform-platform-layer.md`.

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[cfg_attr(target_os = "macos", path = "macos/mod.rs")]
#[cfg_attr(target_os = "windows", path = "windows/mod.rs")]
mod backend;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
compile_error!(
    "LocalDictation supports macOS and Windows. Adding a platform means adding \
     src/platform/<os>/ supplying the backend contract, and one cfg_attr line in \
     src/platform/mod.rs — see docs/decisions/0002-cross-platform-platform-layer.md"
);

use tauri::AppHandle;

use crate::text_insertion::TextInserter;

/// Shortcuts shown beside tray menu items.
///
/// `None` where the platform has no such convention: a Windows notification-area
/// menu showing `Ctrl+Q` would advertise a shortcut nothing responds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayAccelerators {
    pub settings: Option<&'static str>,
    pub quit: Option<&'static str>,
}

/// A key press to synthesise into whichever application has focus.
///
/// Described in neutral terms — a `KeyboardEvent.code` name plus flags — so
/// this module stays free of the global-shortcut plugin's types and each
/// backend can map the name to its own virtual key codes.
// Four independent physical keys is what a modifier set *is*; collapsing them
// into a bitfield would only move the same four questions somewhere less clear.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keystroke {
    /// The `KeyboardEvent.code` name, e.g. `"KeyK"`, `"Space"`, `"F5"`.
    pub code: String,
    /// Command on macOS, the Windows key on Windows.
    pub meta: bool,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("no virtual key code for \"{0}\" on this platform")]
    UnmappedKey(String),
    #[error("could not synthesise the key press: {0}")]
    Synthesis(String),
}

/// The text inserter for this platform.
///
/// Takes a handle because the clipboard is reached through the clipboard
/// plugin, whose accessor is a [`tauri::Manager`] extension trait.
#[must_use]
pub fn text_inserter(app: AppHandle) -> Box<dyn TextInserter> {
    Box::new(backend::text::Inserter::new(app))
}

/// Press `keystroke` as though the user had, into whatever has focus now.
///
/// This exists for one purpose: a chord prefix is registered system-wide, so
/// the app that should have received it never did. When the chord is abandoned,
/// the prefix is handed back this way.
///
/// # Errors
///
/// [`ReplayError::UnmappedKey`] when the key has no virtual key code on this
/// platform, [`ReplayError::Synthesis`] when the OS refuses the event.
pub fn replay_keystroke(keystroke: &Keystroke) -> Result<(), ReplayError> {
    backend::replay::send(keystroke)
}

/// Hide the app from the Dock, taskbar, or app switcher, so it lives only in
/// the menu bar or notification area.
///
/// A no-op on platforms without the concept.
pub fn become_menu_bar_app(app: &mut tauri::App) {
    backend::become_menu_bar_app(app);
}

/// The hotkey a fresh install starts with.
///
/// Platform-specific because a combination that is free on one system is
/// reserved on another.
#[must_use]
pub const fn default_hotkey() -> &'static str {
    backend::DEFAULT_HOTKEY
}

/// Shortcuts to display in the tray menu.
#[must_use]
pub const fn tray_accelerators() -> TrayAccelerators {
    backend::TRAY_ACCELERATORS
}

/// Whether the tray icon should be rendered as a tintable template image.
#[must_use]
pub const fn tray_icon_is_template() -> bool {
    backend::TRAY_ICON_IS_TEMPLATE
}

/// Whether a left click on the tray icon should open the menu.
///
/// It should on macOS. On Windows the convention is that left click performs
/// the primary action and only right click opens the menu.
#[must_use]
pub const fn tray_menu_on_left_click() -> bool {
    backend::TRAY_MENU_ON_LEFT_CLICK
}

/// Open the system settings page where microphone access is granted.
///
/// Best effort: a failure to launch the settings app is logged, not surfaced,
/// because the message that offered the button already names the pane in words.
pub fn open_microphone_settings() {
    open(strings::MICROPHONE_SETTINGS_URL);
}

/// Open the system settings page where permission to synthesise input is
/// granted, if the platform has one.
///
/// Windows has no such page — it does not gate synthetic input behind a
/// permission — so this does nothing there.
pub fn open_input_permission_settings() {
    if let Some(url) = strings::INPUT_PERMISSION_SETTINGS_URL {
        open(url);
    }
}

fn open(url: &str) {
    if let Err(e) = tauri_plugin_opener::open_url(url, None::<&str>) {
        tracing::warn!(error = %e, "could not open the system settings page");
    }
}

/// User-facing wording that names a part of the operating system.
///
/// Errors in this app are written for a person and have to say *where* to fix
/// the problem — and that place is called something different on every
/// platform. Error types compose their messages from these rather than
/// hardcoding one platform's vocabulary.
pub mod strings {
    use super::backend;

    /// How a user of this platform writes the paste shortcut, e.g. `Cmd+V`.
    pub const PASTE_SHORTCUT: &str = backend::strings::PASTE_SHORTCUT;

    /// A two-step chord written the way this platform's users would, used when
    /// an error needs to show what a good one looks like.
    pub const EXAMPLE_CHORD: &str = backend::strings::EXAMPLE_CHORD;

    /// Where microphone access is granted, in words.
    pub const MICROPHONE_SETTINGS: &str = backend::strings::MICROPHONE_SETTINGS;

    /// The same page, as a URL the system will open.
    pub const MICROPHONE_SETTINGS_URL: &str = backend::strings::MICROPHONE_SETTINGS_URL;

    /// Where the user manages which apps start with the system, in words.
    pub const LOGIN_ITEMS_SETTINGS: &str = backend::strings::LOGIN_ITEMS_SETTINGS;

    /// Where permission to synthesise input is granted, or `None` where the
    /// platform has no such permission.
    pub const INPUT_PERMISSION_SETTINGS_URL: Option<&str> =
        backend::strings::INPUT_PERMISSION_SETTINGS_URL;

    /// The whole message shown when the OS refuses to let us synthesise input.
    ///
    /// A whole sentence rather than a place name, because the platforms do not
    /// fail for the same reason: macOS withholds a permission before anything
    /// happens, while Windows blocks input to a more privileged window after
    /// the text is already on the clipboard.
    pub const INSERT_PERMISSION_DENIED: &str = backend::strings::INSERT_PERMISSION_DENIED;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_hotkey_is_a_modifier_plus_a_key() {
        let accelerator = default_hotkey();
        assert!(
            accelerator.contains('+'),
            "a bare key would be swallowed system-wide: {accelerator}"
        );
    }

    #[test]
    fn the_default_hotkey_parses_as_an_accelerator() {
        // A default the OS cannot register would leave every fresh install
        // without a working hotkey.
        assert!(crate::hotkey::validate(default_hotkey()).is_ok());
    }

    #[test]
    fn platform_strings_are_present_and_finished() {
        for s in [
            strings::PASTE_SHORTCUT,
            strings::MICROPHONE_SETTINGS,
            strings::MICROPHONE_SETTINGS_URL,
            strings::LOGIN_ITEMS_SETTINGS,
            strings::INSERT_PERMISSION_DENIED,
        ] {
            assert!(!s.is_empty());
            assert!(!s.contains("TODO"), "unfinished platform string: {s}");
        }
    }

    #[test]
    fn the_paste_shortcut_names_the_v_key() {
        // Every platform's paste is <modifier>+V; only the modifier differs.
        assert!(strings::PASTE_SHORTCUT.ends_with('V'));
    }

    #[test]
    fn settings_urls_carry_a_scheme_the_system_can_open() {
        let urls = [Some(strings::MICROPHONE_SETTINGS_URL), strings::INPUT_PERMISSION_SETTINGS_URL];
        for url in urls.into_iter().flatten() {
            assert!(url.contains(':'), "not an openable URL: {url}");
        }
    }
}
