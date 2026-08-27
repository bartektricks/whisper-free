//! Hushing the rest of the machine while the microphone is open.
//!
//! Outside this module the app knows two verbs, `silence` and `restore`, and
//! one pure rule saying which of them the current state calls for. *How* a
//! platform goes quiet belongs to `platform::mute_system_output` and to nothing
//! else.
//!
//! Muting is advisory in the sense `refine/` and `update/` are advisory: an
//! output device that cannot be silenced is a debug line and a dictation that
//! runs anyway. Nothing here touches `AppState`, nothing here reports an error
//! to the user, and nothing here can fail a dictation.
//!
//! See `docs/decisions/0009-muting-other-audio-while-recording.md`.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use crate::platform;
use crate::state::{AppState, StateSnapshot};
use crate::AppContext;

/// How long the exit path waits for the sound to be back.
///
/// Generous for what it covers, which is one property write on a device that is
/// already open. Anything slower than this is a driver that is not going to
/// answer at all.
const RESTORE_TIMEOUT: Duration = Duration::from_secs(2);

enum Command {
    Silence,
    /// The channel is how the exit path waits for the sound to be back before
    /// the process goes away; everything else passes `None`.
    Restore(Option<Sender<()>>),
}

/// Handle to the thread that owns the output device.
///
/// A thread for the same reason [`crate::audio::AudioEngine`] is one: Windows
/// reaches the endpoint through COM, whose interfaces are bound to the
/// apartment that created them, so one thread has to hold them for their whole
/// life.
///
/// Unlike the audio engine these sends carry **no reply channel**. The callers
/// include `publish_state`, which runs on whichever thread caused the
/// transition, and that is often the hotkey handler, which on macOS is the
/// main thread. None of them may be made to wait on a device. The channel is
/// also what
/// keeps a silence from overtaking the restore before it.
pub struct MuteEngine {
    tx: Sender<Command>,
}

impl MuteEngine {
    #[must_use]
    pub fn spawn() -> Self {
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("mute".into())
            .spawn(move || mute_thread(&rx));

        // The receiver goes down with the failed spawn, so every later send
        // fails and is logged. Silencing is a courtesy; losing it is not worth
        // refusing to start over.
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not start the mute thread");
        }

        Self { tx }
    }

    /// Ask for the machine to go quiet. Idempotent.
    pub fn silence(&self) {
        self.send(Command::Silence, "silence");
    }

    /// Ask for it to come back exactly as it was. Idempotent, and a no-op when
    /// nothing was silenced.
    pub fn restore(&self) {
        self.send(Command::Restore(None), "restore");
    }

    /// Restore, and wait until it has actually happened.
    ///
    /// Only for the exit path. Everywhere else a caller that waited would be
    /// making the hotkey handler wait on a device, but quitting mid-recording
    /// has to outlast the send, or the process would be gone before the thread
    /// picked the message up and the machine would be left silent.
    ///
    /// The wait is bounded, because the thread it waits on is mid-conversation
    /// with an audio device. A driver that never answers would otherwise leave
    /// the app unable to quit, and of the two ways this can go wrong, output
    /// left muted is the one the user undoes with the volume key. It is also
    /// exactly what a crash already leaves behind, per decision 0009.
    pub fn restore_blocking(&self) {
        let (tx, rx) = channel();
        self.send(Command::Restore(Some(tx)), "restore");
        // A thread that is already gone reports back immediately rather than
        // waiting the timeout out: the reply channel travels inside the
        // undelivered command and is dropped with it.
        if let Err(e) = rx.recv_timeout(RESTORE_TIMEOUT) {
            tracing::warn!(event = "mute_restore_unconfirmed", reason = ?e);
        }
    }

    fn send(&self, command: Command, what: &'static str) {
        if self.tx.send(command).is_err() {
            tracing::warn!(event = "mute_engine_gone", request = what);
        }
    }
}

fn mute_thread(rx: &Receiver<Command>) {
    // The same shape as `audio_thread`'s `Option<Active>`: a plain local, one
    // owner, no lock, and the channel is what orders silence against restore.
    let mut muted: Option<platform::OutputMute> = None;

    while let Ok(command) = rx.recv() {
        match command {
            Command::Silence => {
                if muted.is_none() {
                    muted = platform::mute_system_output();
                }
            }
            Command::Restore(done) => {
                if let Some(mute) = muted.take() {
                    platform::restore_system_output(mute);
                }
                if let Some(done) = done {
                    let _ = done.send(());
                }
            }
        }
    }
}

/// Whether the machine should be quiet right now.
///
/// The mute lasts exactly as long as the microphone is open. Transcription,
/// cleanup and the paste all happen with sound back on, because none of them
/// can hear anything.
#[must_use]
pub const fn wants_silence(state: AppState, enabled: bool) -> bool {
    enabled && matches!(state, AppState::Recording)
}

/// Bring the output device in line with the state just published.
///
/// Called from `publish_state` rather than from either end of the recording,
/// which is deliberate. `publish_state` is the one point every path out of a
/// recording passes through, including the ones that never reach
/// `audio.stop()`, so the restore cannot be missed by a path nobody thought of.
pub fn apply(ctx: &AppContext, snapshot: &StateSnapshot) {
    // Taken and dropped before the send, the discipline `overlay::apply` keeps
    // for the same lock: `commands::update_settings` reaches this path having
    // just held it. A poisoned lock reads as "off", so the failure direction is
    // a machine that stays audible rather than one stuck silent.
    let enabled = ctx
        .settings
        .lock()
        .is_ok_and(|settings| settings.mute_while_recording);

    if wants_silence(snapshot.state, enabled) {
        ctx.mute.silence();
    } else {
        ctx.mute.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_wanted_only_while_the_microphone_is_open() {
        assert!(wants_silence(AppState::Recording, true));

        for state in [
            AppState::Uninitialized,
            AppState::Ready,
            AppState::Transcribing,
            AppState::Refining,
            AppState::Inserting,
            AppState::Error,
        ] {
            assert!(
                !wants_silence(state, true),
                "{state:?} does not hold the microphone open"
            );
        }
    }

    /// The exit path waits on this, so a mute thread that never started must
    /// not be able to stop the app quitting. It returns because the reply
    /// channel travels *inside* the undelivered command and dies with it,
    /// which a refactor of `send` could quietly break.
    #[test]
    fn restore_blocking_returns_when_the_mute_thread_is_gone() {
        let (tx, rx) = channel();
        drop(rx);
        let engine = MuteEngine { tx };

        let start = std::time::Instant::now();
        engine.restore_blocking();

        assert!(
            start.elapsed() < RESTORE_TIMEOUT,
            "waited the timeout out instead of noticing the thread was gone"
        );
    }

    #[test]
    fn silence_is_never_wanted_when_the_setting_is_off() {
        for state in [
            AppState::Uninitialized,
            AppState::Ready,
            AppState::Recording,
            AppState::Transcribing,
            AppState::Refining,
            AppState::Inserting,
            AppState::Error,
        ] {
            assert!(
                !wants_silence(state, false),
                "opted out, but {state:?} muted"
            );
        }
    }
}
