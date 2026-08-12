//! The ASR boundary (plan §6, §23.5, §23.7).
//!
//! Everything outside this module thinks in terms of `audio -> transcription`.
//! Model-specific detail — ONNX sessions, quantisation, chunking, tokenisers —
//! lives behind `SpeechRecognizer` and must not leak past it.

pub mod types;

pub use types::{
    AudioBuffer, Capability, Language, LanguageSelection, Transcription, TranscriptionOptions,
};

#[derive(Debug, thiserror::Error)]
pub enum AsrError {
    #[error("no model is installed")]
    ModelNotInstalled,
    #[error("model failed to load: {0}")]
    ModelLoad(String),
    #[error("transcription failed: {0}")]
    Transcription(String),
    #[error("{language} is not supported by this model")]
    UnsupportedLanguage { language: String },
    #[error("this model cannot {0}")]
    UnsupportedCapability(&'static str),
    #[error("cancelled")]
    Cancelled,
}

/// A loadable speech-to-text model.
///
/// Implementations are expected to be usable across threads (`Send`) so the
/// recogniser can live behind a mutex and be driven from the hotkey thread.
pub trait SpeechRecognizer: Send {
    /// Stable identifier of the model this recogniser runs, e.g.
    /// `"parakeet-tdt-0.6b-v3"`. Matches the id in the model registry.
    fn model_id(&self) -> &str;

    /// Languages this model can transcribe.
    fn supported_languages(&self) -> &[Language];

    /// Optional features this model provides.
    fn capabilities(&self) -> &[Capability];

    fn supports(&self, capability: Capability) -> bool {
        self.capabilities().contains(&capability)
    }

    /// Whether the model's files are present on disk and ready to load.
    fn is_available(&self) -> bool;

    /// Load the model into memory. Idempotent: loading an already-loaded model
    /// is a no-op rather than an error.
    fn load(&mut self) -> Result<(), AsrError>;

    /// Release model memory. Idempotent.
    fn unload(&mut self);

    fn is_loaded(&self) -> bool;

    /// Transcribe a complete audio buffer.
    ///
    /// Loads the model first if it is not already loaded. May return a
    /// transcription whose text is empty — callers must check
    /// [`Transcription::is_empty`] rather than assuming success means text.
    fn transcribe(
        &mut self,
        audio: &AudioBuffer,
        options: &TranscriptionOptions,
    ) -> Result<Transcription, AsrError>;

    /// Ask an in-flight `transcribe` to stop early.
    ///
    /// The default is a no-op: cancellation is best-effort, and a recogniser
    /// that cannot interrupt itself is still a valid recogniser.
    fn cancel(&self) {}
}

/// Validate a language request against what a recogniser can actually do.
///
/// Keeps the capability check in one place instead of repeating it in each
/// implementation.
pub fn check_language_request(
    recognizer: &dyn SpeechRecognizer,
    selection: &LanguageSelection,
) -> Result<(), AsrError> {
    match selection {
        LanguageSelection::Auto => {
            if recognizer.supports(Capability::LanguageDetection) {
                Ok(())
            } else {
                Err(AsrError::UnsupportedCapability("detect the language itself"))
            }
        }
        LanguageSelection::Fixed(code) => {
            if !recognizer.supports(Capability::LanguageSelection) {
                return Err(AsrError::UnsupportedCapability("be pinned to one language"));
            }
            if recognizer
                .supported_languages()
                .iter()
                .any(|l| l.code == *code)
            {
                Ok(())
            } else {
                Err(AsrError::UnsupportedLanguage {
                    language: code.clone(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct FakeRecognizer {
        languages: Vec<Language>,
        capabilities: Vec<Capability>,
    }

    impl SpeechRecognizer for FakeRecognizer {
        fn model_id(&self) -> &str {
            "fake"
        }
        fn supported_languages(&self) -> &[Language] {
            &self.languages
        }
        fn capabilities(&self) -> &[Capability] {
            &self.capabilities
        }
        fn is_available(&self) -> bool {
            true
        }
        fn load(&mut self) -> Result<(), AsrError> {
            Ok(())
        }
        fn unload(&mut self) {}
        fn is_loaded(&self) -> bool {
            true
        }
        fn transcribe(
            &mut self,
            _audio: &AudioBuffer,
            _options: &TranscriptionOptions,
        ) -> Result<Transcription, AsrError> {
            Ok(Transcription {
                text: String::new(),
                language: None,
                duration: Duration::ZERO,
                audio_duration: Duration::ZERO,
            })
        }
    }

    fn detecting() -> FakeRecognizer {
        FakeRecognizer {
            languages: vec![Language::new("pl", "Polish"), Language::new("en", "English")],
            capabilities: vec![Capability::LanguageDetection],
        }
    }

    #[test]
    fn auto_is_allowed_when_the_model_detects_languages() {
        let r = detecting();
        assert!(check_language_request(&r, &LanguageSelection::Auto).is_ok());
    }

    #[test]
    fn auto_is_refused_when_the_model_cannot_detect() {
        let r = FakeRecognizer {
            languages: vec![Language::new("en", "English")],
            capabilities: vec![],
        };
        assert!(matches!(
            check_language_request(&r, &LanguageSelection::Auto),
            Err(AsrError::UnsupportedCapability(_))
        ));
    }

    #[test]
    fn pinning_a_language_needs_the_selection_capability() {
        // Parakeet v3 detects but cannot be pinned — exactly this case.
        let r = detecting();
        assert!(matches!(
            check_language_request(&r, &LanguageSelection::Fixed("pl".into())),
            Err(AsrError::UnsupportedCapability(_))
        ));
    }

    #[test]
    fn unknown_language_is_rejected() {
        let r = FakeRecognizer {
            languages: vec![Language::new("en", "English")],
            capabilities: vec![Capability::LanguageSelection],
        };
        assert!(matches!(
            check_language_request(&r, &LanguageSelection::Fixed("pl".into())),
            Err(AsrError::UnsupportedLanguage { .. })
        ));
    }

    #[test]
    fn supported_language_is_accepted() {
        let r = FakeRecognizer {
            languages: vec![Language::new("pl", "Polish")],
            capabilities: vec![Capability::LanguageSelection],
        };
        assert!(check_language_request(&r, &LanguageSelection::Fixed("pl".into())).is_ok());
    }
}
