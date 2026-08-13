//! User settings, persisted as JSON (plan §14).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::asr::LanguageSelection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    /// Record while the hotkey is held, transcribe on release.
    #[default]
    HoldToTalk,
    /// One press starts, the next press stops.
    Toggle,
}

/// The model shipped as the default choice. Not downloaded until the user asks.
pub const DEFAULT_MODEL_ID: &str = "parakeet-tdt-0.6b-v3";

/// The hotkey a fresh install starts with, in the accelerator syntax Tauri
/// understands. Chosen per platform, since a combination that is free on one
/// system is reserved on another.
pub const DEFAULT_HOTKEY: &str = crate::platform::default_hotkey();

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub hotkey: String,
    pub recording_mode: RecordingMode,
    /// Input device name, or `None` for the system default.
    pub input_device: Option<String>,
    pub model_id: String,
    pub language: LanguageSelection,
    pub start_at_login: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_HOTKEY.to_string(),
            recording_mode: RecordingMode::default(),
            input_device: None,
            model_id: DEFAULT_MODEL_ID.to_string(),
            language: LanguageSelection::Auto,
            start_at_login: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("could not write settings: {0}")]
    Write(#[from] std::io::Error),
    #[error("could not encode settings: {0}")]
    Encode(#[from] serde_json::Error),
}

impl Settings {
    /// Read settings from `path`.
    ///
    /// A missing file yields defaults. So does an unreadable or corrupt one:
    /// losing preferences is annoying, but refusing to start is worse, so a bad
    /// file is logged and replaced rather than treated as fatal.
    pub fn load(path: &Path) -> Self {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(path = %path.display(), "no settings file yet, using defaults");
                return Self::default();
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "could not read settings, using defaults");
                return Self::default();
            }
        };

        match serde_json::from_str(&raw) {
            Ok(settings) => settings,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "settings file is invalid, using defaults");
                Self::default()
            }
        }
    }

    /// Write settings to `path`, creating parent directories as needed.
    ///
    /// Writes to a temporary file and renames it over the target, so an
    /// interrupted write cannot leave a half-written settings file behind.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Write`] when the file cannot be created or renamed,
    /// [`SettingsError::Encode`] when serialisation fails.
    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;

        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Where settings live inside the application support directory.
#[must_use]
pub fn settings_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("localdictation-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn defaults_are_hold_to_talk_auto_language() {
        let s = Settings::default();
        assert_eq!(s.recording_mode, RecordingMode::HoldToTalk);
        assert_eq!(s.language, LanguageSelection::Auto);
        assert_eq!(s.hotkey, DEFAULT_HOTKEY);
        assert!(!s.start_at_login);
        assert_eq!(s.input_device, None);
    }

    #[test]
    fn missing_file_yields_defaults() {
        let dir = temp_dir("missing");
        let s = Settings::load(&settings_path(&dir));
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn saves_and_loads_round_trip() {
        let dir = temp_dir("roundtrip");
        let path = settings_path(&dir);

        let s = Settings {
            recording_mode: RecordingMode::Toggle,
            hotkey: "Ctrl+Shift+D".into(),
            language: LanguageSelection::Fixed("pl".into()),
            input_device: Some("MacBook Pro Microphone".into()),
            start_at_login: true,
            ..Settings::default()
        };
        s.save(&path).unwrap();

        assert_eq!(Settings::load(&path), s);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults_instead_of_failing() {
        let dir = temp_dir("corrupt");
        let path = settings_path(&dir);
        std::fs::write(&path, "{ this is not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }

    #[test]
    fn unknown_fields_from_a_future_version_are_ignored() {
        let dir = temp_dir("unknown");
        let path = settings_path(&dir);
        std::fs::write(&path, r#"{"hotkey":"Alt+D","future_option":42}"#).unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.hotkey, "Alt+D");
        // Everything absent falls back to its default.
        assert_eq!(s.recording_mode, RecordingMode::HoldToTalk);
    }

    #[test]
    fn partial_file_keeps_defaults_for_missing_fields() {
        let dir = temp_dir("partial");
        let path = settings_path(&dir);
        std::fs::write(&path, r#"{"start_at_login":true}"#).unwrap();
        let s = Settings::load(&path);
        assert!(s.start_at_login);
        assert_eq!(s.model_id, DEFAULT_MODEL_ID);
    }

    #[test]
    fn save_creates_missing_directories() {
        let dir = temp_dir("nested").join("a").join("b");
        let path = settings_path(&dir);
        Settings::default().save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_leaves_no_temporary_file_behind() {
        let dir = temp_dir("tmpfile");
        let path = settings_path(&dir);
        Settings::default().save(&path).unwrap();
        assert!(!path.with_extension("json.tmp").exists());
    }
}
