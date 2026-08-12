//! The dictation pipeline: hotkey -> record -> transcribe -> insert.
//!
//! This is the orchestration layer. It knows the *order* of the steps and who
//! owns the state, but nothing about how any individual step works.

use tauri::{AppHandle, Manager};

use crate::asr::{AsrError, TranscriptionOptions};
use crate::hotkey::{decide, HotkeyAction, HotkeyEvent};
use crate::state::AppState;
use crate::text_insertion::ClipboardOutcome;
use crate::{fail_app_state, set_app_state, AppContext};

/// Entry point for every hotkey transition.
pub fn on_hotkey(app: &AppHandle, event: HotkeyEvent) {
    let Some(ctx) = app.try_state::<AppContext>() else {
        return;
    };

    let mode = match ctx.settings.lock() {
        Ok(settings) => settings.recording_mode,
        Err(_) => {
            tracing::error!("settings lock poisoned; ignoring hotkey");
            return;
        }
    };

    // Ask the audio engine rather than trusting our own state: it is the thing
    // that actually holds the microphone.
    let recording = ctx.audio.is_recording();

    match decide(mode, event, recording) {
        HotkeyAction::Start => begin(app, &ctx),
        HotkeyAction::Stop => finish(app, &ctx),
        HotkeyAction::Ignore => {}
    }
}

fn begin(app: &AppHandle, ctx: &AppContext) {
    // Test the actual precondition — a loaded recogniser — rather than the
    // state. After a previous failure the state is `Error`, which would make a
    // state-based check miss a genuinely missing model.
    let has_model = ctx
        .recognizer
        .lock()
        .map(|r| r.is_some())
        .unwrap_or(false);
    if !has_model {
        // Recording audio we have no way to transcribe would waste the user's
        // breath, so refuse early and say where to fix it.
        fail_app_state(
            app,
            "No speech model is installed yet. Open Settings › Models to download one.",
        );
        return;
    }

    // A previous failure left us in `Error`, from which `Recording` is not a
    // legal transition. Starting a new dictation is exactly the moment that
    // error stops being relevant, so clear it first.
    let in_error = ctx
        .state
        .lock()
        .map(|s| s.state() == AppState::Error)
        .unwrap_or(false);
    if in_error {
        set_app_state(app, AppState::Ready);
    }

    let device = ctx
        .settings
        .lock()
        .ok()
        .and_then(|s| s.input_device.clone());

    if let Err(e) = ctx.audio.start(device) {
        tracing::warn!(event = "recording_start_failed", error = %e);
        fail_app_state(app, e.user_message());
        return;
    }

    set_app_state(app, AppState::Recording);
}

fn finish(app: &AppHandle, ctx: &AppContext) {
    let audio = match ctx.audio.stop() {
        Ok(audio) => audio,
        Err(e) => {
            tracing::warn!(event = "recording_stop_failed", error = %e);
            fail_app_state(app, e.user_message());
            return;
        }
    };

    // Too short to contain speech — almost always a mis-tap. Returning to Ready
    // silently is kinder than an error banner.
    if audio.duration().as_millis() < crate::audio::MIN_USEFUL_DURATION_MS as u128 {
        tracing::info!(
            event = "recording_discarded",
            reason = "too_short",
            duration_ms = audio.duration().as_millis() as u64
        );
        set_app_state(app, AppState::Ready);
        return;
    }

    set_app_state(app, AppState::Transcribing);

    let options = TranscriptionOptions {
        language: ctx
            .settings
            .lock()
            .map(|s| s.language.clone())
            .unwrap_or_default(),
    };

    let transcription = {
        let mut guard = match ctx.recognizer.lock() {
            Ok(guard) => guard,
            Err(_) => {
                fail_app_state(app, "The speech engine hit an internal error. Restart LocalDictation.");
                return;
            }
        };

        match guard.as_mut() {
            Some(recognizer) => recognizer.transcribe(&audio, &options),
            None => Err(AsrError::ModelNotInstalled),
        }
    };

    let transcription = match transcription {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(event = "transcription_failed", error = %e);
            fail_app_state(app, asr_user_message(&e));
            return;
        }
    };

    // Durations and rates only — never the transcribed text (plan §18).
    tracing::info!(
        event = "transcription_completed",
        audio_ms = transcription.audio_duration.as_millis() as u64,
        inference_ms = transcription.duration.as_millis() as u64,
        real_time_factor = transcription.real_time_factor(),
        chars = transcription.text.chars().count()
    );

    // A real outcome, not a theoretical one — see decision 0001. Saying nothing
    // would leave the user wondering whether the hotkey even worked.
    if transcription.is_empty() {
        tracing::info!(event = "transcription_empty");
        fail_app_state(
            app,
            "Nothing was recognised in that recording. Try speaking a little louder or for longer.",
        );
        return;
    }

    // Deterministic user replacements, applied before the text is delivered.
    let final_text = match ctx.dictionary.lock() {
        Ok(dictionary) => dictionary.apply(&transcription.text),
        Err(_) => {
            tracing::error!("dictionary lock poisoned; inserting the raw transcription");
            transcription.text.clone()
        }
    };

    set_app_state(app, AppState::Inserting);

    match ctx.inserter.insert(&final_text) {
        Ok(outcome) => {
            tracing::info!(
                event = "text_inserted",
                chars = final_text.chars().count(),
                clipboard = ?outcome.clipboard
            );
            // The user's clipboard held something we could not put back. Better
            // to say so than to let them discover it later.
            if outcome.clipboard == ClipboardOutcome::NonTextReplaced {
                fail_app_state(
                    app,
                    "Text inserted. Your clipboard held an image, which could not be restored.",
                );
                return;
            }
            set_app_state(app, AppState::Ready);
        }
        Err(e) => {
            tracing::warn!(event = "text_insertion_failed", error = %e);
            fail_app_state(app, e.user_message());
        }
    }
}

/// User-facing wording for an ASR failure (plan §17).
pub fn asr_user_message(error: &AsrError) -> String {
    match error {
        AsrError::ModelNotInstalled => {
            "No speech model is installed yet. Open Settings › Models to download one.".into()
        }
        AsrError::ModelLoad(_) => {
            "The speech model could not be loaded. Try removing and downloading it again in Settings › Models."
                .into()
        }
        AsrError::Transcription(_) => {
            "The recording could not be transcribed. Please try again.".into()
        }
        AsrError::UnsupportedLanguage { language } => {
            format!("The installed model cannot transcribe {language}. Choose another language in Settings › Speech.")
        }
        AsrError::UnsupportedCapability(what) => {
            format!("The installed model cannot {what}. Choose a different option in Settings › Speech.")
        }
        AsrError::Cancelled => "Transcription was cancelled.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asr_messages_are_plain_language_and_hide_internals() {
        let errors = [
            AsrError::ModelNotInstalled,
            AsrError::ModelLoad("ort::Error: failed to load encoder-model.int8.onnx".into()),
            AsrError::Transcription("shape mismatch [1, 128, 0]".into()),
            AsrError::Cancelled,
        ];
        for e in errors {
            let msg = asr_user_message(&e);
            assert!(!msg.is_empty());
            assert!(!msg.contains("onnx"), "leaked internals: {msg}");
            assert!(!msg.contains("ort::"), "leaked internals: {msg}");
            assert!(!msg.contains('['), "leaked internals: {msg}");
        }
    }

    #[test]
    fn a_missing_model_points_at_the_place_that_fixes_it() {
        let msg = asr_user_message(&AsrError::ModelNotInstalled);
        assert!(msg.contains("Models"));
    }
}
