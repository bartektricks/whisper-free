//! Menu-bar presence (plan §14).
//!
//! The tray menu is the app's primary surface: a status line, a way into
//! settings, and a way out.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{App, AppHandle, Manager, Wry};

use crate::state::{AppState, StateSnapshot};

pub const TRAY_ID: &str = "main";

/// Handles we keep so the status line can be rewritten as state changes.
pub struct TrayHandles {
    pub status: MenuItem<Wry>,
}

/// The text shown at the top of the tray menu for a given state.
#[must_use]
pub fn status_text(snapshot: &StateSnapshot) -> String {
    match snapshot.state {
        AppState::Uninitialized => "No model installed".to_string(),
        AppState::Ready => "Ready".to_string(),
        AppState::Recording => "Recording…".to_string(),
        AppState::Transcribing => "Transcribing…".to_string(),
        AppState::Inserting => "Inserting…".to_string(),
        AppState::Error => snapshot
            .message
            .clone()
            .unwrap_or_else(|| "Something went wrong".to_string()),
    }
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
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("Cmd+,"))?;
    let quit = MenuItem::with_id(app, "quit", "Quit LocalDictation", true, Some("Cmd+Q"))?;

    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
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

    match app.default_window_icon().cloned() {
        Some(icon) => {
            // Template rendering lets macOS tint the icon for light/dark menu bars.
            builder = builder.icon(icon).icon_as_template(true);
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

    Ok(TrayHandles { status })
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
