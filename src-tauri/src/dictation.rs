//! The dictation pipeline: hotkey -> record -> transcribe -> insert.
//!
//! This is the orchestration layer. It knows the *order* of the steps and who
//! owns the state, but nothing about how any individual step works.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, Manager};

use crate::asr::{AsrError, TranscriptionOptions};
use crate::hotkey::chord::{classify, ChordStep};
use crate::hotkey::{decide, HotkeyAction, HotkeyEvent, Shortcut, CHORD_TIMEOUT};
use crate::refine;
use crate::state::AppState;
use crate::{fail_app_state, set_app_state, AppContext};

/// Entry point for every hotkey transition.
///
/// `fired` says which registered shortcut this was. With a chord live there are
/// two, and telling them apart is the whole of the routing below.
pub fn on_hotkey(app: &AppHandle, fired: &Shortcut, event: HotkeyEvent) {
    let Some(ctx) = app.try_state::<AppContext>() else {
        return;
    };

    // Before the chord machinery, which knows nothing about this key and would
    // classify it as `Ignore`. Only the press: the release is the same physical
    // action and would cancel twice.
    if crate::hotkey::is_cancel(fired) {
        if event == HotkeyEvent::Pressed {
            cancel(app, &ctx);
        }
        return;
    }

    let Ok(arming) = ctx.chord.lock() else {
        tracing::error!("chord lock poisoned; ignoring hotkey");
        return;
    };
    let step = classify(arming.chord(), fired, event, arming.window());
    drop(arming);

    match step {
        ChordStep::Arm => {
            arm_chord(app, &ctx);
            return;
        }
        ChordStep::Trigger => {}
        // Stop the clock before triggering: a recording that outlasts the
        // window must not have its second step unregistered underneath it.
        ChordStep::TriggerAndHold => hold_chord(&ctx),
        ChordStep::TriggerAndClose => close_chord(app, &ctx),
        ChordStep::Ignore => return,
    }

    trigger(app, &ctx, event);
}

/// Act on the edge that is the dictation trigger, whichever step produced it.
fn trigger(app: &AppHandle, ctx: &AppContext, event: HotkeyEvent) {
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
        HotkeyAction::Start => begin(app, ctx),
        HotkeyAction::Stop => {
            // `recording` was read a moment ago, and on Windows several release
            // events can arrive concurrently on different threads. Claim the
            // right to finish before acting on it, or two pipelines would run
            // and the loser's `audio.stop()` would raise a spurious error.
            if !claim_finish(&ctx.finishing) {
                tracing::debug!(event = "recording_stop_ignored", reason = "already_finishing");
                return;
            }

            // Inference takes hundreds of milliseconds and insertion sleeps
            // while the paste lands. Running that here would block the thread
            // delivering hotkey events, freezing the UI mid-dictation.
            let worker_app = app.clone();
            // Escape stays claimed across the whole run, not just the recording:
            // transcription and the paste are both things a user may want to
            // call off once they realise they misspoke.
            let spawned = std::thread::Builder::new()
                .name("dictation".into())
                .spawn(move || {
                    if let Some(ctx) = worker_app.try_state::<AppContext>() {
                        finish(&worker_app, &ctx);
                        // The run is over however it ended, so Escape goes back
                        // to the rest of the system. Not on the hotkey handler's
                        // thread here, so the unregistration can be direct.
                        release_cancel_key(&ctx);
                        release_finish(&ctx.finishing);
                    }
                });

            if let Err(e) = spawned {
                tracing::error!(error = %e, "could not start the dictation thread");
                let _ = ctx.audio.cancel();
                off_handler(app, "cancel-release", release_cancel_key);
                release_finish(&ctx.finishing);
                fail_app_state(app, "Could not finish the recording. Please try again.");
            }
        }
        HotkeyAction::Ignore => {}
    }
}

/// How long to leave the prefix unregistered while a replayed press travels.
///
/// Long enough for the synthetic event to be dispatched past our own hotkey
/// registration, short enough that a user pressing the prefix again straight
/// away does not notice it missing.
const REPLAY_SETTLE: std::time::Duration = std::time::Duration::from_millis(50);

/// Run `work` off the hotkey handler.
///
/// Nothing that registers or unregisters a shortcut may run on the thread that
/// delivers hotkey events. The plugin holds its shortcut map locked for the
/// whole of the handler and sends registration to the main thread to wait on
/// it, so calling either from here would deadlock against ourselves — on macOS
/// the handler *is* the main thread, and on both platforms the map is the same
/// lock. Every registration below therefore happens on a thread of its own.
fn off_handler(app: &AppHandle, name: &str, work: impl FnOnce(&AppContext) + Send + 'static) {
    let worker_app = app.clone();
    let spawned = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            if let Some(ctx) = worker_app.try_state::<AppContext>() {
                work(&ctx);
            }
        });

    if let Err(e) = spawned {
        tracing::error!(error = %e, thread = name, "could not start a chord thread");
    }
}

/// Open the window for a chord's second step.
///
/// The second step is claimed from the OS only for as long as this window is
/// open. Registering it permanently would swallow a bare key system-wide, which
/// is exactly what a chord exists to avoid.
fn arm_chord(app: &AppHandle, ctx: &AppContext) {
    let Ok(mut arming) = ctx.chord.lock() else {
        tracing::error!("chord lock poisoned; cannot arm");
        return;
    };
    let Some(second) = arming.chord().second.clone() else {
        return; // Not a chord; `classify` would not have asked for this.
    };
    // Re-arming is the ordinary case on Windows, where `WM_HOTKEY` repeats
    // while the key is held: the second step is already registered, so only
    // the clock restarts.
    let already_registered = arming.holds_second_step();
    let generation = arming.arm();
    drop(arming);

    tracing::debug!(event = "chord_armed", generation);

    let app = app.clone();
    off_handler(&app.clone(), "chord-window", move |ctx| {
        if !already_registered {
            if let Err(e) = ctx.hotkeys.add(&second.accelerator) {
                tracing::warn!(event = "chord_second_step_failed", error = %e);
                // The prefix was swallowed and the chord can never complete, so
                // give the key back and say why the hotkey did nothing.
                if let Some(step) = close_window(ctx) {
                    debug_assert_eq!(step.accelerator, second.accelerator);
                }
                replay_prefix(ctx);
                fail_app_state(&app, e.user_message());
                return;
            }
        }

        std::thread::sleep(CHORD_TIMEOUT);
        abandon_chord(ctx, generation);
    });
}

/// The second step is down: stop the timeout from taking it away.
///
/// Nothing is registered or unregistered here, so it is safe on the hotkey
/// handler's thread.
fn hold_chord(ctx: &AppContext) {
    let Ok(mut arming) = ctx.chord.lock() else {
        tracing::error!("chord lock poisoned; cannot hold the chord window");
        return;
    };
    arming.hold();
}

/// Close the window in the shared state, returning the second step that is
/// still registered and now needs releasing.
///
/// `None` when nothing was registered, which is also how two threads racing to
/// close the same window settle it: only one of them gets the step.
fn close_window(ctx: &AppContext) -> Option<crate::hotkey::chord::Step> {
    let Ok(mut arming) = ctx.chord.lock() else {
        tracing::error!("chord lock poisoned; cannot close the chord window");
        return None;
    };
    if !arming.holds_second_step() {
        return None;
    }
    arming.close();
    arming.chord().second.clone()
}

/// Hand the second step back to the rest of the system.
fn release_second(ctx: &AppContext, accelerator: &str) {
    if let Err(e) = ctx.hotkeys.remove(accelerator) {
        tracing::warn!(event = "chord_second_step_release_failed", error = %e);
    }
}

/// The chord completed: stop stealing its second step.
///
/// Deliberately driven by the second step's *release* rather than by the
/// timeout, because hold-to-talk needs that release to arrive — and it only
/// arrives while the shortcut is still registered.
fn close_chord(app: &AppHandle, ctx: &AppContext) {
    let Some(second) = close_window(ctx) else {
        return;
    };
    off_handler(app, "chord-release", move |ctx| {
        release_second(ctx, &second.accelerator);
    });
}

/// The second step never came: close the window and hand the prefix back.
///
/// `generation` is the window this timeout was started for. A later press may
/// have closed that window and opened another, and abandoning *that* one would
/// cut short a chord the user is still in the middle of typing.
fn abandon_chord(ctx: &AppContext, generation: u64) {
    let Ok(arming) = ctx.chord.lock() else {
        tracing::error!("chord lock poisoned; cannot time out the chord window");
        return;
    };
    let still_ours = arming.still_open(generation);
    drop(arming);
    if !still_ours {
        return;
    }

    // Info rather than debug: this is the app synthesising a keystroke into
    // someone else's window, which is worth being able to see in a log without
    // turning on debug for everything.
    tracing::info!(event = "chord_abandoned", generation);
    if let Some(second) = close_window(ctx) {
        release_second(ctx, &second.accelerator);
    }
    replay_prefix(ctx);
}

/// Give the prefix back to whatever has focus, as though we had never had it.
///
/// The registration has to come off first: the synthetic press is posted at the
/// HID level, where our own global shortcut would catch it and arm the chord
/// again, and again. Re-registering waits for the same reason — the event is
/// delivered asynchronously, so re-registering immediately would race the very
/// press we just sent.
fn replay_prefix(ctx: &AppContext) {
    let Ok(arming) = ctx.chord.lock() else {
        return;
    };
    let prefix = arming.chord().prefix.clone();
    drop(arming);

    if let Err(e) = ctx.hotkeys.remove(&prefix.accelerator) {
        // Still registered, so replaying would feed our own handler. Skipping
        // the replay costs the user one keystroke; the loop would cost them
        // the hotkey entirely.
        tracing::warn!(event = "chord_replay_skipped", error = %e);
        return;
    }

    if let Err(e) = crate::platform::replay_keystroke(&prefix.keystroke()) {
        tracing::warn!(event = "chord_replay_failed", error = %e);
    }

    std::thread::sleep(REPLAY_SETTLE);

    if let Err(e) = ctx.hotkeys.add(&prefix.accelerator) {
        // The hotkey is gone until the user changes it, which is worth shouting
        // about even though there is no app handle here to surface it with.
        tracing::error!(event = "hotkey_reregistration_failed", error = %e);
    }
}

/// Take exclusive ownership of ending the current recording.
///
/// Returns `false` when another thread already holds it, which is the caller's
/// signal to do nothing at all. Takes the flag rather than the context so the
/// rule can be tested without an app.
fn claim_finish(gate: &AtomicBool) -> bool {
    gate.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// Hand the claim back, so the next hotkey press can end its own recording.
fn release_finish(gate: &AtomicBool) {
    gate.store(false, Ordering::Release);
}

/// Claim Escape for the duration of a dictation.
///
/// Escape is everyone else's key; we borrow it only while there is something to
/// abandon. The flag makes the borrow single-entry, so two starts cannot leave
/// two registrations behind — or one release cancel out the other's claim.
///
/// Registration goes through [`off_handler`] because this is reached from the
/// hotkey handler, where registering anything deadlocks. A failure is a warning
/// and nothing more: not being able to cancel is a smaller problem than not
/// being able to dictate.
fn arm_cancel_key(app: &AppHandle) {
    let Some(ctx) = app.try_state::<AppContext>() else {
        return;
    };
    if ctx
        .cancel_key_held
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    off_handler(app, "cancel-arm", |ctx| {
        if let Err(e) = ctx.hotkeys.add(crate::hotkey::CANCEL_ACCELERATOR) {
            tracing::warn!(event = "cancel_key_unavailable", error = %e);
            ctx.cancel_key_held.store(false, Ordering::Release);
        }
    });
}

/// Give Escape back to the rest of the system.
///
/// Safe to call from anywhere and as often as you like; only the thread that
/// wins the flag issues the unregistration.
fn release_cancel_key(ctx: &AppContext) {
    if ctx
        .cancel_key_held
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    if let Err(e) = ctx.hotkeys.remove(crate::hotkey::CANCEL_ACCELERATOR) {
        tracing::warn!(event = "cancel_key_release_failed", error = %e);
    }
}

/// Abandon the dictation in progress, if there is one.
///
/// Two cases, and the `finishing` gate is what tells them apart. Holding it
/// means we own the audio engine and can stop it here. Failing to take it means
/// the pipeline is already past recording — mid-inference, or mid-paste — where
/// there is nothing to interrupt. Setting [`AppContext::cancelled`] is then the
/// whole of the cancellation: [`finish`] checks it before it does anything the
/// user would see.
fn cancel(app: &AppHandle, ctx: &AppContext) {
    ctx.cancelled.store(true, Ordering::Release);

    // Refinement is the one interruptible stage, so tell it directly rather
    // than making the user wait out a generation they have already abandoned.
    // Cheap and harmless when nothing is running: the flag is reset at the
    // start of every `refine`.
    if let Ok(guard) = ctx.refiner.lock() {
        if let Some(refiner) = guard.as_ref() {
            refiner.cancel();
        }
    }

    if !claim_finish(&ctx.finishing) {
        tracing::info!(event = "dictation_cancelled", stage = "pipeline");
        return;
    }

    if ctx.audio.is_recording() {
        if let Err(e) = ctx.audio.cancel() {
            tracing::warn!(event = "recording_cancel_failed", error = %e);
        }
        tracing::info!(event = "dictation_cancelled", stage = "recording");
        // Straight back to Ready: nothing was transcribed, so there is nothing
        // to report and no error to show.
        set_app_state(app, AppState::Ready);
    }

    // On the hotkey handler's thread, so the unregistration cannot happen here.
    off_handler(app, "cancel-release", |ctx| {
        release_cancel_key(ctx);
        release_finish(&ctx.finishing);
    });
}

/// Has Escape been pressed since this run started?
fn is_cancelled(ctx: &AppContext) -> bool {
    ctx.cancelled.load(Ordering::Acquire)
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

    // Fresh run, so any Escape from the previous one is history.
    ctx.cancelled.store(false, Ordering::Release);
    arm_cancel_key(app);

    set_app_state(app, AppState::Recording);
}

fn finish(app: &AppHandle, ctx: &AppContext) {
    // Escape landed between the hotkey and this thread starting. The audio is
    // already discarded, so there is nothing left to do.
    if is_cancelled(ctx) {
        tracing::info!(event = "dictation_cancelled", stage = "before_stop");
        set_app_state(app, AppState::Ready);
        return;
    }

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
                "The speech engine hit an internal error. Restart WhisperFree.",
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

    // The last point where cancelling still means anything: after this the text
    // is on its way to someone else's window. Inference cannot be interrupted
    // part-way, so Escape during it lands here — the work was wasted, but the
    // paste the user called off does not happen.
    if is_cancelled(ctx) {
        tracing::info!(event = "dictation_cancelled", stage = "before_insert");
        set_app_state(app, AppState::Ready);
        return;
    }

    let raw_text = transcription.text;

    // A language model's opinion of what was said, if the user asked for one.
    // Advisory: anything that goes wrong in here returns the raw text.
    let checked = refine_text(app, ctx, &raw_text);

    // Escape may have landed while the model was deciding. Unlike the speech
    // model, this one is interruptible, so the flag is also checked between
    // tokens; this catches the case where it arrived just after the last one.
    if is_cancelled(ctx) {
        tracing::info!(event = "dictation_cancelled", stage = "after_refine");
        set_app_state(app, AppState::Ready);
        return;
    }

    // Deterministic user replacements, applied last so a rule the user wrote
    // by hand is never second-guessed by a model.
    let final_text = ctx.dictionary.lock().map_or_else(
        |_| {
            tracing::error!("dictionary lock poisoned; inserting the unreplaced transcription");
            checked.clone()
        },
        |dictionary| dictionary.apply(&checked),
    );

    insert(app, ctx, &final_text);
}

/// The last step: text into whoever has focus, and back to Ready.
fn insert(app: &AppHandle, ctx: &AppContext, final_text: &str) {
    set_app_state(app, AppState::Inserting);

    // Read once, before the paste. A settings change landing mid-insertion
    // should govern the next dictation, not change what this one does halfway.
    let keep_on_clipboard = ctx
        .settings
        .lock()
        .is_ok_and(|settings| settings.keep_on_clipboard);

    match ctx.inserter.insert(final_text, keep_on_clipboard) {
        Ok(outcome) => {
            tracing::info!(
                event = "text_inserted",
                chars = final_text.chars().count(),
                clipboard = ?outcome.clipboard
            );
            remember(app, ctx, final_text);
            // Only a clipboard that could not be put back at all is worth
            // interrupting anyone over. `PartlyRestored` is deliberately not:
            // the flavour that could not be read is nearly always one macOS
            // derives from another (plain text from RTF, TIFF from PNG), and
            // it reappears by itself once the flavour it comes from is back.
            // Reporting it would be the false alarm decision 0010 removed.
            if outcome.clipboard.lost_the_clipboard() {
                fail_app_state(app, "Text inserted, but your clipboard could not be put back.");
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

/// Keep what was just inserted, if the user has asked us to (decision 0011).
///
/// Advisory in the same sense refinement is: a poisoned lock or a disk that
/// will not take the file is a log line, never a failed dictation. The words
/// are already in the user's document by the time this runs, and losing the
/// copy of them matters less than telling them their dictation failed when it
/// did not.
///
/// The text itself is never logged, here or anywhere.
fn remember(app: &AppHandle, ctx: &AppContext, final_text: &str) {
    let Ok(settings) = ctx.settings.lock() else {
        tracing::error!("settings lock poisoned; the transcription was not kept");
        return;
    };
    if !settings.history_enabled {
        return;
    }
    let retention = settings.history_retention;
    drop(settings);

    let Ok(mut history) = ctx.history.lock() else {
        tracing::error!("history lock poisoned; the transcription was not kept");
        return;
    };
    history.record(final_text, crate::history::now());
    history.prune(retention, crate::history::now());

    // `Session` keeps the list and writes nothing, which is the whole of what
    // choosing it means.
    if retention.persists() {
        if let Err(e) = history.save(&crate::history::history_path(&ctx.data_dir)) {
            tracing::warn!(error = %e, "could not save the history");
        }
    }
    let count = history.entries.len();
    drop(history);

    tracing::info!(event = "transcription_kept", entries = count);
    let _ = app.emit(crate::EVENT_HISTORY_CHANGED, ());
}

/// Run the transcription past the refinement model, if one is switched on and
/// loaded.
///
/// Returns text to insert either way, and never fails the dictation: an absent
/// model, a load error, a rejected rewrite and a cancelled run all resolve to
/// the raw transcription. Losing a correction disappoints; losing the user's
/// words is a bug (decision 0005).
fn refine_text(app: &AppHandle, ctx: &AppContext, raw: &str) -> String {
    let enabled = ctx
        .settings
        .lock()
        .is_ok_and(|settings| settings.refine_enabled);
    if !enabled {
        return raw.to_owned();
    }

    // The user's own words, so the model does not "correct" a term it has
    // simply never seen. Their spellings, not the misheard forms.
    let vocabulary = ctx.dictionary.lock().map_or_else(
        |_| Vec::new(),
        |dictionary| dictionary.replacement_terms(),
    );

    let Ok(mut guard) = ctx.refiner.lock() else {
        tracing::warn!(event = "refinement_skipped", reason = "lock_poisoned");
        return raw.to_owned();
    };
    let Some(refiner) = guard.as_mut() else {
        tracing::info!(event = "refinement_skipped", reason = "not_loaded");
        return raw.to_owned();
    };

    set_app_state(app, AppState::Refining);

    // Keeps the idle watchdog in `lib.rs` from unloading a model in active use.
    if let Ok(mut last_used) = ctx.refiner_last_used.lock() {
        *last_used = Some(std::time::Instant::now());
    }

    let options = refine::RefineOptions { vocabulary };
    let refinement = match refiner.refine(raw, &options) {
        Ok(refinement) => refinement,
        Err(e) => {
            tracing::warn!(event = "refinement_failed", error = %e);
            return raw.to_owned();
        }
    };

    // Shapes only — never the transcription, the candidate, or the vocabulary.
    tracing::info!(
        event = "refinement_completed",
        prompt_tokens = refinement.prompt_tokens,
        generated_tokens = refinement.generated_tokens,
        prefill_ms = crate::millis(refinement.prefill),
        total_ms = crate::millis(refinement.duration)
    );

    match refine::guard::judge(raw, &refinement.text, &refine::Limits::default()) {
        refine::Verdict::Accept(text) => {
            tracing::info!(
                event = "refinement_accepted",
                before_chars = raw.chars().count(),
                after_chars = text.chars().count()
            );
            text
        }
        refine::Verdict::Unchanged => {
            tracing::info!(event = "refinement_unchanged");
            raw.to_owned()
        }
        refine::Verdict::Reject(reason) => {
            tracing::info!(event = "refinement_rejected", reason = reason.as_str());
            raw.to_owned()
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

    #[test]
    fn only_one_release_event_gets_to_finish_a_recording() {
        // Windows delivers a release per `WM_HOTKEY` repeat, so this is the
        // ordinary case there rather than a rare race.
        let gate = AtomicBool::new(false);
        assert!(claim_finish(&gate));
        assert!(!claim_finish(&gate));
        assert!(!claim_finish(&gate));
    }

    #[test]
    fn the_next_recording_can_be_finished_once_the_claim_is_released() {
        let gate = AtomicBool::new(false);
        assert!(claim_finish(&gate));
        release_finish(&gate);
        assert!(claim_finish(&gate), "the gate stayed shut after release");
    }

    #[test]
    fn concurrent_claims_produce_exactly_one_winner() {
        let gate = AtomicBool::new(false);
        let won = std::thread::scope(|scope| {
            // Every thread has to be running before any of them is joined,
            // otherwise this walks the claims one at a time and proves nothing.
            let mut handles = Vec::new();
            for _ in 0..8 {
                handles.push(scope.spawn(|| claim_finish(&gate)));
            }
            handles
                .into_iter()
                .filter_map(|h| h.join().ok())
                .filter(|claimed| *claimed)
                .count()
        });

        assert_eq!(won, 1, "the gate let more than one pipeline through");
    }
}
