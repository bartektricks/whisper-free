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

    let Ok(settings) = ctx.settings.lock() else {
        tracing::error!("settings lock poisoned; ignoring hotkey");
        return;
    };
    let mode = settings.recording_mode;
    drop(settings);

    // Ask the audio engine rather than trusting our own state: it is the thing
    // that actually holds the microphone.
    let recording = ctx.audio.is_recording();

    match decide(mode, event, recording) {
        HotkeyAction::Start => begin(app, &ctx),
        HotkeyAction::Stop => {
            // Inference takes hundreds of milliseconds and insertion sleeps
            // while the paste lands. Running that here would block the thread
            // delivering hotkey events, freezing the UI mid-dictation.
            let worker_app = app.clone();
            let spawned = std::thread::Builder::new()
                .name("dictation".into())
                .spawn(move || {
                    if let Some(ctx) = worker_app.try_state::<AppContext>() {
                        finish(&worker_app, &ctx);
                    }
                });

            if let Err(e) = spawned {
                tracing::error!(error = %e, "could not start the dictation thread");
                let _ = ctx.audio.cancel();
                fail_app_state(app, "Could not finish the recording. Please try again.");
            }
        }
        HotkeyAction::Ignore => {}
    }
}

fn begin(app: &AppHandle, ctx: &AppContext) {
    // Test the actual precondition — a loaded recogniser — rather than the
    // state. After a previous failure the state is `Error`, which would make a
    // state-based check miss a genuinely missing model.
    let has_model = ctx.recognizer.lock().is_ok_and(|r| r.is_some());
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
    // The previous dictation is still transcribing or pasting. Starting another
    // recording now would interleave two pipelines competing for the same
    // recogniser and the same clipboard.
    let current = ctx.state.lock().map(|s| s.state()).ok();
    if matches!(current, Some(AppState::Transcribing | AppState::Inserting)) {
        tracing::debug!(event = "recording_start_ignored", reason = "pipeline_busy");
        return;
    }

    // No explicit error clearing here: `Error -> Recording` is a legal
    // transition and clears the stale message on its own, which avoids racing
    // with failures reported from the dictation worker thread.

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
    if audio.duration().as_millis() < u128::from(crate::audio::MIN_USEFUL_DURATION_MS) {
        tracing::info!(
            event = "recording_discarded",
            reason = "too_short",
            duration_ms = crate::millis(audio.duration())
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
        let Ok(mut guard) = ctx.recognizer.lock() else {
            fail_app_state(
                app,
                "The speech engine hit an internal error. Restart LocalDictation.",
            );
            return;
        };

        guard.as_mut().map_or(Err(AsrError::ModelNotInstalled), |r| {
            r.transcribe(&audio, &options)
        })
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
        audio_ms = crate::millis(transcription.audio_duration),
        inference_ms = crate::millis(transcription.duration),
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
    let raw_text = transcription.text;
    let final_text = ctx.dictionary.lock().map_or_else(
        |_| {
            tracing::error!("dictionary lock poisoned; inserting the raw transcription");
            raw_text.clone()
        },
        |dictionary| dictionary.apply(&raw_text),
    );

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
#[must_use]
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
