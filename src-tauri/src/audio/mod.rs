//! Microphone capture (plan §9).
//!
//! Audio is held in memory and never written to disk. Recording is
//! record -> stop -> transcribe; there is no streaming path in v1 (§23.8).

pub mod capture;
pub mod resample;

use serde::{Deserialize, Serialize};

pub use capture::AudioEngine;

/// The sample rate the ASR layer expects, fixed by decision 0001.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Recordings shorter than this are almost certainly an accidental key tap
/// rather than speech.
pub const MIN_USEFUL_DURATION_MS: u64 = 200;

/// An input device the user can pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDevice {
    /// Stable identifier, persisted in settings. Survives reboots and
    /// reconnections, unlike the display name.
    pub id: String,
    /// Human-readable name, shown in the UI.
    pub name: String,
    /// True for the system default input.
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AudioError {
    #[error("microphone access has not been granted")]
    PermissionDenied,
    #[error("no microphone is available")]
    NoDevice,
    /// Carries the device id for the log; the id is not shown to the user.
    #[error("the microphone \"{0}\" is no longer available")]
    DeviceUnavailable(String),
    #[error("could not read the list of microphones: {0}")]
    DeviceQuery(String),
    #[error("could not start recording: {0}")]
    StreamStart(String),
    #[error("recording is already running")]
    AlreadyRecording,
    #[error("nothing is being recorded")]
    NotRecording,
    #[error("no audio was captured")]
    Empty,
    #[error("could not convert the audio: {0}")]
    Resample(String),
    #[error("the audio system stopped responding")]
    EngineGone,
}

impl AudioError {
    /// The message shown to the user (plan §17) — no internals, and where
    /// possible a hint about what to do next.
    pub fn user_message(&self) -> String {
        match self {
            AudioError::PermissionDenied => {
                "LocalDictation needs microphone access. Grant it in System Settings › Privacy & Security › Microphone, then try again."
                    .into()
            }
            AudioError::NoDevice => {
                "No microphone was found. Connect one and try again.".into()
            }
            AudioError::DeviceUnavailable(_) => {
                // The identifier is an opaque system string, so name the place
                // to fix it instead of showing the id.
                "The microphone you selected is no longer available. Choose another one in Settings › Audio."
                    .into()
            }
            AudioError::Empty => {
                "No audio was captured. Hold the hotkey a moment longer while you speak.".into()
            }
            AudioError::AlreadyRecording | AudioError::NotRecording => {
                "Recording got out of sync. Try again.".into()
            }
            AudioError::DeviceQuery(_)
            | AudioError::StreamStart(_)
            | AudioError::Resample(_)
            | AudioError::EngineGone => {
                "The microphone could not be started. Check that no other app is using it.".into()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_messages_never_leak_internals() {
        let errors = [
            AudioError::PermissionDenied,
            AudioError::NoDevice,
            AudioError::DeviceUnavailable("Yeti".into()),
            AudioError::DeviceQuery("CoreAudio error 560227702".into()),
            AudioError::StreamStart("BuildStreamError::DeviceNotAvailable".into()),
            AudioError::Resample("ResamplerConstructionError".into()),
            AudioError::EngineGone,
            AudioError::Empty,
        ];
        for e in errors {
            let msg = e.user_message();
            assert!(!msg.is_empty());
            // Raw technical detail must stay in the log, not the UI.
            assert!(!msg.contains("CoreAudio"), "leaked internals: {msg}");
            assert!(!msg.contains("Error::"), "leaked internals: {msg}");
            assert!(!msg.contains("Resampler"), "leaked internals: {msg}");
        }
    }

    #[test]
    fn a_disconnected_device_points_at_the_setting_rather_than_its_raw_id() {
        let msg =
            AudioError::DeviceUnavailable("CoreAudio:AppleUSBAudioEngine:12345".into())
                .user_message();
        assert!(!msg.contains("AppleUSBAudioEngine"), "leaked raw id: {msg}");
        assert!(msg.contains("Settings"));
    }
}
