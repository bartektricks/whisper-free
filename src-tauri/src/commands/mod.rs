//! Tauri commands — the only way the UI talks to the backend (plan §16).
//!
//! `#[tauri::command]` fixes these signatures: every argument has to be an
//! owned value the macro can deserialise or inject (`State`, `AppHandle`,
//! `String`), so taking them by reference is not an option here.
#![allow(clippy::needless_pass_by_value)]

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

/// # Errors
///
/// When the state lock is poisoned.
#[tauri::command]
pub fn get_recording_state(ctx: State<'_, AppContext>) -> CommandResult<StateSnapshot> {
    Ok(ctx.state.lock().map_err(lock_err)?.snapshot())
}

/// # Errors
///
/// When the settings lock is poisoned.
#[tauri::command]
pub fn get_settings(ctx: State<'_, AppContext>) -> CommandResult<Settings> {
    Ok(ctx.settings.lock().map_err(lock_err)?.clone())
}

/// Validate, apply, and persist new settings.
///
/// # Errors
///
/// When the shortcut is invalid or already taken, when the login item cannot
/// be changed, or when the settings file cannot be written.
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
        // Parsing is the validation: it is what rejects a malformed chord,
        // which registering the first step alone would happily accept.
        let chord = crate::hotkey::Chord::parse(&settings.hotkey).map_err(|e| e.user_message())?;

        // Only the first step is claimed here. A chord's second step is
        // registered for the moment the window is open — see `dictation`.
        if let Err(e) = ctx.hotkeys.register(&chord.prefix.accelerator) {
            tracing::warn!(event = "hotkey_registration_failed", error = %e);
            // Put the working shortcut back so the user is not left with none.
            if let Ok(previous) = crate::hotkey::Chord::parse(&previous_hotkey) {
                let _ = ctx.hotkeys.register(&previous.prefix.accelerator);
            }
            return Err(e.user_message());
        }

        // Only now that the OS has accepted it: an earlier swap would leave the
        // chord machinery describing a shortcut that is not registered.
        ctx.chord.lock().map_err(lock_err)?.set(chord);
        tracing::info!(event = "hotkey_registered", accelerator = %settings.hotkey);
    }

    // Same reasoning as the hotkey: only persist what the OS actually accepted.
    let previous_autostart = ctx.settings.lock().map_err(lock_err)?.start_at_login;
    if settings.start_at_login != previous_autostart {
        apply_start_at_login(&app, settings.start_at_login)?;
    }

    let overlay_changed = {
        let current = ctx.settings.lock().map_err(lock_err)?;
        current.show_overlay != settings.show_overlay
            || current.overlay_anchor != settings.overlay_anchor
    };

    let path = crate::settings::settings_path(&ctx.data_dir);
    settings.save(&path).map_err(|e| {
        tracing::error!(error = %e, "could not save settings");
        "Could not save your settings. Check that the app has permission to write to its data folder.".to_string()
    })?;

    {
        let mut guard = ctx.settings.lock().map_err(lock_err)?;
        *guard = settings.clone();
    }

    // Only when it actually changed: re-applying on every save would pop the
    // overlay back up for an unrelated edit made while the app is in `Error`.
    // The settings guard above is out of scope by here, which matters because
    // `overlay::apply` takes that same lock.
    if overlay_changed {
        let snapshot = ctx.state.lock().map_err(lock_err)?.snapshot();
        crate::overlay::apply(&app, ctx.inner(), &snapshot);
    }

    tracing::info!(event = "settings_updated");
    let _ = app.emit_to_all("settings_changed", &settings);
    Ok(settings)
}

/// Release the hotkey while the user records a new one.
///
/// A registered shortcut is claimed system-wide, including from our own
/// windows, so the recorder in Settings would never see the very combination
/// the user is trying to replace — pressing it would start a dictation instead.
/// That was already true of a plain hotkey and is unavoidable for a chord,
/// whose prefix is the thing being re-recorded.
///
/// # Errors
///
/// When the OS refuses to release the shortcut.
#[tauri::command]
pub fn suspend_hotkey(ctx: State<'_, AppContext>) -> CommandResult<()> {
    ctx.hotkeys.unregister_all().map_err(|e| e.user_message())?;
    // Any chord window open against the registration just dropped is now
    // describing shortcuts nobody holds.
    ctx.chord.lock().map_err(lock_err)?.close();
    tracing::debug!(event = "hotkey_suspended");
    Ok(())
}

/// Claim the hotkey again once recording finishes.
///
/// Registers whatever the live hotkey is now — the newly chosen one if it was
/// accepted, the previous one if it was not — so the user is never left with
/// no shortcut at all.
///
/// # Errors
///
/// When the shortcut cannot be registered again.
#[tauri::command]
pub fn resume_hotkey(ctx: State<'_, AppContext>) -> CommandResult<()> {
    let accelerator = ctx
        .chord
        .lock()
        .map_err(lock_err)?
        .chord()
        .prefix
        .accelerator
        .clone();

    ctx.hotkeys.register(&accelerator).map_err(|e| {
        tracing::warn!(event = "hotkey_resume_failed", error = %e);
        e.user_message()
    })?;
    tracing::debug!(event = "hotkey_resumed", accelerator = %accelerator);
    Ok(())
}

/// # Errors
///
/// When the settings window cannot be shown or focused.
#[tauri::command]
pub fn open_settings_window(app: AppHandle) -> CommandResult<()> {
    tray::show_settings_window(&app).map_err(|e| {
        tracing::error!(error = %e, "could not open settings window");
        "Could not open the settings window.".to_string()
    })
}

/// # Errors
///
/// When no microphone is present or the host cannot be queried.
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

/// # Errors
///
/// When the settings lock is poisoned, or the microphone cannot be opened.
#[tauri::command]
pub fn start_microphone_test(ctx: State<'_, AppContext>) -> CommandResult<()> {
    let device = ctx.settings.lock().map_err(lock_err)?.input_device.clone();

    ctx.audio.start(device).map_err(|e| {
        tracing::warn!(error = %e, "microphone test could not start");
        e.user_message()
    })
}

/// # Errors
///
/// When nothing was being recorded, or the take could not be converted.
#[tauri::command]
pub fn stop_microphone_test(ctx: State<'_, AppContext>) -> CommandResult<MicrophoneTest> {
    let buffer = ctx.audio.stop().map_err(|e| {
        tracing::warn!(error = %e, "microphone test could not finish");
        e.user_message()
    })?;

    let peak = buffer.samples.iter().fold(0.0f32, |max, s| max.max(s.abs()));

    // macOS hands back digital silence when microphone access is refused, so a
    // flat take is worth calling out rather than reporting as success.
    let heard_audio = peak > 0.0005;
    let duration_ms = crate::millis(buffer.duration());

    tracing::info!(
        event = "microphone_test_finished",
        duration_ms,
        heard_audio
    );

    Ok(MicrophoneTest {
        duration_ms,
        peak_level: peak,
        heard_audio,
    })
}

/// # Errors
///
/// When the audio thread is no longer running.
#[tauri::command]
pub fn cancel_microphone_test(ctx: State<'_, AppContext>) -> CommandResult<()> {
    ctx.audio.cancel().map_err(|e| e.user_message())
}

/// # Errors
///
/// Never; the signature matches the other commands so the UI can treat them
/// alike.
#[tauri::command]
pub fn get_models(ctx: State<'_, AppContext>) -> CommandResult<Vec<ModelInfo>> {
    Ok(ctx.models.list())
}

/// Start downloading a model. Returns immediately; progress arrives as
/// `model_download_progress` events (plan §16).
///
/// # Errors
///
/// When the model id is unknown, the downloads lock is poisoned, or the worker
/// thread cannot be started. Failures during the download itself arrive as
/// `model_download_failed` events instead.
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
    let app_for_thread = app;

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

/// # Errors
///
/// When the downloads lock is poisoned.
#[tauri::command]
pub fn cancel_model_download(ctx: State<'_, AppContext>, model_id: String) -> CommandResult<()> {
    if let Some(flag) = ctx.downloads.lock().map_err(lock_err)?.get(&model_id) {
        flag.cancel();
    }
    Ok(())
}

/// # Errors
///
/// When the recogniser lock is poisoned, or the files cannot be deleted.
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
        let loaded_this = guard.as_ref().is_some_and(|r| r.model_id() == model_id);
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

/// # Errors
///
/// When the dictionary lock is poisoned.
#[tauri::command]
pub fn get_dictionary(ctx: State<'_, AppContext>) -> CommandResult<Vec<DictionaryEntry>> {
    Ok(ctx.dictionary.lock().map_err(lock_err)?.entries.clone())
}

/// # Errors
///
/// When `input` is blank, the lock is poisoned, or the file cannot be saved.
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

/// # Errors
///
/// When `input` is blank, no entry has that id, the lock is poisoned, or the
/// file cannot be saved.
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

/// # Errors
///
/// When no entry has that id, the lock is poisoned, or the file cannot be
/// saved.
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

/// # Errors
///
/// When the dictionary file cannot be written.
fn persist_dictionary(ctx: &AppContext, dictionary: &Dictionary) -> CommandResult<()> {
    let path = crate::dictionary::dictionary_path(&ctx.data_dir);
    dictionary.save(&path).map_err(|e| {
        tracing::error!(error = %e, "could not save the dictionary");
        e.user_message()
    })
}

/// Whether the OS will let us paste into other applications.
///
/// # Errors
///
/// Never; the signature matches the other commands so the UI can treat them
/// alike.
#[tauri::command]
pub fn can_insert_text(ctx: State<'_, AppContext>) -> CommandResult<bool> {
    Ok(ctx.inserter.can_insert())
}

/// Open the Accessibility settings pane so the user can grant permission.
///
/// # Errors
///
/// Never; the signature matches the other commands so the UI can treat them
/// alike.
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

/// Register or unregister the app as a login item.
///
/// # Errors
///
/// When the OS refuses to add or remove the login item.
fn apply_start_at_login(app: &AppHandle, enabled: bool) -> CommandResult<()> {
    use tauri_plugin_autostart::ManagerExt;

    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };

    result.map_err(|e| {
        tracing::warn!(error = %e, enabled, "could not change the login item");
        format!(
            "Starting at login could not be changed. You can add WhisperFree manually in {}.",
            crate::platform::strings::LOGIN_ITEMS_SETTINGS
        )
    })?;

    tracing::info!(event = "start_at_login_changed", enabled);
    Ok(())
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
