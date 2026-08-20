//! Model metadata and on-disk installation (plan §7).
//!
//! Models are never bundled with the app (§23.13) and are never fetched
//! without the user asking. Every file is checked against a pinned SHA-256
//! before it is allowed to be used.

pub mod download;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::asr::{Capability, Language};

/// One file belonging to a model.
#[derive(Debug, Clone, Copy)]
pub struct ModelFile {
    /// Name on disk, inside the model's own directory.
    pub name: &'static str,
    /// Path under the descriptor's `base_url`.
    ///
    /// Usually the same as `name`, but not always: HuggingFace repositories
    /// keep ONNX exports in an `onnx/` subdirectory while the tokeniser sits at
    /// the root, and we flatten both into one directory locally.
    pub remote: &'static str,
    /// Pinned digest — the trust anchor for the download.
    pub sha256: &'static str,
    pub size_bytes: u64,
}

/// Which engine can run this model. Adding a second engine later means adding
/// a variant, not rewriting the manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    Parakeet,
    /// A decoder-only language model behind `refine::onnx` (decision 0005).
    RefinerOnnx,
}

/// What a model is for.
///
/// Speech models and refinement models share the whole download, verification
/// and storage path — they differ only in which slot they are loaded into and
/// which part of Settings lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    /// Turns audio into text.
    Speech,
    /// Checks over text the speech model produced.
    Refiner,
}

#[derive(Debug, Clone, Copy)]
pub struct ModelDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub description: &'static str,
    pub kind: ModelKind,
    pub engine: EngineKind,
    /// Where the files are fetched from, joined with each file's `remote`.
    pub base_url: &'static str,
    pub files: &'static [ModelFile],
    /// (code, English name) pairs.
    pub languages: &'static [(&'static str, &'static str)],
    pub capabilities: &'static [Capability],
}

impl ModelDescriptor {
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size_bytes).sum()
    }

    #[must_use]
    pub fn languages(&self) -> Vec<Language> {
        self.languages
            .iter()
            .map(|(code, name)| Language::new(*code, *name))
            .collect()
    }
}

/// The 25 languages Parakeet v3 was trained on, per the model card.
const PARAKEET_V3_LANGUAGES: &[(&str, &str)] = &[
    ("bg", "Bulgarian"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("hr", "Croatian"),
    ("hu", "Hungarian"),
    ("it", "Italian"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("mt", "Maltese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("sv", "Swedish"),
    ("uk", "Ukrainian"),
];

/// Parakeet v3 detects the language itself but cannot be pinned to one, which
/// is why these are two separate capabilities (see `asr::Capability`).
const PARAKEET_V3_CAPABILITIES: &[Capability] = &[
    Capability::LanguageDetection,
    Capability::Punctuation,
    Capability::Timestamps,
];

/// File names and layout are what `transcribe-rs` expects; digests were
/// verified against the HuggingFace repository (see decision 0001).
const PARAKEET_V3_FILES: &[ModelFile] = &[
    ModelFile {
        name: "encoder-model.int8.onnx",
        remote: "encoder-model.int8.onnx",
        sha256: "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09",
        size_bytes: 652_183_999,
    },
    ModelFile {
        name: "decoder_joint-model.int8.onnx",
        remote: "decoder_joint-model.int8.onnx",
        sha256: "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70",
        size_bytes: 18_202_004,
    },
    ModelFile {
        name: "nemo128.onnx",
        remote: "nemo128.onnx",
        sha256: "a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f",
        size_bytes: 139_764,
    },
    ModelFile {
        name: "vocab.txt",
        remote: "vocab.txt",
        sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
        size_bytes: 93_939,
    },
];

/// The refinement model (decision 0005).
///
/// Digests computed from the files the measurements in that decision were taken
/// against, and cross-checked against the sizes HuggingFace reports.
const QWEN25_05B_FILES: &[ModelFile] = &[
    ModelFile {
        name: "model_q4f16.onnx",
        remote: "onnx/model_q4f16.onnx",
        sha256: "b11c1dd99efd57e6c6e5bc4443a019931a5fbd5dd500d48644d8225f5ce0b2cb",
        size_bytes: 483_003_582,
    },
    ModelFile {
        name: "tokenizer.json",
        remote: "tokenizer.json",
        sha256: "a8506e7111b80c6d8635951a02eab0f4e1a8e4e5772da83846579e97b16f61bf",
        size_bytes: 7_031_673,
    },
];

/// A refiner declares no languages and no capabilities: it neither detects a
/// language nor offers anything the Speech settings can be pointed at.
const NO_LANGUAGES: &[(&str, &str)] = &[];
const NO_CAPABILITIES: &[Capability] = &[];

pub const AVAILABLE_MODELS: &[ModelDescriptor] = &[ModelDescriptor {
    id: "parakeet-tdt-0.6b-v3",
    name: "NVIDIA Parakeet TDT 0.6B v3",
    version: "3",
    description: "Fast and accurate, with automatic language detection and punctuation.",
    kind: ModelKind::Speech,
    engine: EngineKind::Parakeet,
    base_url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main",
    files: PARAKEET_V3_FILES,
    languages: PARAKEET_V3_LANGUAGES,
    capabilities: PARAKEET_V3_CAPABILITIES,
}, ModelDescriptor {
    id: "qwen2.5-0.5b-instruct",
    name: "Qwen2.5 0.5B Instruct",
    version: "2.5",
    description: "Checks transcriptions over and fixes misheard words. Adds about a second.",
    kind: ModelKind::Refiner,
    engine: EngineKind::RefinerOnnx,
    base_url: "https://huggingface.co/onnx-community/Qwen2.5-0.5B-Instruct/resolve/main",
    files: QWEN25_05B_FILES,
    languages: NO_LANGUAGES,
    capabilities: NO_CAPABILITIES,
}];

/// Models of one kind, for the two places that list them separately.
pub fn of_kind(kind: ModelKind) -> impl Iterator<Item = &'static ModelDescriptor> {
    AVAILABLE_MODELS.iter().filter(move |m| m.kind == kind)
}

#[must_use]
pub fn find(id: &str) -> Option<&'static ModelDescriptor> {
    AVAILABLE_MODELS.iter().find(|m| m.id == id)
}

/// What the UI needs to render one row of the model list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    /// What the model is for, so Settings can list speech and refinement
    /// models under separate headings.
    pub kind: ModelKind,
    pub size_bytes: u64,
    pub languages: Vec<Language>,
    pub installed: bool,
    /// Bytes already on disk, so an interrupted download can be described.
    pub bytes_on_disk: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("unknown model \"{0}\"")]
    Unknown(String),
    #[error("download failed: {0}")]
    Download(String),
    #[error("{file} failed its integrity check")]
    Checksum { file: String },
    #[error("could not write the model: {0}")]
    Io(String),
    #[error("download was cancelled")]
    Cancelled,
}

impl ModelError {
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::Unknown(_) => "That model is not one WhisperFree knows about.".into(),
            Self::Download(_) => {
                "The download failed. Check your internet connection and try again.".into()
            }
            Self::Checksum { .. } => {
                "The downloaded model failed its integrity check and was discarded. Try downloading it again."
                    .into()
            }
            Self::Io(_) => {
                "The model could not be saved. Check that there is enough free disk space.".into()
            }
            Self::Cancelled => "The download was cancelled.".into(),
        }
    }
}

/// Where models live on disk.
#[derive(Debug, Clone)]
pub struct ModelStore {
    root: PathBuf,
}

impl ModelStore {
    /// `<app data dir>/models`
    #[must_use]
    pub fn new(app_data_dir: &Path) -> Self {
        Self {
            root: app_data_dir.join("models"),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn dir_for(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    /// A model counts as installed only when every file is present at exactly
    /// the expected size. Size is a cheap proxy checked on every launch; the
    /// digest is verified at download time.
    #[must_use]
    pub fn is_installed(&self, descriptor: &ModelDescriptor) -> bool {
        let dir = self.dir_for(descriptor.id);
        descriptor
            .files
            .iter()
            .all(|f| std::fs::metadata(dir.join(f.name)).is_ok_and(|m| m.len() == f.size_bytes))
    }

    #[must_use]
    pub fn bytes_on_disk(&self, descriptor: &ModelDescriptor) -> u64 {
        let dir = self.dir_for(descriptor.id);
        descriptor
            .files
            .iter()
            .filter_map(|f| std::fs::metadata(dir.join(f.name)).ok())
            .map(|m| m.len())
            .sum()
    }

    #[must_use]
    pub fn info(&self, descriptor: &ModelDescriptor) -> ModelInfo {
        ModelInfo {
            id: descriptor.id.to_string(),
            kind: descriptor.kind,
            name: descriptor.name.to_string(),
            description: descriptor.description.to_string(),
            size_bytes: descriptor.total_bytes(),
            languages: descriptor.languages(),
            installed: self.is_installed(descriptor),
            bytes_on_disk: self.bytes_on_disk(descriptor),
        }
    }

    #[must_use]
    pub fn list(&self) -> Vec<ModelInfo> {
        AVAILABLE_MODELS.iter().map(|m| self.info(m)).collect()
    }

    /// Delete an installed model and everything under its directory.
    ///
    /// Removing a model that is not installed is not an error.
    ///
    /// # Errors
    ///
    /// [`ModelError::Io`] when the directory exists but cannot be deleted.
    pub fn remove(&self, id: &str) -> Result<(), ModelError> {
        let dir = self.dir_for(id);
        if !dir.exists() {
            return Ok(());
        }
        std::fs::remove_dir_all(&dir).map_err(|e| ModelError::Io(e.to_string()))?;
        tracing::info!(event = "model_removed", model_id = id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("whisperfree-models-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn parakeet() -> &'static ModelDescriptor {
        find("parakeet-tdt-0.6b-v3").unwrap()
    }

    #[test]
    fn the_default_model_is_in_the_registry() {
        let m = parakeet();
        assert_eq!(m.id, crate::settings::DEFAULT_MODEL_ID);
        assert_eq!(m.engine, EngineKind::Parakeet);
    }

    #[test]
    fn parakeet_declares_polish_and_english() {
        let codes: Vec<&str> = parakeet().languages.iter().map(|(c, _)| *c).collect();
        assert!(codes.contains(&"pl"));
        assert!(codes.contains(&"en"));
        assert_eq!(codes.len(), 25);
    }

    #[test]
    fn parakeet_detects_languages_but_cannot_be_pinned_to_one() {
        // The distinction matters: the UI must not offer a language picker
        // that the model would ignore.
        let caps = parakeet().capabilities;
        assert!(caps.contains(&Capability::LanguageDetection));
        assert!(!caps.contains(&Capability::LanguageSelection));
    }

    #[test]
    fn every_file_has_a_pinned_digest() {
        for model in AVAILABLE_MODELS {
            for file in model.files {
                assert_eq!(file.sha256.len(), 64, "{} has a malformed digest", file.name);
                assert!(file.sha256.chars().all(|c| c.is_ascii_hexdigit()));
                assert!(file.size_bytes > 0);
            }
        }
    }

    #[test]
    fn total_size_is_the_sum_of_the_files() {
        assert_eq!(parakeet().total_bytes(), 670_619_706);
    }

    #[test]
    fn a_model_with_no_files_on_disk_is_not_installed() {
        let store = ModelStore::new(&temp_dir("absent"));
        assert!(!store.is_installed(parakeet()));
        assert_eq!(store.bytes_on_disk(parakeet()), 0);
    }

    #[test]
    fn a_partially_downloaded_model_is_not_installed() {
        // A truncated file must never be treated as ready to load.
        let dir = temp_dir("partial");
        let store = ModelStore::new(&dir);
        let model_dir = store.dir_for(parakeet().id);
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("vocab.txt"), b"short").unwrap();

        assert!(!store.is_installed(parakeet()));
        assert_eq!(store.bytes_on_disk(parakeet()), 5);
    }

    #[test]
    fn a_model_with_every_file_at_the_right_size_is_installed() {
        let dir = temp_dir("complete");
        let store = ModelStore::new(&dir);
        let model_dir = store.dir_for(parakeet().id);
        std::fs::create_dir_all(&model_dir).unwrap();
        for f in parakeet().files {
            std::fs::write(model_dir.join(f.name), vec![0u8; f.size_bytes as usize]).unwrap();
        }
        assert!(store.is_installed(parakeet()));
    }

    #[test]
    fn removing_a_model_that_is_not_there_is_not_an_error() {
        let store = ModelStore::new(&temp_dir("remove-absent"));
        assert!(store.remove(parakeet().id).is_ok());
    }

    #[test]
    fn removing_deletes_the_files() {
        let dir = temp_dir("remove");
        let store = ModelStore::new(&dir);
        let model_dir = store.dir_for(parakeet().id);
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("vocab.txt"), b"x").unwrap();

        store.remove(parakeet().id).unwrap();
        assert!(!model_dir.exists());
    }

    #[test]
    fn error_messages_stay_readable() {
        for e in [
            ModelError::Download("connection reset by peer".into()),
            ModelError::Checksum {
                file: "encoder-model.int8.onnx".into(),
            },
            ModelError::Io("ENOSPC".into()),
        ] {
            let msg = e.user_message();
            assert!(!msg.contains("ENOSPC"), "leaked internals: {msg}");
            assert!(!msg.contains("peer"), "leaked internals: {msg}");
        }
    }
}
