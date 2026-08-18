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

use tauri::{AppHandle, Monitor, PhysicalPosition, PhysicalSize};

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

/// Raise an always-on-top window above other floating windows, and let it
/// follow the user between Spaces.
///
/// "Always on top" is not the same claim on both platforms. On Windows it means
/// what it says. On macOS it means `NSFloatingWindowLevel`, which merely *ties*
/// with every other floating window — another app's picture-in-picture will
/// cover it — so the level and the Spaces behaviour have to be set explicitly.
///
/// It does **not** get a window into another application's full-screen Space on
/// macOS; that needs an `NSPanel`, and no window level achieves it. See
/// `docs/decisions/0004-dictation-overlay.md`.
pub fn float_above_other_windows(window: &tauri::WebviewWindow) {
    backend::float_above_other_windows(window);
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

/// One monitor's geometry, in the units Tauri reports it in.
///
/// A plain copy of what [`tauri::Monitor`] carries, so the monitor-picking
/// logic below is a pure function over data a test can build.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorBounds {
    pub position: PhysicalPosition<i32>,
    pub size: PhysicalSize<u32>,
    pub scale: f64,
}

impl From<&Monitor> for MonitorBounds {
    fn from(monitor: &Monitor) -> Self {
        Self {
            position: *monitor.position(),
            size: *monitor.size(),
            scale: monitor.scale_factor(),
        }
    }
}

/// Which of `monitors` the user is working on, as an index into the slice.
///
/// The display holding the focused window, falling back to the display under
/// the pointer — in that order, because the focused window is where dictated
/// text is about to be pasted, and the pointer is often left on another screen.
///
/// `None` when neither can be resolved: nothing is focused, the platform
/// refuses to say, or the answer lands outside every known monitor. Callers
/// choose their own fallback rather than being handed a guess.
///
/// Deliberately *not* `AppHandle::monitor_from_point`: on macOS that compares
/// against `CGDisplayBounds`, which is in points, while `cursor_position`
/// returns points multiplied by the primary monitor's scale factor, so the
/// lookup misses on any Retina display and silently falls back to the primary.
#[must_use]
pub fn active_monitor(monitors: &[MonitorBounds]) -> Option<usize> {
    backend::focus::active_monitor(monitors)
}

/// The units a platform reports window and pointer positions in.
///
/// Not `pub`: only the backends name it, when they say which space the point
/// they just measured is in. Each backend names exactly one, so on any single
/// platform the other variant is only ever constructed by the tests below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
enum ScreenUnit {
    /// The same units as [`MonitorBounds`] — Windows measures the virtual
    /// desktop in physical pixels throughout.
    Physical,
    /// Points. macOS reports every global coordinate in points, while
    /// [`MonitorBounds`] has already been multiplied by each monitor's scale
    /// factor, so the monitor is what has to be converted.
    Logical,
}

/// The first monitor whose bounds contain the point `(x, y)`.
///
/// Pure, so every layout that has ever placed the overlay on the wrong screen
/// can be written down as a test — the same split as [`crate::state::is_valid`]
/// and [`crate::hotkey::decide`].
fn monitor_containing(
    monitors: &[MonitorBounds],
    x: f64,
    y: f64,
    unit: ScreenUnit,
) -> Option<usize> {
    monitors.iter().position(|monitor| {
        // A monitor reporting a nonsensical scale factor is measured as it is
        // rather than dividing by zero.
        let scale = if unit == ScreenUnit::Logical && monitor.scale > 0.0 {
            monitor.scale
        } else {
            1.0
        };

        let left = f64::from(monitor.position.x) / scale;
        let top = f64::from(monitor.position.y) / scale;
        let right = left + f64::from(monitor.size.width) / scale;
        let bottom = top + f64::from(monitor.size.height) / scale;

        x >= left && x < right && y >= top && y < bottom
    })
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

    /// A 1x display at the origin, as Windows reports one.
    fn plain(x: i32, y: i32, width: u32, height: u32) -> MonitorBounds {
        MonitorBounds {
            position: PhysicalPosition::new(x, y),
            size: PhysicalSize::new(width, height),
            scale: 1.0,
        }
    }

    /// A 2x display: Tauri multiplies both the origin and the size by the
    /// scale factor, while macOS reports window positions in points.
    fn retina(x_points: i32, y_points: i32, width_points: u32, height_points: u32) -> MonitorBounds {
        MonitorBounds {
            position: PhysicalPosition::new(x_points * 2, y_points * 2),
            size: PhysicalSize::new(width_points * 2, height_points * 2),
            scale: 2.0,
        }
    }

    #[test]
    fn a_physical_point_picks_the_monitor_it_falls_inside() {
        let monitors = [plain(0, 0, 1920, 1080), plain(1920, 0, 2560, 1440)];
        assert_eq!(monitor_containing(&monitors, 100.0, 100.0, ScreenUnit::Physical), Some(0));
        assert_eq!(monitor_containing(&monitors, 3000.0, 900.0, ScreenUnit::Physical), Some(1));
    }

    /// The regression this whole thing exists for: on a Retina display a point
    /// measured in points must not be compared with a bound measured in
    /// pixels, or every window past the halfway mark lands on the wrong screen.
    #[test]
    fn a_point_on_a_retina_display_is_not_compared_with_pixels() {
        let monitors = [retina(0, 0, 1512, 982), plain(1512, 0, 2560, 1440)];

        // Bottom-right corner of the built-in display, in points.
        assert_eq!(monitor_containing(&monitors, 1400.0, 900.0, ScreenUnit::Logical), Some(0));
        // Comparing that same point against pixels would have found the
        // external display, since 1400 is past the built-in's 1512-point width
        // once doubled.
        assert_eq!(monitor_containing(&monitors, 2000.0, 900.0, ScreenUnit::Logical), Some(1));
    }

    /// A display to the left of the primary has a negative origin.
    #[test]
    fn a_monitor_at_a_negative_origin_is_found() {
        let monitors = [plain(0, 0, 1920, 1080), plain(-2560, -100, 2560, 1440)];
        assert_eq!(monitor_containing(&monitors, -1000.0, 500.0, ScreenUnit::Physical), Some(1));
    }

    /// A minimised window on Windows reports a rectangle far off the desktop,
    /// and a window can be dragged partly off it. Neither may be forced onto a
    /// monitor: the caller has a fallback and a guess would defeat it.
    #[test]
    fn a_point_outside_every_monitor_belongs_to_none() {
        let monitors = [plain(0, 0, 1920, 1080)];
        assert_eq!(monitor_containing(&monitors, -32000.0, -32000.0, ScreenUnit::Physical), None);
        assert_eq!(monitor_containing(&monitors, 1920.0, 500.0, ScreenUnit::Physical), None);
    }

    #[test]
    fn no_monitors_at_all_is_not_a_panic() {
        assert_eq!(monitor_containing(&[], 0.0, 0.0, ScreenUnit::Physical), None);
    }

    #[test]
    fn a_monitor_reporting_no_scale_factor_is_measured_as_it_is() {
        let broken = MonitorBounds {
            position: PhysicalPosition::new(0, 0),
            size: PhysicalSize::new(1920, 1080),
            scale: 0.0,
        };
        assert_eq!(monitor_containing(&[broken], 100.0, 100.0, ScreenUnit::Logical), Some(0));
    }

    #[test]
    fn settings_urls_carry_a_scheme_the_system_can_open() {
        let urls = [Some(strings::MICROPHONE_SETTINGS_URL), strings::INPUT_PERMISSION_SETTINGS_URL];
        for url in urls.into_iter().flatten() {
            assert!(url.contains(':'), "not an openable URL: {url}");
        }
    }
}
