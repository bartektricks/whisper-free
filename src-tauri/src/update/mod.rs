//! In-app updates (decision 0006).
//!
//! The shape is deliberately the one `refine` uses: **updating is advisory**.
//! It runs beside the dictation loop, never inside it, and it must never be
//! able to fail a dictation. So nothing here touches [`crate::state`] — the
//! status below is its own channel, with its own event, and a failed check is
//! a line in the Updates panel rather than an error banner over an app that
//! still transcribes perfectly well.
//!
//! Two rules the rest of the app depends on:
//!
//! - **Checking is opt-in.** `Settings::check_for_updates` is off by default,
//!   and while it is off nothing here makes a network call unless the user
//!   presses the button. That is what keeps the privacy invariant true.
//! - **Nothing here may run on the main thread.** When the app cannot be
//!   overwritten in place, the install path raises an admin prompt through
//!   `run_on_main_thread` and blocks waiting for the answer; called from the
//!   tray menu handler — which *is* the main thread on macOS — that deadlocks,
//!   the same way registering a shortcut from the hotkey handler does. Both
//!   public verbs return immediately and do their work on the async runtime.

pub mod schedule;

mod plugin;

use std::sync::atomic::Ordering;
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;
use crate::{tray, AppContext};

/// Emitted to the UI whenever the update status changes.
pub const EVENT_UPDATE_STATUS_CHANGED: &str = "update_status_changed";

/// Where a user reads what changed, or downloads a build by hand when an
/// install fails. The tag scheme is `docs/RELEASING.md`'s: `v` + the version.
pub const RELEASES_URL: &str = "https://github.com/bartektricks/whisper-free/releases";

/// Emit progress roughly this often, in bytes.
///
/// The bundles are tens of megabytes, so this is about fifty updates over a
/// download — enough for a bar that moves, few enough not to flood the
/// webview. `models::download` throttles the same way, at 2 MB.
const PROGRESS_STEP: u64 = 1 << 20;

/// Where a check came from.
///
/// It decides only how loud the outcome is: a check the user asked for always
/// answers, in success or failure, while the daily one stays silent unless it
/// has something to offer. An unprompted "Up to date" is noise, and an
/// unprompted "could not reach the update server" is worse — it reports a
/// problem with a feature the user is not currently using.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Manual,
    Scheduled,
}

/// Mirrors `UpdatePhase` in `src/types/index.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    /// Nothing has been asked yet this session.
    #[default]
    Idle,
    Checking,
    /// Checked, and this build is current.
    UpToDate,
    /// A newer version is published and waiting for the user to accept it.
    Available,
    Downloading,
    /// Installed on disk; the running process is still the old one.
    ReadyToRestart,
    /// Carries a user-facing message alongside.
    Failed,
}

/// Mirrors `UpdateStatus` in `src/types/index.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct UpdateStatus {
    pub phase: UpdatePhase,
    /// The version on offer, once a check has found one.
    pub version: Option<String>,
    /// Where to read what changed.
    ///
    /// The manifest's own `notes` field is deliberately not carried:
    /// `tauri-action` fills it with the workflow's `releaseBody`, which for
    /// this project is the install instructions rather than a changelog.
    pub release_url: Option<String>,
    pub downloaded_bytes: u64,
    /// Zero until the server announces a length, which it need not do.
    pub total_bytes: u64,
    /// User-facing failure text; only set in [`UpdatePhase::Failed`].
    pub message: Option<String>,
}

impl UpdateStatus {
    fn phase(phase: UpdatePhase) -> Self {
        Self {
            phase,
            ..Self::default()
        }
    }

    fn offering(phase: UpdatePhase, version: String) -> Self {
        Self {
            phase,
            release_url: Some(format!("{RELEASES_URL}/tag/v{version}")),
            version: Some(version),
            ..Self::default()
        }
    }

    fn failed(message: String) -> Self {
        Self {
            phase: UpdatePhase::Failed,
            message: Some(message),
            ..Self::default()
        }
    }
}

/// The handful of outcomes a user can do something about.
///
/// `plugin::classify` is what maps the transport's much longer error list onto
/// these; the raw detail is logged there and never travels further.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UpdateError {
    #[error("the update endpoint could not be reached")]
    Unreachable,
    #[error("no build is published for this platform")]
    NotPublished,
    #[error("the download did not verify against the bundled public key")]
    Signature,
    #[error("replacing the installed app was refused")]
    PermissionRefused,
    #[error("the update could not be written into place")]
    Install,
    #[error("a dictation is in progress")]
    Busy,
    #[error("no update has been offered yet")]
    NothingToInstall,
    #[error("the updater is not configured for this build")]
    Unavailable,
}

impl UpdateError {
    /// The message shown to the user (plan §17) — no internals, and where
    /// possible a way out that does not involve this feature working.
    #[must_use]
    pub fn user_message(&self) -> String {
        match *self {
            Self::Unreachable => {
                "Could not reach the update server. Check your internet connection and try again."
                    .into()
            }
            Self::NotPublished => {
                "No update is published for this computer yet. Check again later.".into()
            }
            Self::Signature => {
                // Deliberately not offered as retryable: a download that does
                // not verify is the one case where trying again is the wrong
                // advice.
                "The download could not be verified, so it was not installed. Download the new version from the releases page instead."
                    .into()
            }
            Self::PermissionRefused => {
                format!(
                    "WhisperFree needs permission to replace itself in {}. Try again and approve the prompt, or download the new version from the releases page.",
                    crate::platform::strings::INSTALL_LOCATION
                )
            }
            Self::Install => {
                format!(
                    "The update could not be installed. Check that WhisperFree in {} is not locked or open twice, or download the new version from the releases page.",
                    crate::platform::strings::INSTALL_LOCATION
                )
            }
            Self::Busy => {
                "WhisperFree is in the middle of a dictation. Try again in a moment.".into()
            }
            Self::NothingToInstall => "Check for updates first.".into(),
            Self::Unavailable => {
                "Updates are not available in this build. Download the new version from the releases page."
                    .into()
            }
        }
    }
}

/// The current status, for a UI that has just opened.
#[must_use]
pub fn status(ctx: &AppContext) -> UpdateStatus {
    ctx.update
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

/// Ask whether a newer version is published.
///
/// Returns immediately; the answer arrives as [`EVENT_UPDATE_STATUS_CHANGED`].
pub fn check(app: &AppHandle, trigger: Trigger) {
    let Some(ctx) = app.try_state::<AppContext>() else {
        return;
    };
    if !claim(&ctx) {
        // A check or a download is already running. A second click is a
        // no-op, not a second request.
        tracing::debug!(event = "update_check_skipped", reason = "already running");
        return;
    }

    // The attempt is recorded, not the outcome: an endpoint that is down must
    // not turn into a request every time the watchdog ticks.
    if let Ok(mut last) = ctx.last_update_check.lock() {
        *last = Some(Instant::now());
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        publish(&app, &UpdateStatus::phase(UpdatePhase::Checking));
        let outcome = plugin::check(&app).await;
        release(&app);

        match outcome {
            Ok(Some(found)) => {
                tracing::info!(event = "update_available", version = %found.version);
                publish(
                    &app,
                    &UpdateStatus::offering(UpdatePhase::Available, found.version),
                );
            }
            Ok(None) => {
                tracing::info!(event = "update_none", version = env!("CARGO_PKG_VERSION"));
                publish(&app, &quiet_outcome(trigger, UpdatePhase::UpToDate));
            }
            Err(e) => {
                // Already logged with its detail in `plugin::classify`.
                publish(&app, &quiet_failure(trigger, e));
            }
        }
    });
}

/// Download the offered update and put it in place.
///
/// Returns immediately; progress and the outcome arrive as
/// [`EVENT_UPDATE_STATUS_CHANGED`].
pub fn install(app: &AppHandle) {
    let Some(ctx) = app.try_state::<AppContext>() else {
        return;
    };

    // Checked against the real thing rather than against `AppState`, which can
    // be stale or `Error` after a failure — the same reasoning as
    // `dictation`'s preconditions. Installing mid-dictation would replace the
    // binary underneath a running transcription, and on Windows would kill the
    // process outright.
    if is_dictating(&ctx) {
        publish(app, &UpdateStatus::failed(UpdateError::Busy.user_message()));
        return;
    }

    // Only what a check actually offered may be installed, so a stray click
    // cannot start a download of something nobody looked at.
    let offered = match ctx.update.lock() {
        Ok(guard) if guard.phase == UpdatePhase::Available => guard.version.clone(),
        Ok(_) => {
            publish(
                app,
                &UpdateStatus::failed(UpdateError::NothingToInstall.user_message()),
            );
            return;
        }
        Err(_) => return,
    };
    let Some(offered) = offered else {
        return;
    };

    if !claim(&ctx) {
        tracing::debug!(event = "update_install_skipped", reason = "already running");
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tracing::info!(event = "update_install_started", version = %offered);

        let mut last_reported: u64 = 0;
        let result = plugin::install(&app, |downloaded, total| {
            let total = total.unwrap_or(0);
            // The final chunk always reports, so the bar reaches its end
            // rather than stopping a megabyte short.
            let finished = total > 0 && downloaded >= total;
            if !finished && downloaded.saturating_sub(last_reported) < PROGRESS_STEP {
                return;
            }
            last_reported = downloaded;
            publish(
                &app,
                &UpdateStatus {
                    phase: UpdatePhase::Downloading,
                    downloaded_bytes: downloaded,
                    total_bytes: total,
                    ..UpdateStatus::offering(UpdatePhase::Downloading, offered.clone())
                },
            );
        })
        .await;
        release(&app);

        match result {
            Ok(version) => {
                tracing::info!(event = "update_installed", version = %version);
                publish(
                    &app,
                    &UpdateStatus::offering(UpdatePhase::ReadyToRestart, version),
                );
            }
            Err(e) => publish(&app, &UpdateStatus::failed(e.user_message())),
        }
    });
}

/// Open the release notes for whatever is on offer, or the releases page when
/// nothing has been checked yet.
///
/// Opened from Rust, like every other link the app follows: the settings
/// window holds no `opener` permission, and the failure messages point at the
/// releases page too, so this is the one route for both.
pub fn open_release_notes(ctx: &AppContext) {
    let url = status(ctx)
        .release_url
        .unwrap_or_else(|| RELEASES_URL.to_string());
    crate::platform::open_url(&url);
}

/// Restart into the version just installed.
///
/// Only meaningful on macOS: the Windows installer exits the app itself, so by
/// the time an install succeeds there the process is already gone.
pub fn restart(app: &AppHandle) {
    tracing::info!(event = "update_restart_requested");
    app.restart();
}

/// Drop the update model to match the current settings, forever.
///
/// Started once at setup and left running. It is unconditional on purpose:
/// with the setting off every tick takes two locks, finds nothing to do, and
/// makes no network call, which is cheaper than starting and stopping a thread
/// each time the checkbox moves.
pub fn watch(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            std::thread::sleep(schedule::STARTUP_DELAY);
            loop {
                if should_check_now(&app) {
                    check(&app, Trigger::Scheduled);
                }
                std::thread::sleep(schedule::CHECK_POLL);
            }
        });

    if let Err(e) = spawned {
        // The button in Settings still works, so this costs the automatic
        // check and nothing else.
        tracing::error!(error = %e, "could not start the update check thread");
    }
}

/// One tick of the watchdog, split out so the borrow of managed state ends
/// with the call rather than spanning the loop.
fn should_check_now(app: &AppHandle) -> bool {
    let Some(ctx) = app.try_state::<AppContext>() else {
        return false;
    };
    // The setting is read every tick rather than captured once, so switching
    // it on takes effect without restarting anything.
    if !ctx.settings.lock().is_ok_and(|s| s.check_for_updates) {
        return false;
    }
    let last = match ctx.last_update_check.lock() {
        Ok(guard) => *guard,
        Err(_) => return false,
    };
    schedule::is_check_due(last, Instant::now(), schedule::CHECK_INTERVAL)
}

/// Whether the app is part-way through a dictation.
fn is_dictating(ctx: &AppContext) -> bool {
    if ctx.audio.is_recording() {
        return true;
    }
    ctx.state.lock().is_ok_and(|sm| {
        matches!(
            sm.snapshot().state,
            AppState::Recording | AppState::Transcribing | AppState::Refining | AppState::Inserting
        )
    })
}

/// A scheduled check that found nothing says nothing.
fn quiet_outcome(trigger: Trigger, loud: UpdatePhase) -> UpdateStatus {
    match trigger {
        Trigger::Manual => UpdateStatus::phase(loud),
        Trigger::Scheduled => UpdateStatus::phase(UpdatePhase::Idle),
    }
}

/// A scheduled check that failed says nothing either — it reports a problem
/// with a feature the user is not, at that moment, using.
fn quiet_failure(trigger: Trigger, error: UpdateError) -> UpdateStatus {
    match trigger {
        Trigger::Manual => UpdateStatus::failed(error.user_message()),
        Trigger::Scheduled => UpdateStatus::phase(UpdatePhase::Idle),
    }
}

/// Claim the single-entry flag, or report that someone else holds it.
///
/// Hotkey events taught this lesson once already (`AppContext::finishing`): a
/// check-then-act on "is something running" loses to two clicks landing
/// together, and two concurrent downloads of the same bundle would fight over
/// the same install path.
fn claim(ctx: &AppContext) -> bool {
    ctx.update_busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

fn release(app: &AppHandle) {
    if let Some(ctx) = app.try_state::<AppContext>() {
        ctx.update_busy.store(false, Ordering::SeqCst);
    }
}

/// Store the status, rewrite the tray label, and tell the UI.
///
/// The order matches `publish_state`: the shared copy is current before
/// anything can read it in response to the event.
fn publish(app: &AppHandle, status: &UpdateStatus) {
    let Some(ctx) = app.try_state::<AppContext>() else {
        return;
    };
    if let Ok(mut guard) = ctx.update.lock() {
        guard.clone_from(status);
    }
    if let Ok(guard) = ctx.tray.lock() {
        if let Some(handles) = guard.as_ref() {
            let _ = handles.updates.set_text(tray::update_text(status));
        }
    }
    if let Err(e) = app.emit(EVENT_UPDATE_STATUS_CHANGED, status) {
        tracing::warn!(error = %e, "could not emit update status");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_messages_never_leak_internals() {
        let errors = [
            UpdateError::Unreachable,
            UpdateError::NotPublished,
            UpdateError::Signature,
            UpdateError::PermissionRefused,
            UpdateError::Install,
            UpdateError::Busy,
            UpdateError::NothingToInstall,
            UpdateError::Unavailable,
        ];
        for e in errors {
            let msg = e.user_message();
            assert!(!msg.is_empty());
            // The transport is an implementation detail of `update/plugin.rs`.
            assert!(!msg.contains("reqwest"), "leaked internals: {msg}");
            assert!(!msg.contains("minisign"), "leaked internals: {msg}");
            assert!(!msg.contains("latest.json"), "leaked internals: {msg}");
            assert!(!msg.contains("tar"), "leaked internals: {msg}");
            assert!(!msg.contains("Error::"), "leaked internals: {msg}");
        }
    }

    #[test]
    fn a_failure_the_user_can_route_around_says_how() {
        // Every one of these leaves the user stuck unless the message points
        // somewhere, because the feature that would have helped is the one
        // that just failed.
        for e in [
            UpdateError::Signature,
            UpdateError::PermissionRefused,
            UpdateError::Install,
            UpdateError::Unavailable,
        ] {
            let msg = e.user_message();
            assert!(msg.contains("releases page"), "no way out offered: {msg}");
        }
    }

    #[test]
    fn install_failures_name_the_place_this_platform_calls_it() {
        // Never a literal: "the Applications folder" is wrong on Windows.
        let msg = UpdateError::PermissionRefused.user_message();
        assert!(msg.contains(crate::platform::strings::INSTALL_LOCATION));
    }

    #[test]
    fn a_scheduled_check_stays_silent_and_a_manual_one_answers() {
        assert_eq!(
            quiet_outcome(Trigger::Scheduled, UpdatePhase::UpToDate).phase,
            UpdatePhase::Idle
        );
        assert_eq!(
            quiet_outcome(Trigger::Manual, UpdatePhase::UpToDate).phase,
            UpdatePhase::UpToDate
        );
        assert_eq!(
            quiet_failure(Trigger::Scheduled, UpdateError::Unreachable).phase,
            UpdatePhase::Idle
        );

        let loud = quiet_failure(Trigger::Manual, UpdateError::Unreachable);
        assert_eq!(loud.phase, UpdatePhase::Failed);
        assert!(loud.message.is_some());
    }

    #[test]
    fn an_offer_links_at_the_tag_the_release_workflow_creates() {
        // `docs/RELEASING.md`: the tag is `v` + the version in Cargo.toml.
        let status = UpdateStatus::offering(UpdatePhase::Available, "0.2.0".into());
        assert_eq!(
            status.release_url.as_deref(),
            Some("https://github.com/bartektricks/whisper-free/releases/tag/v0.2.0")
        );
    }

    #[test]
    fn a_fresh_status_is_idle_and_says_nothing() {
        let status = UpdateStatus::default();
        assert_eq!(status.phase, UpdatePhase::Idle);
        assert!(status.version.is_none());
        assert!(status.message.is_none());
    }
}
