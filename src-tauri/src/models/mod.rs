//! Model metadata and on-disk installation (plan §7).
//!
//! Models are never bundled with the app (§23.13) and are never fetched
//! without the user asking. Every file is checked against a pinned SHA-256
//! before it is allowed to be used.

pub mod download;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::asr::{Capability, Language, LanguageSelection};

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
    /// Where this one file comes from, when that is not the descriptor's
    /// `base_url`.
    ///
    /// Canary needs `NeMo`'s 128-mel preprocessor (`nemo128.onnx`) but its own
    /// repositories do not ship one, so it borrows the byte-identical file
    /// from the Parakeet repository. Two models then share a digest for the
    /// same file, which is the point: it is the same preprocessor.
    pub base_url: Option<&'static str>,
}

impl ModelFile {
    /// The URL to fetch this file from, given the descriptor it belongs to.
    #[must_use]
    pub fn url(&self, descriptor_base: &str) -> String {
        format!("{}/{}", self.base_url.unwrap_or(descriptor_base), self.remote)
    }
}

/// Which engine can run this model. Adding a second engine later means adding
/// a variant, not rewriting the manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    Parakeet,
    /// NVIDIA Canary, an attention encoder-decoder. The counterpart to
    /// Parakeet rather than a replacement: it must be told which language it
    /// is listening to, and cannot work it out for itself.
    Canary,
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

/// The 25 European languages Parakeet v3 and Canary 1B v2 were both trained
/// on. The two lists are the same set, so they are the same constant: a model
/// that declared its own copy would drift from the other for no reason.
const EUROPEAN_25_LANGUAGES: &[(&str, &str)] = &[
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
        base_url: None,
    },
    ModelFile {
        name: "decoder_joint-model.int8.onnx",
        remote: "decoder_joint-model.int8.onnx",
        sha256: "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70",
        size_bytes: 18_202_004,
        base_url: None,
    },
    ModelFile {
        name: "nemo128.onnx",
        remote: "nemo128.onnx",
        sha256: "a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f",
        size_bytes: 139_764,
        base_url: None,
    },
    ModelFile {
        name: "vocab.txt",
        remote: "vocab.txt",
        sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
        size_bytes: 93_939,
        base_url: None,
    },
];

/// What Canary 180M Flash is offered for.
///
/// NVIDIA lists four languages and `transcribe-rs` agrees, but Spanish decodes
/// to an empty string on this export: three clips of different lengths, all
/// empty, where Parakeet transcribed the same audio fine and this same model
/// handled English, German and French. An empty transcription is a surfaced
/// failure by design, so offering Spanish here would be offering a language
/// that reliably fails. Declared languages are the ones that were measured to
/// work, not the ones the model card claims.
const CANARY_FLASH_LANGUAGES: &[(&str, &str)] = &[
    ("de", "German"),
    ("en", "English"),
    ("fr", "French"),
];

/// The exact opposite of Parakeet's pair: Canary is told which language it is
/// listening to and never guesses, so it declares `LanguageSelection` and not
/// `LanguageDetection`. `asr::check_language_request` is what turns that into
/// a refusal rather than a silently ignored setting.
const CANARY_CAPABILITIES: &[Capability] =
    &[Capability::LanguageSelection, Capability::Punctuation];

/// Where Canary borrows its mel preprocessor from.
///
/// `nemo128.onnx` is `NeMo`'s 128-band filterbank, shared by every model in the
/// family, and istupakov's Canary exports leave it out. The digest below is
/// the same one `PARAKEET_V3_FILES` pins, because it is the same file.
const NEMO_PREPROCESSOR_BASE_URL: &str =
    "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main";

const NEMO128_PREPROCESSOR: ModelFile = ModelFile {
    name: "nemo128.onnx",
    remote: "nemo128.onnx",
    sha256: "a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f",
    size_bytes: 139_764,
    base_url: Some(NEMO_PREPROCESSOR_BASE_URL),
};

/// Digests are the LFS object ids HuggingFace reports for the repository,
/// which are SHA-256 of the file contents; `vocab.txt` is not stored in LFS
/// and was hashed directly.
const CANARY_1B_V2_FILES: &[ModelFile] = &[
    ModelFile {
        name: "encoder-model.int8.onnx",
        remote: "encoder-model.int8.onnx",
        sha256: "6d96e9945898e5ace48f4efecd459ca1df81859730be27b8af6b197639403ee1",
        size_bytes: 859_078_138,
        base_url: None,
    },
    ModelFile {
        name: "decoder-model.int8.onnx",
        remote: "decoder-model.int8.onnx",
        sha256: "52d83aa7aad41fbbe4f9dfcd341d784735a6eb4c6eb0d3290fc27a0d8ac39abf",
        size_bytes: 170_040_374,
        base_url: None,
    },
    ModelFile {
        name: "vocab.txt",
        remote: "vocab.txt",
        sha256: "2c9efe6104fd29522ea27ce0e3aef5d37c690af4e5a4232e643e23ca403ffea3",
        size_bytes: 208_022,
        base_url: None,
    },
    NEMO128_PREPROCESSOR,
];

const CANARY_180M_FLASH_FILES: &[ModelFile] = &[
    ModelFile {
        name: "encoder-model.int8.onnx",
        remote: "encoder-model.int8.onnx",
        sha256: "996d1c89e6cbc891a7c88bf410884c178ffa474f7b13084522ac74a5e144cc81",
        size_bytes: 133_710_896,
        base_url: None,
    },
    ModelFile {
        name: "decoder-model.int8.onnx",
        remote: "decoder-model.int8.onnx",
        sha256: "9dd9c447872088c912e916d73751f9621a54085d5bc46788454fe904db51a914",
        size_bytes: 79_520_211,
        base_url: None,
    },
    ModelFile {
        name: "vocab.txt",
        remote: "vocab.txt",
        sha256: "2dae6fc7815f9640645e0c765522b278ee0cef49b482d91f6913e334628d3e77",
        size_bytes: 53_555,
        base_url: None,
    },
    NEMO128_PREPROCESSOR,
];

/// The refinement model (decision 0012, replacing 0005's Qwen2.5).
///
/// Digests computed from the files the measurements in that decision were taken
/// against, and cross-checked against the sizes HuggingFace reports.
///
/// The first model in the registry whose weights do not fit in one file. ONNX
/// Runtime finds `model_q4.onnx_data` by the location string recorded inside
/// the graph, resolved next to the `.onnx`, so the local name has to be exactly
/// that and `ModelStore::dir_for` flattening every file into one directory is
/// what makes it work.
const S1_MINI_FILES: &[ModelFile] = &[
    ModelFile {
        name: "model_q4.onnx",
        remote: "onnx/model_q4.onnx",
        sha256: "be5f0d8d03ac387bdd2d2582e4e114ca3c23a44b70bf03be609844542107745c",
        size_bytes: 369_635,
        base_url: None,
    },
    ModelFile {
        name: "model_q4.onnx_data",
        remote: "onnx/model_q4.onnx_data",
        sha256: "85bcddf9b558e4881215c32652bc9345672530d77a432d2aee7f2e0c1ee62869",
        size_bytes: 403_007_488,
        base_url: None,
    },
    ModelFile {
        name: "tokenizer.json",
        remote: "tokenizer.json",
        sha256: "40ae5d1ee027b985684a3bbeef4ee16b2b5697d1d90658bec5bc5d2a73018bd7",
        size_bytes: 9_117_036,
        base_url: None,
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
    languages: EUROPEAN_25_LANGUAGES,
    capabilities: PARAKEET_V3_CAPABILITIES,
}, ModelDescriptor {
    id: "canary-1b-v2",
    name: "NVIDIA Canary 1B v2",
    version: "2",
    description: "More accurate than Parakeet, and slower. You choose the language rather than it guessing.",
    kind: ModelKind::Speech,
    engine: EngineKind::Canary,
    base_url: "https://huggingface.co/istupakov/canary-1b-v2-onnx/resolve/main",
    files: CANARY_1B_V2_FILES,
    languages: EUROPEAN_25_LANGUAGES,
    capabilities: CANARY_CAPABILITIES,
}, ModelDescriptor {
    id: "canary-180m-flash",
    name: "NVIDIA Canary 180M Flash",
    version: "1",
    description: "A fifth the download and the quickest to run, for English, German or French.",
    kind: ModelKind::Speech,
    engine: EngineKind::Canary,
    base_url: "https://huggingface.co/istupakov/canary-180m-flash-onnx/resolve/main",
    files: CANARY_180M_FLASH_FILES,
    languages: CANARY_FLASH_LANGUAGES,
    capabilities: CANARY_CAPABILITIES,
}, ModelDescriptor {
    // "S1-mini" by "Superwhisper", with that exact capitalisation: the licence
    // requires the model keep its name wherever it is used, so this string and
    // the Settings copy that shows it are not free to be prettified.
    id: "s1-mini",
    name: "S1-mini",
    version: "1",
    description: "Superwhisper's transcript cleaner. Drops fillers, fixes false starts, and writes out numbers and dates. English only.",
    kind: ModelKind::Refiner,
    engine: EngineKind::RefinerOnnx,
    base_url: "https://huggingface.co/onnx-community/s1-mini-ONNX/resolve/main",
    files: S1_MINI_FILES,
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

/// The language a model falls back to when the user's choice cannot be kept.
///
/// English if the model has it, and the first language it declares otherwise —
/// never nothing, because a model that must be pinned has no valid "unset".
fn fallback_language(descriptor: &ModelDescriptor) -> Option<String> {
    descriptor
        .languages
        .iter()
        .find(|(code, _)| *code == "en")
        .or_else(|| descriptor.languages.first())
        .map(|(code, _)| (*code).to_string())
}

/// Bring a language selection into line with what `descriptor` can actually do.
///
/// One `settings.language` has to serve models that disagree about what
/// choosing a language even means: Parakeet detects and refuses to be pinned,
/// Canary is pinned and cannot detect. `asr::check_language_request` turns a
/// mismatch into a failed dictation, which is right at the point of use and
/// far too late in Settings — so switching model carries the selection across
/// to the nearest thing the new model can honour.
///
/// Pure, and tested per capability pairing: this is the rule, not a UI detail.
#[must_use]
pub fn normalise_language(
    descriptor: &ModelDescriptor,
    selection: LanguageSelection,
) -> LanguageSelection {
    let detects = descriptor
        .capabilities
        .contains(&Capability::LanguageDetection);
    let pins = descriptor
        .capabilities
        .contains(&Capability::LanguageSelection);

    match selection {
        // Asking for detection from a model that cannot detect: pin it to
        // something it does speak rather than leave every dictation failing.
        LanguageSelection::Auto if !detects && pins => fallback_language(descriptor)
            .map_or(LanguageSelection::Auto, LanguageSelection::Fixed),
        LanguageSelection::Auto => LanguageSelection::Auto,
        // A pinned language a model cannot honour. Detection is the better
        // answer where it exists, since it covers whatever they picked.
        LanguageSelection::Fixed(_) if !pins => LanguageSelection::Auto,
        LanguageSelection::Fixed(code) => {
            if descriptor.languages.iter().any(|(c, _)| *c == code) {
                LanguageSelection::Fixed(code)
            } else {
                fallback_language(descriptor)
                    .map_or(LanguageSelection::Auto, LanguageSelection::Fixed)
            }
        }
    }
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
    /// What the model can do, so Settings › Speech knows whether to offer a
    /// language picker, the word "Automatic", or neither.
    pub capabilities: Vec<Capability>,
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
/// Total size of the files directly inside a directory.
fn directory_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .filter(std::fs::Metadata::is_file)
            .map(|m| m.len())
            .fold(0, u64::saturating_add)
    })
}

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
            capabilities: descriptor.capabilities.to_vec(),
            installed: self.is_installed(descriptor),
            bytes_on_disk: self.bytes_on_disk(descriptor),
        }
    }

    #[must_use]
    pub fn list(&self) -> Vec<ModelInfo> {
        AVAILABLE_MODELS.iter().map(|m| self.info(m)).collect()
    }

    /// Delete an installed model and everything under its directory.
    /// Delete downloads for models the registry no longer offers.
    ///
    /// A retired model's files are unreachable rather than merely unused: with
    /// no descriptor naming them, `list` cannot show the model and `remove`
    /// cannot be reached from Settings, so half a gigabyte would sit there for
    /// good. Decision 0012 retired one, which is why this exists.
    ///
    /// Only ever deletes whole directories under the models root, and only ones
    /// no descriptor claims. Model files are a cache of something
    /// re-downloadable, never user data.
    pub fn remove_retired(&self) -> u64 {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return 0;
        };

        let mut freed: u64 = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if AVAILABLE_MODELS.iter().any(|m| m.id == name) {
                continue;
            }

            let bytes = directory_bytes(&path);
            match std::fs::remove_dir_all(&path) {
                Ok(()) => {
                    freed = freed.saturating_add(bytes);
                    tracing::info!(event = "retired_model_removed", model_id = name, bytes);
                }
                Err(e) => {
                    tracing::warn!(event = "retired_model_kept", model_id = name, error = %e);
                }
            }
        }
        freed
    }

    /// Delete a model's directory.
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

    #[test]
    fn a_retired_models_download_is_reclaimed_and_current_ones_are_kept() {
        // Decision 0012 dropped a descriptor, which strands its files: with
        // nothing naming them, Settings can neither list nor remove them.
        let root = temp_dir("retired");
        let store = ModelStore::new(&root);

        let retired = store.dir_for("qwen2.5-0.5b-instruct");
        std::fs::create_dir_all(&retired).unwrap();
        std::fs::write(retired.join("model_q4f16.onnx"), vec![0_u8; 2_048]).unwrap();

        let current = store.dir_for("s1-mini");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("model_q4.onnx"), vec![0_u8; 512]).unwrap();

        let freed = store.remove_retired();

        assert_eq!(freed, 2_048, "should report what it reclaimed");
        assert!(!retired.exists(), "the retired download should be gone");
        assert!(current.exists(), "a model still in the registry must be kept");
    }

    #[test]
    fn reclaiming_is_a_no_op_when_there_is_nothing_retired() {
        let root = temp_dir("retired-none");
        let store = ModelStore::new(&root);
        std::fs::create_dir_all(store.dir_for("s1-mini")).unwrap();
        assert_eq!(store.remove_retired(), 0);
        assert!(store.dir_for("s1-mini").exists());
    }

    fn parakeet() -> &'static ModelDescriptor {
        find("parakeet-tdt-0.6b-v3").unwrap()
    }

    fn canary() -> &'static ModelDescriptor {
        find("canary-1b-v2").unwrap()
    }

    fn flash() -> &'static ModelDescriptor {
        find("canary-180m-flash").unwrap()
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
    fn canary_can_be_pinned_to_a_language_but_cannot_detect_one() {
        // The exact mirror of Parakeet, and the reason a single
        // `settings.language` needs normalising when the model changes.
        for m in [canary(), flash()] {
            assert!(m.capabilities.contains(&Capability::LanguageSelection), "{}", m.id);
            assert!(!m.capabilities.contains(&Capability::LanguageDetection), "{}", m.id);
        }
    }

    #[test]
    fn every_speech_model_declares_at_least_one_language_it_can_be_told_to_use() {
        // A model that must be pinned and lists nothing to pin it to would
        // leave `normalise_language` with no valid answer at all.
        for m in of_kind(ModelKind::Speech) {
            if m.capabilities.contains(&Capability::LanguageSelection) {
                assert!(!m.languages.is_empty(), "{} has nothing to pin to", m.id);
                assert!(fallback_language(m).is_some(), "{}", m.id);
            }
        }
    }

    #[test]
    fn canary_borrows_the_preprocessor_from_the_parakeet_repository() {
        // istupakov's Canary exports leave `nemo128.onnx` out, so the file is
        // fetched from elsewhere — and it must be the *same* file, or the mel
        // features feeding the encoder are not the ones it was exported for.
        let borrowed = canary()
            .files
            .iter()
            .find(|f| f.name == "nemo128.onnx")
            .expect("canary needs a preprocessor");
        let original = parakeet()
            .files
            .iter()
            .find(|f| f.name == "nemo128.onnx")
            .unwrap();

        assert_eq!(borrowed.sha256, original.sha256);
        assert_eq!(borrowed.size_bytes, original.size_bytes);
        assert!(borrowed.base_url.is_some(), "would resolve against canary's repo");
        assert_eq!(borrowed.url(canary().base_url), original.url(parakeet().base_url));
    }

    #[test]
    fn a_file_without_an_override_resolves_against_its_own_model() {
        let encoder = canary()
            .files
            .iter()
            .find(|f| f.name == "encoder-model.int8.onnx")
            .unwrap();
        assert!(encoder.url(canary().base_url).starts_with(canary().base_url));
    }

    #[test]
    fn model_ids_are_unique() {
        // Two descriptors sharing an id would share a directory on disk, and
        // `is_installed` compares sizes, so the second would look installed.
        let mut ids: Vec<&str> = AVAILABLE_MODELS.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    #[test]
    fn switching_to_a_model_that_cannot_detect_pins_a_language_instead() {
        // Saving `Auto` against Canary verbatim would leave every dictation
        // failing a capability check with no sign of why in Settings.
        let pinned = normalise_language(canary(), LanguageSelection::Auto);
        assert_eq!(pinned, LanguageSelection::Fixed("en".into()));
    }

    #[test]
    fn switching_to_a_model_that_cannot_be_pinned_goes_back_to_detection() {
        let selection = normalise_language(parakeet(), LanguageSelection::Fixed("pl".into()));
        assert_eq!(selection, LanguageSelection::Auto);
    }

    #[test]
    fn a_language_the_new_model_does_not_speak_falls_back_rather_than_failing() {
        // Canary 1B v2 has Polish; the 180M Flash has four languages and none
        // of them is Polish.
        assert_eq!(
            normalise_language(canary(), LanguageSelection::Fixed("pl".into())),
            LanguageSelection::Fixed("pl".into())
        );
        assert_eq!(
            normalise_language(flash(), LanguageSelection::Fixed("pl".into())),
            LanguageSelection::Fixed("en".into())
        );
    }

    #[test]
    fn normalising_twice_changes_nothing_the_second_time() {
        // `update_settings` runs this on every save, not just on a model
        // change, so it has to be a fixed point or it would fight the user.
        for m in of_kind(ModelKind::Speech) {
            for start in [
                LanguageSelection::Auto,
                LanguageSelection::Fixed("pl".into()),
                LanguageSelection::Fixed("qq".into()),
            ] {
                let once = normalise_language(m, start);
                let twice = normalise_language(m, once.clone());
                assert_eq!(once, twice, "{}", m.id);
            }
        }
    }

    #[test]
    fn a_normalised_selection_is_one_the_model_would_accept() {
        // The contract between this and `asr::check_language_request`: what
        // comes out of here must never be what that function refuses.
        for m in of_kind(ModelKind::Speech) {
            match normalise_language(m, LanguageSelection::Auto) {
                LanguageSelection::Auto => {
                    assert!(m.capabilities.contains(&Capability::LanguageDetection), "{}", m.id);
                }
                LanguageSelection::Fixed(code) => {
                    assert!(m.capabilities.contains(&Capability::LanguageSelection), "{}", m.id);
                    assert!(m.languages.iter().any(|(c, _)| *c == code), "{}", m.id);
                }
            }
        }
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
        assert_eq!(canary().total_bytes(), 1_029_466_298);
        assert_eq!(flash().total_bytes(), 213_424_426);
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
