//! Parakeet TDT 0.6B v3, via `transcribe-rs` on ONNX Runtime.
//!
//! This is the only file that knows Parakeet exists. Everything model-specific
//! — the ONNX layout, quantisation, chunk sizes, the crate's own API — stops
//! here (plan §23.7).
//!
//! The choices below are justified by measurement in
//! `docs/decisions/0001-parakeet-inference-runtime.md`.

use std::path::PathBuf;
use std::time::Instant;

use transcribe_rs::onnx::parakeet::ParakeetModel;
use transcribe_rs::onnx::Quantization;
use transcribe_rs::transcriber::{EnergyAdaptiveChunked, EnergyAdaptiveConfig, Transcriber};
use transcribe_rs::{set_ort_accelerator, OrtAccelerator, SpeechModel, TranscribeOptions};

use super::{
    AsrError, AudioBuffer, Capability, Language, SpeechRecognizer, Transcription,
    TranscriptionOptions,
};

/// Audio longer than this is decoded in chunks rather than in one pass.
///
/// Decision 0001 found that a single long buffer can decode to an empty string
/// even when it plainly contains speech, and that ~15 s chunks recover it. A
/// 29 s clip failed; its first 15 s did not.
const CHUNK_THRESHOLD_SECS: f64 = 18.0;

/// Target chunk length once chunking kicks in.
const CHUNK_TARGET_SECS: f32 = 15.0;

/// Seconds either side of the target to hunt for a quiet split point, so
/// chunks break in pauses rather than mid-word.
const CHUNK_SEARCH_SECS: f32 = 3.0;

pub struct ParakeetRecognizer {
    model_id: String,
    model_dir: PathBuf,
    languages: Vec<Language>,
    capabilities: Vec<Capability>,
    model: Option<ParakeetModel>,
}

impl ParakeetRecognizer {
    pub fn new(
        model_id: impl Into<String>,
        model_dir: PathBuf,
        languages: Vec<Language>,
        capabilities: Vec<Capability>,
    ) -> Self {
        Self {
            model_id: model_id.into(),
            model_dir,
            languages,
            capabilities,
            model: None,
        }
    }
}

impl SpeechRecognizer for ParakeetRecognizer {
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
        self.model_dir.join("vocab.txt").exists()
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
        let model = ParakeetModel::load(&self.model_dir, &Quantization::Int8).map_err(|e| {
            tracing::error!(error = %e, "parakeet failed to load");
            AsrError::ModelLoad(e.to_string())
        })?;

        tracing::info!(
            event = "model_loaded",
            model_id = %self.model_id,
            load_ms = started.elapsed().as_millis() as u64
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
        // Parakeet v3 detects the language itself and cannot be pinned, so a
        // fixed-language request is refused rather than silently ignored.
        super::check_language_request(self, &options.language)?;

        self.load()?;
        let model = self.model.as_mut().ok_or(AsrError::ModelNotInstalled)?;

        let audio_duration = audio.duration();
        let started = Instant::now();

        let result = if audio_duration.as_secs_f64() > CHUNK_THRESHOLD_SECS {
            let mut chunker = EnergyAdaptiveChunked::new(
                EnergyAdaptiveConfig {
                    target_chunk_secs: CHUNK_TARGET_SECS,
                    search_window_secs: CHUNK_SEARCH_SECS,
                    ..Default::default()
                },
                TranscribeOptions::default(),
            );
            chunker.transcribe(model, &audio.samples)
        } else {
            model.transcribe(&audio.samples, &TranscribeOptions::default())
        };

        let result = result.map_err(|e| {
            tracing::warn!(error = %e, "parakeet transcription failed");
            AsrError::Transcription(e.to_string())
        })?;

        Ok(Transcription {
            text: result.text.trim().to_string(),
            // `transcribe-rs` does not report which language it detected, so
            // claiming one would be a guess.
            language: None,
            duration: started.elapsed(),
            audio_duration,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::LanguageSelection;

    fn recognizer(dir: PathBuf) -> ParakeetRecognizer {
        ParakeetRecognizer::new(
            "parakeet-tdt-0.6b-v3",
            dir,
            vec![Language::new("pl", "Polish"), Language::new("en", "English")],
            vec![Capability::LanguageDetection, Capability::Punctuation],
        )
    }

    #[test]
    fn a_missing_model_directory_is_not_available() {
        let r = recognizer(PathBuf::from("/nonexistent/model/dir"));
        assert!(!r.is_available());
        assert!(!r.is_loaded());
    }

    #[test]
    fn loading_without_the_files_reports_a_missing_model() {
        let mut r = recognizer(PathBuf::from("/nonexistent/model/dir"));
        assert!(matches!(r.load(), Err(AsrError::ModelNotInstalled)));
    }

    #[test]
    fn unloading_when_nothing_is_loaded_is_harmless() {
        let mut r = recognizer(PathBuf::from("/nonexistent/model/dir"));
        r.unload();
        r.unload();
        assert!(!r.is_loaded());
    }

    #[test]
    fn pinning_a_language_is_refused_before_any_model_work_happens() {
        // Parakeet v3 cannot honour a fixed language, so the request must fail
        // on capability rather than load a 650 MB model and ignore the option.
        let mut r = recognizer(PathBuf::from("/nonexistent/model/dir"));
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
    fn the_chunking_threshold_sits_above_the_length_that_decoded_cleanly() {
        // Decision 0001: 15 s decoded fine on its own, 20 s and 29 s did not.
        // The threshold must not be so high that a known-bad length slips
        // through as a single pass.
        assert!(CHUNK_THRESHOLD_SECS >= 15.0);
        assert!(CHUNK_THRESHOLD_SECS < 20.0);
        assert!(CHUNK_TARGET_SECS <= CHUNK_THRESHOLD_SECS as f32);
    }
}
