//! Tauri commands — the only way the UI talks to the backend (plan §16).

use tauri::{AppHandle, Manager, State};

use crate::audio::AudioDevice;
use crate::dictionary::{Dictionary, DictionaryEntry};
use crate::models::download::CancelFlag;
use crate::models::{ModelError, ModelInfo};
use crate::settings::Settings;
use crate::state::StateSnapshot;
use crate::{tray, AppContext};

/// Errors crossing into the UI are plain strings written for a person to read
/// (plan §17). Technical detail goes to the log, not to the user.
pub type CommandResult<T> = Result<T, String>;

#[tauri::command]
pub fn get_recording_state(ctx: State<'_, AppContext>) -> CommandResult<StateSnapshot> {
    Ok(ctx.state.lock().map_err(lock_err)?.snapshot())
}

#[tauri::command]
pub fn get_settings(ctx: State<'_, AppContext>) -> CommandResult<Settings> {
    Ok(ctx.settings.lock().map_err(lock_err)?.clone())
}

#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    ctx: State<'_, AppContext>,
    settings: Settings,
) -> CommandResult<Settings> {
    // The shortcut has to be claimed from the OS before it is persisted —
    // otherwise a taken shortcut would be saved and silently do nothing.
    let previous_hotkey = ctx.settings.lock().map_err(lock_err)?.hotkey.clone();
    if settings.hotkey != previous_hotkey {
        crate::hotkey::validate(&settings.hotkey).map_err(|e| e.user_message())?;
        if let Err(e) = ctx.hotkeys.register(&settings.hotkey) {
            tracing::warn!(event = "hotkey_registration_failed", error = %e);
            // Put the working shortcut back so the user is not left with none.
            let _ = ctx.hotkeys.register(&previous_hotkey);
            return Err(e.user_message());
        }
        tracing::info!(event = "hotkey_registered", accelerator = %settings.hotkey);
    }

    let path = crate::settings::settings_path(&ctx.data_dir);
    settings.save(&path).map_err(|e| {
        tracing::error!(error = %e, "could not save settings");
        "Could not save your settings. Check that the app has permission to write to its data folder.".to_string()
    })?;

    {
        let mut guard = ctx.settings.lock().map_err(lock_err)?;
        *guard = settings.clone();
    }

    tracing::info!(event = "settings_updated");
    let _ = app.emit_to_all("settings_changed", &settings);
    Ok(settings)
}

#[tauri::command]
pub fn open_settings_window(app: AppHandle) -> CommandResult<()> {
    tray::show_settings_window(&app).map_err(|e| {
        tracing::error!(error = %e, "could not open settings window");
        "Could not open the settings window.".to_string()
    })
}

#[tauri::command]
pub fn list_audio_devices(ctx: State<'_, AppContext>) -> CommandResult<Vec<AudioDevice>> {
    ctx.audio.list_devices().map_err(|e| {
        tracing::warn!(error = %e, "could not list microphones");
        e.user_message()
    })
}

/// What a microphone test produced. Levels and durations only — the audio
/// itself is discarded (plan §9, §18).
#[derive(Debug, Clone, serde::Serialize)]
pub struct MicrophoneTest {
    pub duration_ms: u64,
    /// Loudest sample in the take, 0.0–1.0.
    pub peak_level: f32,
    /// False when the take was silent, which usually means permission was
    /// granted in name only or the wrong device is selected.
    pub heard_audio: bool,
}

#[tauri::command]
pub fn start_microphone_test(ctx: State<'_, AppContext>) -> CommandResult<()> {
    let device = ctx
        .settings
        .lock()
        .map_err(lock_err)?
        .input_device
        .clone();

    ctx.audio.start(device).map_err(|e| {
        tracing::warn!(error = %e, "microphone test could not start");
        e.user_message()
    })
}

#[tauri::command]
pub fn stop_microphone_test(ctx: State<'_, AppContext>) -> CommandResult<MicrophoneTest> {
    let buffer = ctx.audio.stop().map_err(|e| {
        tracing::warn!(error = %e, "microphone test could not finish");
        e.user_message()
    })?;

    let peak = buffer
        .samples
        .iter()
        .fold(0.0f32, |max, s| max.max(s.abs()));

    // macOS hands back digital silence when microphone access is refused, so a
    // flat take is worth calling out rather than reporting as success.
    let heard_audio = peak > 0.0005;

    tracing::info!(
        event = "microphone_test_finished",
        duration_ms = buffer.duration().as_millis() as u64,
        heard_audio
    );

    Ok(MicrophoneTest {
        duration_ms: buffer.duration().as_millis() as u64,
        peak_level: peak,
        heard_audio,
    })
}

#[tauri::command]
pub fn cancel_microphone_test(ctx: State<'_, AppContext>) -> CommandResult<()> {
    ctx.audio.cancel().map_err(|e| e.user_message())
}

#[tauri::command]
pub fn get_models(ctx: State<'_, AppContext>) -> CommandResult<Vec<ModelInfo>> {
    Ok(ctx.models.list())
}

/// Start downloading a model. Returns immediately; progress arrives as
/// `model_download_progress` events (plan §16).
#[tauri::command]
pub fn download_model(
    app: AppHandle,
    ctx: State<'_, AppContext>,
    model_id: String,
) -> CommandResult<()> {
    let descriptor = crate::models::find(&model_id)
        .ok_or_else(|| ModelError::Unknown(model_id.clone()).user_message())?;

    let cancel = CancelFlag::new();
    {
        let mut downloads = ctx.downloads.lock().map_err(lock_err)?;
        if downloads.contains_key(&model_id) {
            // Already running — a double click should not start a second one.
            return Ok(());
        }
        downloads.insert(model_id.clone(), cancel.clone());
    }

    let store = ctx.models.clone();
    let app_for_thread = app.clone();

    std::thread::Builder::new()
        .name("model-download".into())
        .spawn(move || {
            use tauri::Emitter;

            let result = crate::models::download::install(
                &store,
                descriptor,
                &cancel,
                |progress| {
                    let _ = app_for_thread.emit("model_download_progress", &progress);
                },
            );

            if let Some(ctx) = app_for_thread.try_state::<AppContext>() {
                if let Ok(mut downloads) = ctx.downloads.lock() {
                    downloads.remove(descriptor.id);
                }
            }

            match result {
                Ok(()) => {
                    let _ = app_for_thread.emit("model_download_completed", descriptor.id);
                    // The model is on disk now, so the app can become usable.
                    crate::load_installed_model(&app_for_thread);
                }
                Err(e) => {
                    tracing::warn!(event = "model_download_failed", error = %e);
                    let _ = app_for_thread.emit(
                        "model_download_failed",
                        serde_json::json!({
                            "model_id": descriptor.id,
                            "message": e.user_message(),
                        }),
                    );
                }
            }
        })
        .map_err(|e| {
            tracing::error!(error = %e, "could not start the download thread");
            "The download could not be started.".to_string()
        })?;

    Ok(())
}

#[tauri::command]
pub fn cancel_model_download(ctx: State<'_, AppContext>, model_id: String) -> CommandResult<()> {
    if let Some(flag) = ctx.downloads.lock().map_err(lock_err)?.get(&model_id) {
        flag.cancel();
    }
    Ok(())
}

#[tauri::command]
pub fn delete_model(
    app: AppHandle,
    ctx: State<'_, AppContext>,
    model_id: String,
) -> CommandResult<()> {
    // Drop the loaded copy first: deleting files out from under a live ONNX
    // session is asking for trouble.
    let was_loaded = {
        let mut guard = ctx.recognizer.lock().map_err(lock_err)?;
        let loaded_this = guard
            .as_ref()
            .map(|r| r.model_id() == model_id)
            .unwrap_or(false);
        if loaded_this {
            if let Some(r) = guard.as_mut() {
                r.unload();
            }
            *guard = None;
        }
        loaded_this
    };

    ctx.models
        .remove(&model_id)
        .map_err(|e| e.user_message())?;

    if was_loaded {
        // Dictation is no longer possible until another model is installed.
        crate::set_app_state(&app, crate::state::AppState::Uninitialized);
    }
    Ok(())
}

#[tauri::command]
pub fn get_dictionary(ctx: State<'_, AppContext>) -> CommandResult<Vec<DictionaryEntry>> {
    Ok(ctx.dictionary.lock().map_err(lock_err)?.entries.clone())
}

#[tauri::command]
pub fn add_dictionary_entry(
    ctx: State<'_, AppContext>,
    input: String,
    replacement: String,
) -> CommandResult<Vec<DictionaryEntry>> {
    let mut dictionary = ctx.dictionary.lock().map_err(lock_err)?;
    dictionary
        .add(&input, &replacement)
        .map_err(|e| e.user_message())?;
    persist_dictionary(&ctx, &dictionary)?;
    Ok(dictionary.entries.clone())
}

#[tauri::command]
pub fn update_dictionary_entry(
    ctx: State<'_, AppContext>,
    id: u64,
    input: String,
    replacement: String,
    enabled: bool,
) -> CommandResult<Vec<DictionaryEntry>> {
    let mut dictionary = ctx.dictionary.lock().map_err(lock_err)?;
    dictionary
        .update(id, &input, &replacement, enabled)
        .map_err(|e| e.user_message())?;
    persist_dictionary(&ctx, &dictionary)?;
    Ok(dictionary.entries.clone())
}

#[tauri::command]
pub fn delete_dictionary_entry(
    ctx: State<'_, AppContext>,
    id: u64,
) -> CommandResult<Vec<DictionaryEntry>> {
    let mut dictionary = ctx.dictionary.lock().map_err(lock_err)?;
    dictionary.remove(id).map_err(|e| e.user_message())?;
    persist_dictionary(&ctx, &dictionary)?;
    Ok(dictionary.entries.clone())
}

fn persist_dictionary(ctx: &AppContext, dictionary: &Dictionary) -> CommandResult<()> {
    let path = crate::dictionary::dictionary_path(&ctx.data_dir);
    dictionary.save(&path).map_err(|e| {
        tracing::error!(error = %e, "could not save the dictionary");
        e.user_message()
    })
}

/// Whether the OS will let us paste into other applications.
#[tauri::command]
pub fn can_insert_text(ctx: State<'_, AppContext>) -> CommandResult<bool> {
    Ok(ctx.inserter.can_insert())
}

/// Open the Accessibility settings pane so the user can grant permission.
#[tauri::command]
pub fn request_insert_permission(ctx: State<'_, AppContext>) -> CommandResult<()> {
    ctx.inserter.request_permission();
    Ok(())
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    tracing::info!(event = "app_quit_requested");
    app.exit(0);
}

fn lock_err<T>(_: T) -> String {
    tracing::error!("application state lock was poisoned");
    "The application hit an internal error. Restarting it should clear this.".to_string()
}

/// Small extension so commands can emit without importing `Emitter` everywhere.
trait EmitToAll {
    fn emit_to_all<S: serde::Serialize + Clone>(&self, event: &str, payload: S)
        -> tauri::Result<()>;
}

impl EmitToAll for AppHandle {
    fn emit_to_all<S: serde::Serialize + Clone>(
        &self,
        event: &str,
        payload: S,
    ) -> tauri::Result<()> {
        use tauri::Emitter;
        self.emit(event, payload)
    }
}
