//! LocalDictation — fully local macOS dictation.
//!
//! Architecture (plan §4): the UI is a thin Svelte layer that renders state and
//! sends commands. Rust owns the state machine, audio, the ASR engine, and text
//! insertion. Nothing leaves the machine.

pub mod asr;
pub mod audio;
pub mod commands;
pub mod logging;
pub mod platform;
pub mod settings;
pub mod state;
pub mod tray;

use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tracing_appender::non_blocking::WorkerGuard;

use settings::Settings;
use state::{AppState, StateMachine, StateSnapshot};
use tray::TrayHandles;

/// Everything the backend owns, shared with commands via Tauri managed state.
pub struct AppContext {
    pub state: Mutex<StateMachine>,
    pub settings: Mutex<Settings>,
    /// `~/Library/Application Support/LocalDictation`
    pub data_dir: PathBuf,
    pub tray: Mutex<Option<TrayHandles>>,
    /// Owns the microphone on its own thread.
    pub audio: audio::AudioEngine,
    /// Kept alive for the process lifetime so buffered log lines are flushed.
    _log_guard: Option<WorkerGuard>,
}

/// Emitted to the UI whenever the authoritative state changes.
pub const EVENT_STATE_CHANGED: &str = "state_changed";

/// Move the app to `next`, then tell the menu bar and the UI about it.
///
/// Invalid transitions are logged and dropped rather than propagated: a stray
/// event should not take the app down.
pub fn set_app_state(app: &AppHandle, next: AppState) {
    let Some(ctx) = app.try_state::<AppContext>() else {
        return;
    };
    let snapshot = {
        let Ok(mut sm) = ctx.state.lock() else {
            tracing::error!("state lock poisoned");
            return;
        };
        match sm.transition_to(next) {
            Ok(snapshot) => snapshot,
            Err(e) => {
                tracing::warn!(error = %e, "ignored invalid state transition");
                return;
            }
        }
    };
    publish_state(app, &ctx, &snapshot);
}

/// Move the app into `Error` with a message written for the user.
pub fn fail_app_state(app: &AppHandle, message: impl Into<String>) {
    let Some(ctx) = app.try_state::<AppContext>() else {
        return;
    };
    let message = message.into();
    tracing::warn!(event = "app_error", message = %message);
    let snapshot = {
        let Ok(mut sm) = ctx.state.lock() else {
            return;
        };
        sm.fail(message)
    };
    publish_state(app, &ctx, &snapshot);
}

fn publish_state(app: &AppHandle, ctx: &AppContext, snapshot: &StateSnapshot) {
    if let Ok(guard) = ctx.tray.lock() {
        if let Some(handles) = guard.as_ref() {
            let _ = handles.status.set_text(tray::status_text(snapshot));
        }
    }
    if let Err(e) = app.emit(EVENT_STATE_CHANGED, snapshot) {
        tracing::warn!(error = %e, "could not emit state change");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::get_recording_state,
            commands::get_settings,
            commands::update_settings,
            commands::open_settings_window,
            commands::list_audio_devices,
            commands::start_microphone_test,
            commands::stop_microphone_test,
            commands::cancel_microphone_test,
            commands::quit_app,
        ])
        .on_window_event(|window, event| {
            // Closing the settings window must not quit a menu-bar app.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let log_dir = app.path().app_log_dir()?;
            let log_guard = logging::init(&log_dir);

            tracing::info!(
                event = "app_started",
                version = env!("CARGO_PKG_VERSION"),
                data_dir = %data_dir.display()
            );

            let settings_file = settings::settings_path(&data_dir);
            let first_run = !settings_file.exists();
            let settings = Settings::load(&settings_file);

            // Menu bar only: no Dock icon, no app menu.
            platform::become_menu_bar_app(app);

            let handles = tray::build(app)?;

            let state = StateMachine::new();
            let initial = state.snapshot();
            let _ = handles.status.set_text(tray::status_text(&initial));

            app.manage(AppContext {
                state: Mutex::new(state),
                settings: Mutex::new(settings),
                data_dir,
                tray: Mutex::new(Some(handles)),
                audio: audio::AudioEngine::spawn(),
                _log_guard: log_guard,
            });

            // A menu-bar app that shows nothing at all on first launch reads as
            // a failed install, so introduce ourselves once.
            if first_run {
                tracing::info!(event = "first_run");
                tray::show_settings_window(app.handle())?;
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running LocalDictation");
}
