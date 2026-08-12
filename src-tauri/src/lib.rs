//! LocalDictation — fully local macOS dictation.
//!
//! Architecture (plan §4): the UI is a thin Svelte layer that renders state and
//! sends commands. Rust owns the state machine, audio, the ASR engine, and text
//! insertion. Nothing leaves the machine.

pub mod asr;
pub mod audio;
pub mod commands;
pub mod dictation;
pub mod dictionary;
pub mod hotkey;
pub mod logging;
pub mod models;
pub mod platform;
pub mod settings;
pub mod state;
pub mod text_insertion;
pub mod tray;

use std::collections::HashMap;
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
    /// The loaded speech model, absent until one is installed and loaded.
    pub recognizer: Mutex<Option<Box<dyn asr::SpeechRecognizer>>>,
    /// System-wide shortcut registration.
    pub hotkeys: Box<dyn hotkey::GlobalHotkeys>,
    /// Installed models on disk.
    pub models: models::ModelStore,
    /// Cancel handles for downloads currently in flight, keyed by model id.
    pub downloads: Mutex<HashMap<String, models::download::CancelFlag>>,
    /// User replacements applied after transcription.
    pub dictionary: Mutex<dictionary::Dictionary>,
    /// Places the final text into the focused application.
    pub inserter: Box<dyn text_insertion::TextInserter>,
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

/// Build a recogniser for a model descriptor.
///
/// This is the one place that maps an engine to an implementation; a second
/// engine becomes another match arm rather than a change anywhere else.
fn build_recognizer(
    descriptor: &models::ModelDescriptor,
    model_dir: PathBuf,
) -> Box<dyn asr::SpeechRecognizer> {
    match descriptor.engine {
        models::EngineKind::Parakeet => Box::new(asr::parakeet::ParakeetRecognizer::new(
            descriptor.id,
            model_dir,
            descriptor.languages(),
            descriptor.capabilities.to_vec(),
        )),
    }
}

/// Load the configured model in the background, moving the app to `Ready` when
/// it succeeds. Does nothing if the model is not installed.
pub fn load_installed_model(app: &AppHandle) {
    let app = app.clone();
    std::thread::Builder::new()
        .name("model-load".into())
        .spawn(move || {
            let Some(ctx) = app.try_state::<AppContext>() else {
                return;
            };

            let model_id = match ctx.settings.lock() {
                Ok(s) => s.model_id.clone(),
                Err(_) => return,
            };

            let Some(descriptor) = models::find(&model_id) else {
                tracing::warn!(model_id = %model_id, "configured model is not in the registry");
                return;
            };

            if !ctx.models.is_installed(descriptor) {
                tracing::info!(event = "model_not_installed", model_id = %model_id);
                return;
            }

            let mut recognizer = build_recognizer(descriptor, ctx.models.dir_for(descriptor.id));
            match recognizer.load() {
                Ok(()) => {
                    if let Ok(mut guard) = ctx.recognizer.lock() {
                        *guard = Some(recognizer);
                    }
                    drop(ctx);
                    set_app_state(&app, AppState::Ready);
                }
                Err(e) => {
                    let message = dictation::asr_user_message(&e);
                    drop(ctx);
                    fail_app_state(&app, message);
                }
            }
        })
        .expect("could not start the model loading thread");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    // Only one shortcut is ever registered, so the identity of
                    // the shortcut does not matter — only the edge.
                    let edge = match event.state {
                        tauri_plugin_global_shortcut::ShortcutState::Pressed => {
                            hotkey::HotkeyEvent::Pressed
                        }
                        tauri_plugin_global_shortcut::ShortcutState::Released => {
                            hotkey::HotkeyEvent::Released
                        }
                    };
                    dictation::on_hotkey(app, edge);
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::get_recording_state,
            commands::get_settings,
            commands::update_settings,
            commands::open_settings_window,
            commands::list_audio_devices,
            commands::start_microphone_test,
            commands::stop_microphone_test,
            commands::cancel_microphone_test,
            commands::get_models,
            commands::download_model,
            commands::cancel_model_download,
            commands::delete_model,
            commands::get_dictionary,
            commands::add_dictionary_entry,
            commands::update_dictionary_entry,
            commands::delete_dictionary_entry,
            commands::can_insert_text,
            commands::request_insert_permission,
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
            let data_dir_for_store = data_dir.clone();
            let dictionary = dictionary::Dictionary::load(&dictionary::dictionary_path(&data_dir));

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
                recognizer: Mutex::new(None),
                hotkeys: Box::new(hotkey::TauriGlobalHotkeys::new(app.handle().clone())),
                models: models::ModelStore::new(&data_dir_for_store),
                downloads: Mutex::new(HashMap::new()),
                dictionary: Mutex::new(dictionary),
                inserter: platform::text_inserter(),
                _log_guard: log_guard,
            });

            // A shortcut another app already owns must not stop startup — the
            // app still runs, and Settings is where the user fixes it.
            let ctx = app.state::<AppContext>();
            let accelerator = ctx
                .settings
                .lock()
                .map(|s| s.hotkey.clone())
                .unwrap_or_else(|_| settings::DEFAULT_HOTKEY.to_string());

            match ctx.hotkeys.register(&accelerator) {
                Ok(()) => tracing::info!(event = "hotkey_registered", accelerator = %accelerator),
                Err(e) => {
                    tracing::warn!(event = "hotkey_registration_failed", error = %e);
                    let message = e.user_message();
                    drop(ctx);
                    fail_app_state(app.handle(), message);
                }
            }

            // Loading takes about a second and ~1.4 GB, so it happens off the
            // startup path. The app stays Uninitialized until it succeeds.
            load_installed_model(app.handle());

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
