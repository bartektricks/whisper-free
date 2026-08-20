//! Menu-bar presence (plan §14).
//!
//! The tray menu is the app's primary surface: a status line, a way into
//! settings, and a way out.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Emitter, Manager, Wry};

use crate::platform;
use crate::state::{AppState, StateSnapshot};
use crate::update::{UpdatePhase, UpdateStatus};

pub const TRAY_ID: &str = "main";

/// Handles we keep so menu labels can be rewritten as state changes.
pub struct TrayHandles {
    pub status: MenuItem<Wry>,
    /// Both a label and a button: it reports where an update has got to, and
    /// clicking it does the next sensible thing for that phase.
    pub updates: MenuItem<Wry>,
}

/// The text shown at the top of the tray menu for a given state.
#[must_use]
pub fn status_text(snapshot: &StateSnapshot) -> String {
    match snapshot.state {
        AppState::Uninitialized => "No model installed".to_string(),
        AppState::Ready => "Ready".to_string(),
        AppState::Recording => "Recording…".to_string(),
        AppState::Transcribing => "Transcribing…".to_string(),
        AppState::Refining => "Checking the text…".to_string(),
        AppState::Inserting => "Inserting…".to_string(),
        AppState::Error => snapshot
            .message
            .clone()
            .unwrap_or_else(|| "Something went wrong".to_string()),
    }
}

/// The text shown on the updates item for a given update status.
///
/// This is the only place a menu-bar user learns an update exists, so it says
/// what is happening rather than staying a generic verb. The idle wording is
/// what the item shows for most of the app's life.
#[must_use]
pub fn update_text(status: &UpdateStatus) -> String {
    let version = status.version.as_deref().unwrap_or("");
    match status.phase {
        UpdatePhase::Idle | UpdatePhase::UpToDate | UpdatePhase::Failed => {
            "Check for Updates…".to_string()
        }
        UpdatePhase::Checking => "Checking for Updates…".to_string(),
        UpdatePhase::Available => format!("Update to {version}…"),
        // No announced length means no percentage: inventing one is worse
        // than a label that only says it is working.
        UpdatePhase::Downloading => percent(status).map_or_else(
            || "Downloading Update…".to_string(),
            |percent| format!("Downloading Update… {percent}%"),
        ),
        UpdatePhase::ReadyToRestart => "Restart to Update".to_string(),
    }
}

/// How far a download has got, or `None` when the server announced no length.
fn percent(status: &UpdateStatus) -> Option<u64> {
    if status.total_bytes == 0 {
        return None;
    }
    status
        .downloaded_bytes
        .saturating_mul(100)
        .checked_div(status.total_bytes)
        .map(|p| p.min(100))
}

/// Create the menu-bar item and its menu.
///
/// # Errors
///
/// Propagates the Tauri failure when a menu item, the menu, or the tray icon
/// cannot be created.
pub fn build(app: &App) -> tauri::Result<TrayHandles> {
    // Disabled: a label, not an action.
    let status = MenuItem::with_id(app, "status", "Starting…", false, None::<&str>)?;

    // Only platforms whose menus carry working shortcuts advertise them.
    let accelerators = platform::tray_accelerators();
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, accelerators.settings)?;
    // No accelerator on either platform: this is a rarely used item, and every
    // shortcut it could claim is one the focused app might want.
    let updates = MenuItem::with_id(
        app,
        "updates",
        update_text(&UpdateStatus::default()),
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        "Quit WhisperFree",
        true,
        accelerators.quit,
    )?;

    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &updates,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(platform::tray_menu_on_left_click())
        .on_menu_event(|app, event| match event.id.as_ref() {
            "updates" => on_updates_clicked(app),
            "settings" => {
                if let Err(e) = show_settings_window(app) {
                    tracing::error!(error = %e, "could not open settings window");
                }
            }
            "quit" => {
                tracing::info!(event = "app_quit_requested");
                app.exit(0);
            }
            other => tracing::debug!(id = other, "unhandled tray menu event"),
        });

    // Where the menu is not on the left button, that button has to do something
    // useful instead, or the icon looks dead to a click.
    if !platform::tray_menu_on_left_click() {
        builder = builder.on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Err(e) = show_settings_window(tray.app_handle()) {
                    tracing::error!(error = %e, "could not open settings window");
                }
            }
        });
    }

    match app.default_window_icon().cloned() {
        Some(icon) => {
            builder = builder
                .icon(icon)
                // Template rendering lets macOS tint the icon for light and
                // dark menu bars. Elsewhere the icon is drawn as supplied.
                .icon_as_template(platform::tray_icon_is_template());
            tracing::info!(event = "tray_icon_attached");
        }
        None => {
            // Without an icon the menu bar item is invisible and the app looks
            // like it failed to start, so this is worth shouting about.
            tracing::error!("no default window icon available; tray item will be blank");
        }
    }

    builder.build(app)?;
    tracing::info!(event = "tray_built");

    Ok(TrayHandles { status, updates })
}

/// What the updates item does, which depends on where the update has got to.
///
/// Both branches open Settings › Updates, because everything past "one is
/// available" — the notes, the progress, the failure — needs more room than a
/// menu label. Nothing here blocks: `update::check` and `update::install`
/// return immediately, which they must, since on macOS this handler runs on
/// the main thread and the install path wants that thread for its own prompt.
fn on_updates_clicked(app: &AppHandle) {
    let phase = app
        .try_state::<crate::AppContext>()
        .map_or(UpdatePhase::Idle, |ctx| crate::update::status(&ctx).phase);

    match phase {
        // Already offered, downloaded or in flight: show the panel rather than
        // starting a second request behind the first.
        UpdatePhase::Available
        | UpdatePhase::Downloading
        | UpdatePhase::ReadyToRestart
        | UpdatePhase::Checking => {}
        UpdatePhase::Idle | UpdatePhase::UpToDate | UpdatePhase::Failed => {
            crate::update::check(app, crate::update::Trigger::Manual);
        }
    }

    if let Err(e) = show_settings_window(app) {
        tracing::error!(error = %e, "could not open settings window");
    }
    // Sent after the window is up so a webview that was only just created has
    // its listener attached; the panel is idempotent about being told twice.
    if let Err(e) = app.emit(crate::EVENT_SHOW_SECTION, "updates") {
        tracing::warn!(error = %e, "could not focus the updates section");
    }
}

/// Show the settings window, creating focus even though the app is an
/// Accessory (Dock-less) process.
///
/// # Errors
///
/// Propagates the Tauri failure when the window cannot be shown or focused. A
/// window missing from the app config is logged and treated as a no-op.
pub fn show_settings_window(app: &AppHandle) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window("settings") else {
        tracing::error!("settings window is missing from the app config");
        return Ok(());
    };
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    tracing::info!(event = "settings_window_shown", visible = window.is_visible()?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn downloading(downloaded: u64, total: u64) -> UpdateStatus {
        UpdateStatus {
            phase: UpdatePhase::Downloading,
            version: Some("0.2.0".into()),
            downloaded_bytes: downloaded,
            total_bytes: total,
            ..UpdateStatus::default()
        }
    }

    #[test]
    fn the_resting_label_invites_a_check() {
        // What the item reads for almost all of the app's life.
        for phase in [
            UpdatePhase::Idle,
            UpdatePhase::UpToDate,
            UpdatePhase::Failed,
        ] {
            let status = UpdateStatus {
                phase,
                ..UpdateStatus::default()
            };
            assert_eq!(update_text(&status), "Check for Updates…");
        }
    }

    #[test]
    fn an_offer_names_the_version_so_the_menu_alone_is_enough() {
        let status = UpdateStatus {
            phase: UpdatePhase::Available,
            version: Some("0.2.0".into()),
            ..UpdateStatus::default()
        };
        assert_eq!(update_text(&status), "Update to 0.2.0…");
    }

    #[test]
    fn a_download_of_known_length_shows_how_far_it_has_got() {
        assert_eq!(
            update_text(&downloading(25, 100)),
            "Downloading Update… 25%"
        );
    }

    #[test]
    fn a_download_of_unknown_length_invents_no_percentage() {
        // The server need not announce a length, and a bar reading 0% while
        // bytes arrive is worse than one that only says it is working.
        let text = update_text(&downloading(5_000_000, 0));
        assert_eq!(text, "Downloading Update…");
        assert!(!text.contains('%'));
    }

    #[test]
    fn a_download_never_reports_past_the_end() {
        // The announced length is the server's claim, not a guarantee.
        assert_eq!(
            update_text(&downloading(150, 100)),
            "Downloading Update… 100%"
        );
    }

    #[test]
    fn an_installed_update_asks_for_the_restart_rather_than_taking_it() {
        let status = UpdateStatus {
            phase: UpdatePhase::ReadyToRestart,
            version: Some("0.2.0".into()),
            ..UpdateStatus::default()
        };
        assert_eq!(update_text(&status), "Restart to Update");
    }
}
