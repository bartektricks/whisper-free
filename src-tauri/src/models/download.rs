//! Downloading and verifying model files.
//!
//! Nothing here runs unless the user pressed Download. Each file is streamed to
//! a `.part` file, hashed as it arrives, and only moved into place once the
//! digest matches — so an interrupted or corrupted download can never leave
//! something loadable behind.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{ModelDescriptor, ModelError, ModelFile, ModelStore};

/// Progress across a whole model, not just the current file — that is what a
/// progress bar needs.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub model_id: String,
    /// File currently being fetched.
    pub file: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
}

impl DownloadProgress {
    /// Completed fraction in `0.0..=1.0`, clamped so a server that sends more
    /// than it advertised cannot push a progress bar past the end.
    #[must_use]
    pub fn fraction(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        // `u64` has no lossless conversion to `f64`, and none is needed: model
        // files are a few hundred megabytes, far below the 2^53 where integers
        // stop being exact, and the result only ever drives a progress bar.
        #[allow(clippy::as_conversions, clippy::cast_precision_loss)]
        let fraction = self.downloaded_bytes as f64 / self.total_bytes as f64;
        fraction.min(1.0)
    }
}

/// Cooperative cancellation, checked between read chunks.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Compute the SHA-256 of a file already on disk.
///
/// # Errors
///
/// [`ModelError::Io`] when the file cannot be opened or read.
pub fn file_digest(path: &Path) -> Result<String, ModelError> {
    let mut file = std::fs::File::open(path).map_err(|e| ModelError::Io(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| ModelError::Io(e.to_string()))?;
        let Some(chunk) = buf.get(..n) else { break };
        if chunk.is_empty() {
            break;
        }
        hasher.update(chunk);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Download every file of `descriptor` that is not already present and valid.
///
/// `on_progress` is called as bytes arrive, with totals across the whole model.
///
/// # Errors
///
/// [`ModelError::Download`] when a transfer fails, [`ModelError::Checksum`]
/// when a file does not match its pinned digest, [`ModelError::Io`] when it
/// cannot be written, and [`ModelError::Cancelled`] when `cancel` is raised.
pub fn install(
    store: &ModelStore,
    descriptor: &ModelDescriptor,
    cancel: &CancelFlag,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<(), ModelError> {
    let dir = store.dir_for(descriptor.id);
    std::fs::create_dir_all(&dir).map_err(|e| ModelError::Io(e.to_string()))?;

    let total_bytes = descriptor.total_bytes();
    let mut completed_bytes = 0u64;

    tracing::info!(
        event = "model_download_started",
        model_id = descriptor.id,
        total_bytes
    );

    for file in descriptor.files {
        let target = dir.join(file.name);

        // Already there at the right size — trust the size check here rather
        // than re-hashing 650 MB on every launch.
        if std::fs::metadata(&target).is_ok_and(|m| m.len() == file.size_bytes) {
            completed_bytes = completed_bytes.saturating_add(file.size_bytes);
            on_progress(DownloadProgress {
                model_id: descriptor.id.to_string(),
                file: file.name.to_string(),
                downloaded_bytes: completed_bytes,
                total_bytes,
            });
            continue;
        }

        let url = format!("{}/{}", descriptor.base_url, file.remote);
        download_one(
            &url,
            &target,
            file,
            cancel,
            completed_bytes,
            total_bytes,
            descriptor.id,
            &mut on_progress,
        )?;
        completed_bytes = completed_bytes.saturating_add(file.size_bytes);
    }

    tracing::info!(event = "model_download_completed", model_id = descriptor.id);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn download_one(
    url: &str,
    target: &Path,
    file: &ModelFile,
    cancel: &CancelFlag,
    bytes_before: u64,
    total_bytes: u64,
    model_id: &str,
    on_progress: &mut impl FnMut(DownloadProgress),
) -> Result<(), ModelError> {
    let part = target.with_extension("part");

    let mut response = ureq::get(url)
        .call()
        .map_err(|e| ModelError::Download(e.to_string()))?;

    let mut reader = response.body_mut().as_reader();
    let mut out = std::fs::File::create(&part).map_err(|e| ModelError::Io(e.to_string()))?;

    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut written = 0u64;
    let mut last_reported = 0u64;

    loop {
        if cancel.is_cancelled() {
            let _ = std::fs::remove_file(&part);
            return Err(ModelError::Cancelled);
        }

        let n = reader
            .read(&mut buf)
            .map_err(|e| ModelError::Download(e.to_string()))?;
        let Some(chunk) = buf.get(..n) else { break };
        if chunk.is_empty() {
            break;
        }

        hasher.update(chunk);
        out.write_all(chunk)
            .map_err(|e| ModelError::Io(e.to_string()))?;
        written = written.saturating_add(n.try_into().unwrap_or(u64::MAX));

        // Roughly every 2 MB, so the UI updates without being flooded.
        if written.saturating_sub(last_reported) >= 2 << 20 {
            last_reported = written;
            on_progress(DownloadProgress {
                model_id: model_id.to_string(),
                file: file.name.to_string(),
                downloaded_bytes: bytes_before.saturating_add(written),
                total_bytes,
            });
        }
    }

    out.flush().map_err(|e| ModelError::Io(e.to_string()))?;
    drop(out);

    let digest = hex(&hasher.finalize());
    if digest != file.sha256 {
        // Never leave a file that failed verification where it could be loaded.
        let _ = std::fs::remove_file(&part);
        tracing::error!(
            event = "model_checksum_failed",
            model_id = model_id,
            file = file.name
        );
        return Err(ModelError::Checksum {
            file: file.name.to_string(),
        });
    }

    std::fs::rename(&part, target).map_err(|e| ModelError::Io(e.to_string()))?;

    on_progress(DownloadProgress {
        model_id: model_id.to_string(),
        file: file.name.to_string(),
        downloaded_bytes: bytes_before.saturating_add(written),
        total_bytes,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_encodes_lowercase_and_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
    }

    #[test]
    fn digest_of_a_known_file_matches_sha256() {
        let path = std::env::temp_dir().join("whisperfree-digest-test.txt");
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            file_digest(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn progress_fraction_is_clamped_and_safe_at_zero() {
        let p = |downloaded, total| DownloadProgress {
            model_id: "m".into(),
            file: "f".into(),
            downloaded_bytes: downloaded,
            total_bytes: total,
        };
        assert_eq!(p(0, 0).fraction(), 0.0);
        assert_eq!(p(50, 100).fraction(), 0.5);
        // A server that sends more than advertised must not produce >100%.
        assert_eq!(p(150, 100).fraction(), 1.0);
    }

    #[test]
    fn cancel_flag_starts_clear_and_latches() {
        let flag = CancelFlag::new();
        assert!(!flag.is_cancelled());
        flag.cancel();
        assert!(flag.is_cancelled());
    }

    #[test]
    fn a_cancel_flag_clone_shares_the_same_state() {
        // The download thread holds a clone; cancelling from the UI must reach it.
        let flag = CancelFlag::new();
        let clone = flag.clone();
        flag.cancel();
        assert!(clone.is_cancelled());
    }
}
