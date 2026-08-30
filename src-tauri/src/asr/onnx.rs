//! The ONNX speech recognisers, via `transcribe-rs` (plan §6, §23.5, §23.7).
//!
//! This is the only file that knows any particular speech model exists.
//! Everything model-specific — the ONNX layout, quantisation, chunk sizes, the
//! crate's own API — stops here. Adding a model is a `ModelDescriptor` plus, if
//! it is a family we do not run yet, one [`OnnxEngine`] variant.
//!
//! The Parakeet choices below are justified by measurement in
//! `docs/decisions/0001-parakeet-inference-runtime.md`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use transcribe_rs::onnx::canary::CanaryModel;
use transcribe_rs::onnx::parakeet::ParakeetModel;
use transcribe_rs::onnx::Quantization;
use transcribe_rs::transcriber::{EnergyAdaptiveChunked, EnergyAdaptiveConfig, Transcriber};
use transcribe_rs::{set_ort_accelerator, OrtAccelerator, SpeechModel, TranscribeOptions};

use super::{
    AsrError, AudioBuffer, Capability, Language, LanguageSelection, SpeechRecognizer,
    Transcription, TranscriptionOptions,
};

/// How a recogniser splits audio too long to decode in one pass.
#[derive(Debug, Clone, Copy)]
pub struct Chunking {
    /// Audio longer than this is decoded in chunks rather than in one pass.
    pub threshold_secs: f64,
    /// Target chunk length once chunking kicks in.
    pub target_secs: f32,
    /// Seconds either side of the target to hunt for a quiet split point, so
    /// chunks break in pauses rather than mid-word.
    pub search_secs: f32,
}

impl Chunking {
    /// Measured for Parakeet in decision 0001: a single long buffer can decode
    /// to an empty string even when it plainly contains speech, and ~15 s
    /// chunks recover it. A 29 s clip failed; its first 15 s did not.
    const PARAKEET: Self = Self {
        threshold_secs: 18.0,
        target_secs: 15.0,
        search_secs: 3.0,
    };

    /// Inherited from Parakeet rather than measured for Canary.
    ///
    /// Canary is an attention encoder-decoder and has no reason to share
    /// Parakeet's failure, so this is a safe default and not a finding:
    /// shorter chunks never break a model that could have taken longer ones,
    /// they only cost a little context at the seams. Measure before raising it.
    const CANARY: Self = Self::PARAKEET;
}

/// Which `transcribe-rs` model a recogniser drives.
///
/// The variants exist because the two families load differently and disagree
/// about language, not because anything outside this file cares which is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnnxEngine {
    Parakeet,
    Canary,
}

impl OnnxEngine {
    const fn chunking(self) -> Chunking {
        match self {
            Self::Parakeet => Chunking::PARAKEET,
            Self::Canary => Chunking::CANARY,
        }
    }

    fn load(self, dir: &Path) -> Result<Box<dyn SpeechModel>, AsrError> {
        let load = |e: transcribe_rs::TranscribeError| {
            tracing::error!(error = %e, "speech model failed to load");
            AsrError::ModelLoad(e.to_string())
        };
        match self {
            Self::Parakeet => ParakeetModel::load(dir, &Quantization::Int8)
                .map(|m| -> Box<dyn SpeechModel> { Box::new(m) })
                .map_err(load),
            Self::Canary => CanaryModel::load(dir, &Quantization::Int8)
                .map(|m| -> Box<dyn SpeechModel> { Box::new(m) })
                .map_err(load),
        }
    }
}

/// Close up the space Canary leaves before punctuation.
///
/// Canary decodes German as `Wie geht es dir heute ? Das ist wunderbar .`,
/// a space before every terminal mark, because the punctuation arrives as its
/// own token and nothing joins it back on. Its English and French output has
/// no such spacing at all, which is what makes this safe to apply everywhere:
/// there is no legitimate space to destroy, not even the narrow one French
/// typography puts before `?` and `!`, because the model never emits it.
/// Parakeet does not produce the artefact either, so this is a no-op there.
///
/// Deliberately only the marks that are always tight against the word before
/// them. Quotes and brackets are left alone: getting those right needs to know
/// which side of a pair each one is, and guessing wrong moves the space to the
/// wrong place rather than removing it.
fn tidy_punctuation(text: &str) -> String {
    const TIGHT: [char; 6] = ['.', ',', '!', '?', ';', ':'];

    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if TIGHT.contains(&ch) {
            while out.ends_with(' ') {
                out.pop();
            }
        }
        out.push(ch);
    }
    out
}

pub struct OnnxRecognizer {
    model_id: String,
    engine: OnnxEngine,
    model_dir: PathBuf,
    languages: Vec<Language>,
    capabilities: Vec<Capability>,
    /// File names the descriptor declares, so availability is checked against
    /// what this model actually needs rather than a name hardcoded here.
    files: Vec<String>,
    model: Option<Box<dyn SpeechModel>>,
}

impl OnnxRecognizer {
    pub fn new(
        model_id: impl Into<String>,
        engine: OnnxEngine,
        model_dir: PathBuf,
        languages: Vec<Language>,
        capabilities: Vec<Capability>,
        files: Vec<String>,
    ) -> Self {
        Self {
            model_id: model_id.into(),
            engine,
            model_dir,
            languages,
            capabilities,
            files,
            model: None,
        }
    }

    /// Translate the app's language selection into what the crate expects.
    ///
    /// `None` means "work it out", which is what Parakeet does and Canary
    /// cannot; a caller only reaches here once `check_language_request` has
    /// agreed the model can honour what was asked.
    fn transcribe_options(selection: &LanguageSelection) -> TranscribeOptions {
        TranscribeOptions {
            language: match selection {
                LanguageSelection::Auto => None,
                LanguageSelection::Fixed(code) => Some(code.clone()),
            },
            ..Default::default()
        }
    }
}

impl SpeechRecognizer for OnnxRecognizer {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn supported_languages(&self) -> &[Language] {
        &self.languages
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    fn is_available(&self) -> bool {
        !self.files.is_empty()
            && self
                .files
                .iter()
                .all(|name| self.model_dir.join(name).exists())
    }

    fn load(&mut self) -> Result<(), AsrError> {
        if self.model.is_some() {
            return Ok(());
        }
        if !self.is_available() {
            return Err(AsrError::ModelNotInstalled);
        }

        // CPU, deliberately: CoreML measured 2.9x slower and used 4.5x the
        // memory on this int8 graph (decision 0001).
        set_ort_accelerator(OrtAccelerator::CpuOnly);

        let started = Instant::now();
        let model = self.engine.load(&self.model_dir)?;

        tracing::info!(
            event = "model_loaded",
            model_id = %self.model_id,
            load_ms = crate::millis(started.elapsed())
        );
        self.model = Some(model);
        Ok(())
    }

    fn unload(&mut self) {
        if self.model.take().is_some() {
            tracing::info!(event = "model_unloaded", model_id = %self.model_id);
        }
    }

    fn is_loaded(&self) -> bool {
        self.model.is_some()
    }

    fn transcribe(
        &mut self,
        audio: &AudioBuffer,
        options: &TranscriptionOptions,
    ) -> Result<Transcription, AsrError> {
        // Refused up front rather than silently ignored: Parakeet cannot be
        // pinned to a language and Canary cannot detect one, so exactly one of
        // the two selections is wrong for any given model.
        super::check_language_request(self, &options.language)?;

        self.load()?;
        let chunking = self.engine.chunking();
        let model = self.model.as_deref_mut().ok_or(AsrError::ModelNotInstalled)?;

        let audio_duration = audio.duration();
        let started = Instant::now();
        let transcribe_options = Self::transcribe_options(&options.language);

        let split_into_chunks = audio_duration.as_secs_f64() > chunking.threshold_secs;
        let result = if split_into_chunks {
            let mut chunker = EnergyAdaptiveChunked::new(
                EnergyAdaptiveConfig {
                    target_chunk_secs: chunking.target_secs,
                    search_window_secs: chunking.search_secs,
                    ..Default::default()
                },
                transcribe_options,
            );
            chunker.transcribe(model, &audio.samples)
        } else {
            model.transcribe(&audio.samples, &transcribe_options)
        };

        let result = result.map_err(|e| {
            tracing::warn!(error = %e, "transcription failed");
            AsrError::Transcription(e.to_string())
        })?;

        Ok(Transcription {
            text: tidy_punctuation(result.text.trim()),
            // `transcribe-rs` does not report which language it detected, so
            // claiming one would be a guess. When the user pinned one we do
            // know, and saying so costs nothing.
            language: match &options.language {
                LanguageSelection::Fixed(code) => {
                    self.languages.iter().find(|l| l.code == *code).cloned()
                }
                LanguageSelection::Auto => None,
            },
            duration: started.elapsed(),
            audio_duration,
            chunked: split_into_chunks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parakeet(dir: PathBuf) -> OnnxRecognizer {
        OnnxRecognizer::new(
            "parakeet-tdt-0.6b-v3",
            OnnxEngine::Parakeet,
            dir,
            vec![
                Language::new("pl", "Polish"),
                Language::new("en", "English"),
            ],
            vec![Capability::LanguageDetection, Capability::Punctuation],
            vec!["vocab.txt".to_string()],
        )
    }

    fn canary(dir: PathBuf) -> OnnxRecognizer {
        OnnxRecognizer::new(
            "canary-1b-v2",
            OnnxEngine::Canary,
            dir,
            vec![
                Language::new("pl", "Polish"),
                Language::new("en", "English"),
            ],
            vec![Capability::LanguageSelection, Capability::Punctuation],
            vec!["vocab.txt".to_string()],
        )
    }

    #[test]
    fn a_missing_model_directory_is_not_available() {
        let r = parakeet(PathBuf::from("/nonexistent/model/dir"));
        assert!(!r.is_available());
        assert!(!r.is_loaded());
    }

    #[test]
    fn loading_without_the_files_reports_a_missing_model() {
        let mut r = parakeet(PathBuf::from("/nonexistent/model/dir"));
        assert!(matches!(r.load(), Err(AsrError::ModelNotInstalled)));
    }

    #[test]
    fn unloading_when_nothing_is_loaded_is_harmless() {
        let mut r = parakeet(PathBuf::from("/nonexistent/model/dir"));
        r.unload();
        r.unload();
        assert!(!r.is_loaded());
    }

    #[test]
    fn pinning_a_language_is_refused_before_any_model_work_happens() {
        // Parakeet cannot honour a fixed language, so the request must fail on
        // capability rather than load a 650 MB model and ignore the option.
        let mut r = parakeet(PathBuf::from("/nonexistent/model/dir"));
        let audio = AudioBuffer::new(vec![0.0; 16_000], 16_000);
        let options = TranscriptionOptions {
            language: LanguageSelection::Fixed("pl".into()),
        };
        assert!(matches!(
            r.transcribe(&audio, &options),
            Err(AsrError::UnsupportedCapability(_))
        ));
    }

    #[test]
    fn asking_canary_to_detect_is_refused_the_same_way() {
        // The mirror image, and the reason both capabilities exist: Canary is
        // told the language or it is told nothing useful.
        let mut r = canary(PathBuf::from("/nonexistent/model/dir"));
        let audio = AudioBuffer::new(vec![0.0; 16_000], 16_000);
        let options = TranscriptionOptions {
            language: LanguageSelection::Auto,
        };
        assert!(matches!(
            r.transcribe(&audio, &options),
            Err(AsrError::UnsupportedCapability(_))
        ));
    }

    #[test]
    fn a_pinned_language_reaches_the_engine_and_auto_leaves_it_open() {
        // The bug this guards against is silent: passing the crate's default
        // options drops the user's language and Canary quietly decodes English.
        let pinned = OnnxRecognizer::transcribe_options(&LanguageSelection::Fixed("pl".into()));
        assert_eq!(pinned.language.as_deref(), Some("pl"));

        let auto = OnnxRecognizer::transcribe_options(&LanguageSelection::Auto);
        assert_eq!(auto.language, None);
    }

    #[test]
    fn availability_is_checked_against_every_file_the_model_declares() {
        let dir = std::env::temp_dir().join("whisperfree-onnx-availability");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut r = canary(dir.clone());
        r.files = vec!["vocab.txt".into(), "nemo128.onnx".into()];
        std::fs::write(dir.join("vocab.txt"), b"x").unwrap();
        assert!(!r.is_available(), "one file present is not installed");

        std::fs::write(dir.join("nemo128.onnx"), b"x").unwrap();
        assert!(r.is_available());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn punctuation_is_closed_up_against_the_word_before_it() {
        // Measured output from Canary 180M Flash on German audio. Pasting this
        // into a document verbatim is visibly wrong, and no later stage in the
        // pipeline would fix it: the dictionary replaces words, and refinement
        // is opt-in and allowed to decline.
        assert_eq!(
            tidy_punctuation("Wie geht es dir heute ? Das ist wunderbar ."),
            "Wie geht es dir heute? Das ist wunderbar."
        );
        assert_eq!(
            tidy_punctuation("Guten Tag . Dies ist ein Test ."),
            "Guten Tag. Dies ist ein Test."
        );
    }

    #[test]
    fn text_that_was_already_right_is_left_exactly_as_it_was() {
        // Parakeet and Canary's own English and French output, which must
        // survive this untouched — the step is a repair, not a style.
        for text in [
            "Bonjour, ceci est un test de reconnaissance vocale en français.",
            "The quick brown fox jumps over the lazy dog.",
            "Hello, this is a test of English speech recognition. Does it work?",
            "Hola, esto es una prueba del reconocimiento de voz en español.",
            "",
        ] {
            assert_eq!(tidy_punctuation(text), text);
        }
    }

    #[test]
    fn only_spaces_immediately_before_the_mark_are_removed() {
        // Nothing else about the text may move: a newline is structure, and
        // the space *after* a mark is what separates the next sentence.
        assert_eq!(tidy_punctuation("one . two"), "one. two");
        assert_eq!(tidy_punctuation("one\n. two"), "one\n. two");
        assert_eq!(tidy_punctuation("a   ,   b"), "a,   b");
        assert_eq!(tidy_punctuation("wait ..."), "wait...");
    }

    // Asserting on constants is the point here: these are tuning values, and
    // the test exists so that editing one past a measured boundary fails.
    #[test]
    fn the_chunking_threshold_sits_above_the_length_that_decoded_cleanly() {
        // Decision 0001: 15 s decoded fine on its own, 20 s and 29 s did not.
        // The threshold must not be so high that a known-bad length slips
        // through as a single pass.
        const {
            assert!(Chunking::PARAKEET.threshold_secs >= 15.0);
            assert!(Chunking::PARAKEET.threshold_secs < 20.0);
        }

        assert!(f64::from(Chunking::PARAKEET.target_secs) <= Chunking::PARAKEET.threshold_secs);
        assert!(f64::from(Chunking::CANARY.target_secs) <= Chunking::CANARY.threshold_secs);
    }
}
