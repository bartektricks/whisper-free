//! The refinement boundary (decision 0005).
//!
//! Everything outside this module thinks in terms of `text -> text`. Which
//! model runs, how it is prompted, how it is decoded — all of that lives behind
//! [`TextRefiner`] and must not leak past it, the same way `asr/` hides the
//! speech model.
//!
//! Refinement is **advisory**. Every failure in here — an absent model, a load
//! error, a rejected rewrite, a cancelled run — resolves to "paste what the
//! speech model said". Losing a correction is a disappointment; losing the
//! user's words is a bug.

pub mod guard;
pub mod onnx;
pub mod prompt;

pub use guard::{Limits, RejectReason, Rule, Verdict};
pub use prompt::Styling;

use std::time::Duration;

/// Longest transcription we will hand to a language model.
///
/// Refinement cost grows with the prompt, and a dictation long enough to pass
/// this is one where a per-token wait would be felt as a hang. Over the limit
/// the raw transcription is pasted, which is the same outcome as the feature
/// being switched off.
pub const MAX_INPUT_CHARS: usize = 2_000;

/// How many correction tokens we allow beyond the length of the input.
///
/// A cleaned transcript is roughly as long as its input. The margin covers
/// punctuation and expanded numerals; anything past it is the model having
/// started to write something else, and stopping mid-sentence there costs
/// nothing, because the guard would have rejected the result anyway.
///
/// The `+ 32` half of the model card's own `1.3 x input_tokens + 32` ceiling.
pub const OUTPUT_TOKEN_MARGIN: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum RefineError {
    #[error("no refinement model is installed")]
    ModelNotInstalled,
    #[error("model failed to load: {0}")]
    ModelLoad(String),
    #[error("generation failed: {0}")]
    Generation(String),
    #[error("cancelled")]
    Cancelled,
    #[error("input of {chars} characters is too long to refine")]
    TooLong { chars: usize },
}

/// What the refiner was asked to do.
#[derive(Debug, Clone, Copy, Default)]
pub struct RefineOptions {
    /// The register the cleaned text is written in.
    ///
    /// The only knob. Decision 0005 passed the user's dictionary here too, as a
    /// hint to the model; a fine-tuned normaliser has no slot for one, so those
    /// terms went to [`guard::judge`] instead, which is where they always
    /// belonged. See decision 0012.
    pub styling: Styling,
}

/// A proposed rewrite, before the guard has had a look at it.
#[derive(Debug, Clone)]
pub struct Refinement {
    pub text: String,
    /// Wall-clock time in the model.
    pub duration: Duration,
    /// Of which, the first pass over the prompt. Prompt length is the only
    /// lever on this one, so it is worth seeing on its own.
    pub prefill: Duration,
    /// Tokens the model produced. Logged as a shape; the text never is.
    pub generated_tokens: usize,
    /// Tokens the prompt occupied.
    pub prompt_tokens: usize,
}

/// A loadable model that proposes corrections to transcribed text.
///
/// Mirrors [`crate::asr::SpeechRecognizer`]: `Send` so it can live behind a
/// mutex on [`crate::AppContext`], loaded lazily inside the call, idempotent
/// `load`/`unload`, and an `is_available` disk check.
pub trait TextRefiner: Send {
    /// Stable identifier matching the model registry.
    fn model_id(&self) -> &str;

    /// Whether the model's files are present on disk and ready to load.
    fn is_available(&self) -> bool;

    /// Load the model into memory. Idempotent.
    ///
    /// # Errors
    ///
    /// [`RefineError::ModelNotInstalled`] when the files are absent, or
    /// [`RefineError::ModelLoad`] when they are present but unusable.
    fn load(&mut self) -> Result<(), RefineError>;

    /// Release model memory. Idempotent.
    fn unload(&mut self);

    fn is_loaded(&self) -> bool;

    /// Propose a corrected version of `text`.
    ///
    /// Loads the model first if needed. The result is a *proposal*: callers
    /// must put it through [`guard::judge`] before using it.
    ///
    /// # Errors
    ///
    /// [`RefineError::TooLong`] above [`MAX_INPUT_CHARS`],
    /// [`RefineError::Cancelled`] when cancellation lands mid-generation, and
    /// [`RefineError::Generation`] when inference fails.
    fn refine(
        &mut self,
        text: &str,
        options: &RefineOptions,
    ) -> Result<Refinement, RefineError>;

    /// Ask an in-flight `refine` to stop.
    ///
    /// Unlike the speech model, generation *is* interruptible: the flag is
    /// checked between tokens, so Escape during refinement takes effect within
    /// a token rather than at the end of the run.
    fn cancel(&self);
}

/// User-facing wording for a refinement failure.
///
/// Refinement failures are not shown as errors today — the pipeline falls back
/// to the raw transcription instead — but the wording exists so that a future
/// surface has something written for a person rather than a Rust error.
#[must_use]
pub fn user_message(error: &RefineError) -> String {
    match error {
        RefineError::ModelNotInstalled => {
            "No cleanup model is installed yet. Open Settings › Models to download one.".into()
        }
        RefineError::ModelLoad(_) => {
            "The cleanup model could not be loaded. Try removing and downloading it again in Settings › Models."
                .into()
        }
        RefineError::Generation(_) => {
            "The transcription could not be cleaned up, so it was inserted as heard.".into()
        }
        RefineError::Cancelled => "Cleanup was cancelled.".into(),
        RefineError::TooLong { .. } => {
            "That dictation was too long to clean up, so it was inserted as heard.".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refine_messages_are_plain_language_and_hide_internals() {
        let errors = [
            RefineError::ModelNotInstalled,
            RefineError::ModelLoad("ort::Error: failed to load model_q4f16.onnx".into()),
            RefineError::Generation("tensor shape mismatch [1, 28, 0, 128] in past_key_values".into()),
            RefineError::Cancelled,
            RefineError::TooLong { chars: 4_096 },
        ];
        for e in errors {
            let msg = user_message(&e);
            assert!(!msg.is_empty());
            for internal in [
                "ort", "onnx", "tokenizer", "tensor", "kv", "past_key_values", "logits", "f16",
            ] {
                assert!(
                    !msg.to_lowercase().contains(internal),
                    "leaked internals: {msg}"
                );
            }
        }
    }

    #[test]
    fn generation_errors_never_carry_the_text_that_caused_them() {
        // `refinement_failed` logs the error with `%e`, so whatever a variant
        // carries reaches the log file — and the README promises logs never
        // contain transcription text. The two stages that touch user text are
        // tokenising the prompt and detokenising the output, and a tokeniser
        // error can quote the input it choked on, so neither may interpolate
        // the underlying error.
        //
        // Checked against the source because there is no way to provoke a
        // tokeniser failure from here, and a comment alone does not survive a
        // refactor.
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/refine/onnx.rs"
        ))
        .unwrap_or_default();
        assert!(!source.is_empty(), "could not read onnx.rs to check it");

        // Scoped to `Generation`: loading `tokenizer.json` off disk also
        // mentions the tokeniser and may interpolate freely, because a
        // `ModelLoad` failure has never seen a transcription.
        for line in source.lines() {
            let touches_user_text = line.contains("tokenis") || line.contains("detokenis");
            let interpolates = line.contains("{e}") || line.contains("{err");
            assert!(
                !(touches_user_text && interpolates && line.contains("Generation")),
                "this can put the transcription in the log: {}",
                line.trim()
            );
        }
    }

    #[test]
    fn every_failure_names_what_happened_to_the_users_words() {
        // The whole stage is advisory, so a failure message must reassure
        // rather than alarm: the text still arrives.
        for e in [
            RefineError::Generation("boom".into()),
            RefineError::TooLong { chars: 9_000 },
        ] {
            let msg = user_message(&e);
            assert!(
                msg.contains("inserted as heard"),
                "does not say the text survived: {msg}"
            );
        }
    }
}
