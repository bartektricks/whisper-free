//! User settings, persisted as JSON (plan §14).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::asr::LanguageSelection;
use crate::overlay::OverlayAnchor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    /// Record while the hotkey is held, transcribe on release.
    #[default]
    HoldToTalk,
    /// One press starts, the next press stops.
    Toggle,
}

/// How long a kept transcription stays kept (decision 0011).
///
/// [`Self::Session`] is the one that behaves differently in kind rather than in
/// degree: it never reaches the disk at all, so a user who wants the list
/// without a file in their home directory has that. The rest are windows, and
/// `HistoryRetention::cutoff` is the pure rule that turns them into a date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HistoryRetention {
    /// Kept in memory for this run only, and never written to disk.
    Session,
    OneDay,
    /// The default when history is switched on: long enough to find yesterday's
    /// paragraph, short enough that the file does not become an archive nobody
    /// meant to keep.
    #[default]
    SevenDays,
    ThirtyDays,
    Forever,
}

/// The model shipped as the default choice. Not downloaded until the user asks.
pub const DEFAULT_MODEL_ID: &str = "parakeet-tdt-0.6b-v3";

/// The refinement model offered by default. Also not downloaded until asked,
/// and unlike the speech model, not used until switched on either.
pub const DEFAULT_REFINE_MODEL_ID: &str = "s1-mini";

/// The refinement model this one replaced (decision 0012).
///
/// `Settings` is `#[serde(default)]`, so every install made before that change
/// still names it. Left alone, `models::find` would return `None`,
/// `sync_refiner` would quietly skip, and cleanup would stop working for
/// anyone who had it switched on, with the checkbox still ticked.
pub const RETIRED_REFINE_MODEL_ID: &str = "qwen2.5-0.5b-instruct";

/// The hotkey a fresh install starts with, in the accelerator syntax Tauri
/// understands. Chosen per platform, since a combination that is free on one
/// system is reserved on another.
pub const DEFAULT_HOTKEY: &str = crate::platform::default_hotkey();

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
// Independent checkboxes, not states of one thing: a user can want any
// combination of them, and the file is a flat mirror of the panel. Folding them
// into an enum or a bitfield would make `settings.json` harder to read by hand,
// which is the one debugging tool this file has.
#[allow(clippy::struct_excessive_bools)]
pub struct Settings {
    pub hotkey: String,
    pub recording_mode: RecordingMode,
    /// Input device name, or `None` for the system default.
    pub input_device: Option<String>,
    pub model_id: String,
    pub language: LanguageSelection,
    pub start_at_login: bool,
    /// Show the floating indicator while dictating. On by default: a menu-bar
    /// app that gives no sign it is recording reads as a broken hotkey.
    pub show_overlay: bool,
    pub overlay_anchor: OverlayAnchor,
    /// Silence the rest of the machine while the microphone is open
    /// (decision 0009).
    ///
    /// **Defaults to `true`, and that changes behaviour for existing
    /// installs.** It is meant to: this is an opt-out, so a settings file
    /// written before the feature existed has no such key and picks it up from
    /// `Default` at the next launch. One checkbox in Settings › Audio turns it
    /// off. Contrast `check_for_updates`, which is off by default because it
    /// reaches the network; this one only touches a volume the user can see.
    pub mute_while_recording: bool,
    /// Run transcriptions past a language model before pasting them
    /// (decision 0005). Off by default: it costs about a second per dictation
    /// and a second model in memory, and the loop works without it.
    pub refine_enabled: bool,
    /// Which refinement model to use, when enabled.
    pub refine_model_id: String,
    /// How much of the model's rewrite to accept (decision 0012).
    ///
    /// Not a model setting: S1-mini cleans a transcript the same way either
    /// way, and this decides how much of that cleanup survives the guard.
    pub refine_strength: RefineStrength,
    /// The register the cleaned text is written in.
    pub refine_styling: crate::refine::Styling,
    /// Ask GitHub once a day whether a newer version is published
    /// (decision 0006). Off by default, and load-bearing that it is: this is
    /// the only thing in the app that reaches the network without the user
    /// pressing a button for it, so it does not happen until they say so.
    pub check_for_updates: bool,
    /// Leave the transcription on the clipboard instead of putting back what
    /// was there before (decision 0011).
    ///
    /// Off by default, because it is the one setting here that takes something
    /// away: the clipboard the user was holding. On, the paste still happens
    /// and the words are also there to paste again.
    pub keep_on_clipboard: bool,
    /// Keep a local list of what was dictated (decision 0011).
    ///
    /// **Off by default, and unlike `mute_while_recording` it must stay that
    /// way.** This is the only thing in the app that writes transcription text
    /// to disk, so it is switched on by a person who has read what it does, not
    /// inherited from `Default` by an install that never asked for it.
    pub history_enabled: bool,
    /// How long kept transcriptions last, when the history is on.
    pub history_retention: HistoryRetention,
    /// Whether the user has been through first-run setup (decision 0007).
    ///
    /// **Defaults to `true`, deliberately.** A settings file written before
    /// onboarding existed has no such key, and serde fills the gap from
    /// `Default`, so a `false` default would march every established user
    /// through a tour of permissions they granted months ago. The single place
    /// it becomes `false` is a first run, where `setup` in `lib.rs` finds no
    /// settings file at all and writes one saying so.
    pub onboarding_completed: bool,
}

/// How much of a proposed cleanup to accept.
///
/// The stage is off by default either way; this is what someone who has
/// switched it on is choosing between.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefineStrength {
    /// Punctuation, capitalisation and a misheard word. Anything larger is
    /// thrown away, so fillers and false starts survive into the paste.
    LightTouch,
    /// The whole of what a normaliser does: fillers dropped, false starts
    /// resolved, numbers and dates written out. Still bounded: nothing the
    /// speaker did not say may appear.
    #[default]
    FullCleanup,
}

impl RefineStrength {
    /// The guard thresholds this strength judges by.
    #[must_use]
    pub const fn limits(self) -> crate::refine::Limits {
        match self {
            Self::LightTouch => crate::refine::Limits::light_touch(),
            Self::FullCleanup => crate::refine::Limits::full_cleanup(),
        }
    }
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
            show_overlay: true,
            overlay_anchor: OverlayAnchor::default(),
            mute_while_recording: true,
            refine_enabled: false,
            refine_model_id: DEFAULT_REFINE_MODEL_ID.to_string(),
            refine_strength: RefineStrength::default(),
            refine_styling: crate::refine::Styling::default(),
            check_for_updates: false,
            keep_on_clipboard: false,
            history_enabled: false,
            history_retention: HistoryRetention::default(),
            onboarding_completed: true,
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

        match serde_json::from_str::<Self>(&raw) {
            Ok(mut settings) => {
                settings.migrate();
                settings
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "settings file is invalid, using defaults");
                Self::default()
            }
        }
    }

    /// Bring a settings file written by an older version up to date.
    ///
    /// Not saved from here: the next `update_settings` writes it, and until
    /// then the in-memory value is the one everything reads. Keeping the write
    /// out of the load path means a read-only data directory still starts.
    fn migrate(&mut self) {
        if self.refine_model_id == RETIRED_REFINE_MODEL_ID {
            tracing::info!(
                event = "settings_migrated",
                from = RETIRED_REFINE_MODEL_ID,
                to = DEFAULT_REFINE_MODEL_ID,
                "the cleanup model was replaced"
            );
            self.refine_model_id = DEFAULT_REFINE_MODEL_ID.to_string();
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
        let dir = std::env::temp_dir().join(format!("whisperfree-test-{name}"));
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

    /// The overlay is the only feedback a menu-bar app gives while dictating,
    /// so a fresh install has to have it on.
    #[test]
    fn the_overlay_defaults_to_on_and_out_of_the_way() {
        let s = Settings::default();
        assert!(s.show_overlay);
        assert_eq!(s.overlay_anchor, OverlayAnchor::BottomCentre);
    }

    /// A settings file written before the overlay existed must not silently
    /// turn it off.
    #[test]
    fn a_file_from_before_the_overlay_still_enables_it() {
        let dir = temp_dir("pre-overlay");
        let path = settings_path(&dir);
        std::fs::write(&path, r#"{"hotkey":"Alt+D","start_at_login":true}"#).unwrap();
        let s = Settings::load(&path);
        assert!(s.show_overlay);
        assert_eq!(s.overlay_anchor, OverlayAnchor::BottomCentre);
    }

    /// The regression this default exists for: an install from before
    /// onboarding must not be sent through it on the next launch.
    #[test]
    fn a_file_from_before_onboarding_is_treated_as_already_onboarded() {
        let dir = temp_dir("pre-onboarding");
        let path = settings_path(&dir);
        std::fs::write(&path, r#"{"hotkey":"Alt+D","start_at_login":true}"#).unwrap();
        assert!(Settings::load(&path).onboarding_completed);
    }

    /// The opposite of the rule above, and deliberately so: muting is an
    /// opt-out, so a file written before it existed arrives with it switched
    /// on rather than being grandfathered out of it.
    #[test]
    fn a_file_from_before_muting_gets_it_switched_on() {
        let dir = temp_dir("pre-muting");
        let path = settings_path(&dir);
        std::fs::write(&path, r#"{"hotkey":"Alt+D","start_at_login":true}"#).unwrap();
        assert!(Settings::load(&path).mute_while_recording);
    }

    /// And someone who has turned it off keeps it off, which is the whole
    /// point of it being a setting.
    #[test]
    fn a_file_that_opts_out_of_muting_is_believed() {
        let dir = temp_dir("opted-out-of-muting");
        let path = settings_path(&dir);
        std::fs::write(&path, r#"{"mute_while_recording":false}"#).unwrap();
        assert!(!Settings::load(&path).mute_while_recording);
    }

    /// The other half of the same rule: once a first run has written the flag,
    /// quitting halfway through setup resumes it rather than skipping it.
    #[test]
    fn a_file_that_says_onboarding_is_pending_is_believed() {
        let dir = temp_dir("mid-onboarding");
        let path = settings_path(&dir);
        std::fs::write(&path, r#"{"onboarding_completed":false}"#).unwrap();
        assert!(!Settings::load(&path).onboarding_completed);
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
            show_overlay: false,
            overlay_anchor: OverlayAnchor::TopRight,
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

    #[test]
    fn a_settings_file_naming_the_retired_cleanup_model_is_migrated() {
        // Without this the id survives, `models::find` returns None,
        // `sync_refiner` skips, and cleanup stops working with the checkbox
        // still ticked, which looks like a bug in the feature rather than a
        // model that was replaced. See decision 0012.
        let dir = temp_dir("retired-refiner");
        let path = settings_path(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            format!(r#"{{"refine_enabled":true,"refine_model_id":"{RETIRED_REFINE_MODEL_ID}"}}"#),
        )
        .unwrap();

        let loaded = Settings::load(&path);
        assert_eq!(loaded.refine_model_id, DEFAULT_REFINE_MODEL_ID);
        // The rest of the file still applies: migrating is not resetting.
        assert!(loaded.refine_enabled);
    }

    #[test]
    fn a_settings_file_naming_a_current_model_is_left_alone() {
        let dir = temp_dir("kept-refiner");
        let path = settings_path(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"refine_model_id":"something-else"}"#).unwrap();
        assert_eq!(Settings::load(&path).refine_model_id, "something-else");
    }

    #[test]
    fn the_two_strengths_map_to_the_two_guard_rules() {
        assert_eq!(RefineStrength::LightTouch.limits(), crate::refine::Limits::light_touch());
        assert_eq!(RefineStrength::FullCleanup.limits(), crate::refine::Limits::full_cleanup());
        // Full cleanup is the default, because it is what someone switching the
        // stage on is switching it on *for*.
        assert_eq!(RefineStrength::default(), RefineStrength::FullCleanup);
    }
}
