//! The one file that may name `tauri_plugin_updater`.
//!
//! Everything outside `update/` knows `check` and `install` and an
//! [`UpdateError`]; nothing outside this file knows that a manifest, a
//! minisign signature or a tar archive is involved. A second update transport
//! would be another implementation here, not a change anywhere else — the same
//! boundary `asr/parakeet.rs` and `refine/onnx.rs` keep.

use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

use super::UpdateError;

/// What a check found, with the plugin's own types left behind.
pub(super) struct Found {
    pub version: String,
}

/// Ask the endpoint whether a newer version is published.
///
/// `Ok(None)` means this build is current — the plugin compares the manifest's
/// version against `app.package_info().version`, which for this crate is the
/// version in `Cargo.toml`.
pub(super) async fn check(app: &AppHandle) -> Result<Option<Found>, UpdateError> {
    // `updater()` rather than `updater_builder()`: the default builder already
    // installs an `on_before_exit` hook that runs Tauri's own
    // `cleanup_before_exit`, which is what takes the tray icon down before the
    // Windows installer kills the process. Supplying our own hook would
    // replace it rather than run alongside it.
    let updater = app.updater().map_err(classify)?;
    let found = updater.check().await.map_err(classify)?;
    Ok(found.map(|update| Found {
        version: update.version,
    }))
}

/// Download the published update and put it in place.
///
/// Checks again first. That costs one request on an action the user takes at
/// most a few times a year, and it buys not holding a plugin handle across a
/// mutex and an await — which is the only way the type could escape this file.
///
/// `on_progress` receives running bytes and the announced total, which the
/// server may never send; callers get `None` in that case.
///
/// Returns the version actually installed, which is the one the manifest
/// offered at this moment rather than the one the earlier check saw.
pub(super) async fn install<C: FnMut(u64, Option<u64>)>(
    app: &AppHandle,
    mut on_progress: C,
) -> Result<String, UpdateError> {
    let updater = app.updater().map_err(classify)?;
    let Some(update) = updater.check().await.map_err(classify)? else {
        // Published and then withdrawn between the check and the install.
        return Err(UpdateError::NotPublished);
    };
    let version = update.version.clone();

    let mut downloaded: u64 = 0;
    update
        .download_and_install(
            |chunk, total| {
                downloaded =
                    downloaded.saturating_add(u64::try_from(chunk).unwrap_or(u64::MAX));
                on_progress(downloaded, total);
            },
            || tracing::info!(event = "update_download_finished"),
        )
        .await
        .map_err(classify)?;

    Ok(version)
}

/// Sort a plugin failure into one of the handful of things a user can act on.
///
/// The plugin's error enum is `#[non_exhaustive]`, so the catch-all arm is
/// load-bearing rather than defensive padding: a variant added upstream must
/// land somewhere sensible instead of failing to compile in a patch release.
fn classify(error: tauri_plugin_updater::Error) -> UpdateError {
    use tauri_plugin_updater::Error as E;

    // The detail belongs in the log, and only in the log.
    tracing::warn!(event = "update_failed", error = %error);

    match error {
        E::Reqwest(_) | E::Network(_) | E::Http(_) => UpdateError::Unreachable,

        // Nothing published for this machine: either no release yet, or a
        // release whose manifest has no entry for this platform — which is
        // what a one-platform release build produces.
        E::ReleaseNotFound
        | E::TargetNotFound(_)
        | E::TargetsNotFound(_)
        | E::Serialization(_)
        | E::Semver(_) => UpdateError::NotPublished,

        // The download did not match the key this build was compiled against.
        E::Minisign(_) | E::Base64(_) | E::SignatureUtf8(_) => UpdateError::Signature,

        // macOS reports a non-writable install location this way: the plugin
        // falls back to an AppleScript admin prompt and turns a refusal into
        // `PermissionDenied`. Linux surfaces the same refusal as
        // `AuthenticationFailed`.
        E::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            UpdateError::PermissionRefused
        }
        E::AuthenticationFailed => UpdateError::PermissionRefused,

        E::Io(_)
        | E::TempDirNotFound
        | E::TempDirNotOnSameMountPoint
        | E::BinaryNotFoundInArchive
        | E::InvalidUpdaterFormat
        | E::PackageInstallFailed
        | E::DebInstallFailed => UpdateError::Install,

        // Configuration and platform-support failures. These cannot happen in
        // a build that shipped — `tauri.conf.json` carries the endpoint and
        // the key, and the app targets exactly two architectures — so they get
        // the generic message rather than advice nobody can follow.
        _ => UpdateError::Unavailable,
    }
}
