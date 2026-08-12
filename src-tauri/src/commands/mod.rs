//! Tauri commands — the only way the UI talks to the backend (plan §16).

use tauri::{AppHandle, State};

use crate::audio::AudioDevice;
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
