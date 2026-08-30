//! Types shared by every speech recogniser (plan §6, §8).
//!
//! Nothing here knows what Parakeet is. Languages are data, not variants, so
//! adding a language never means touching application logic.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// A language the recogniser can produce, identified by ISO 639-1 code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Language {
    pub code: String,
    pub name: String,
}

impl Language {
    pub fn new(code: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            name: name.into(),
        }
    }
}

/// What the user asked for: let the model decide, or pin one language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "code", rename_all = "snake_case")]
pub enum LanguageSelection {
    /// Let the model detect the language. Requires `Capability::LanguageDetection`.
    #[default]
    Auto,
    /// Force a specific language. Requires `Capability::LanguageSelection`.
    Fixed(String),
}

/// Optional features a recogniser may or may not have (plan §6).
///
/// Modelled as capabilities rather than assumed, so a future recogniser that
/// cannot detect languages degrades gracefully instead of misbehaving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Detects the spoken language on its own.
    LanguageDetection,
    /// Honours a caller-specified language.
    LanguageSelection,
    /// Emits punctuation and capitalisation.
    Punctuation,
    /// Produces word or segment timings.
    Timestamps,
    /// Can transcribe incrementally from a live stream.
    Streaming,
}

#[derive(Debug, Clone, Default)]
pub struct TranscriptionOptions {
    pub language: LanguageSelection,
}

/// The result of one transcription.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Transcription {
    pub text: String,
    /// The language the recogniser reports, when it can report one.
    pub language: Option<Language>,
    /// How long inference took.
    pub duration: Duration,
    /// Length of the audio that was transcribed, for real-time-factor logging.
    pub audio_duration: Duration,
    /// Whether the audio was decoded in one pass or split into chunks.
    ///
    /// Logged rather than acted on. When a long dictation comes back short,
    /// the first question is which decode path produced it, and inferring that
    /// from the duration means knowing a threshold that belongs to the engine.
    pub chunked: bool,
}

impl Transcription {
    /// Inference time divided by audio length. Below 1.0 is faster than real time.
    #[must_use]
    pub fn real_time_factor(&self) -> f64 {
        let audio = self.audio_duration.as_secs_f64();
        if audio <= 0.0 {
            return f64::NAN;
        }
        self.duration.as_secs_f64() / audio
    }

    /// True when the model returned nothing usable.
    ///
    /// This is a real, observed outcome and not a theoretical one — see
    /// `docs/decisions/0001-parakeet-inference-runtime.md`. Callers must
    /// surface it rather than silently inserting an empty string.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// 16 kHz mono audio, samples in [-1.0, 1.0].
///
/// The sample rate is carried rather than assumed so a future recogniser that
/// wants something other than 16 kHz can say so.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl AudioBuffer {
    #[must_use]
    pub const fn new(samples: Vec<f32>, sample_rate: u32) -> Self {
        Self {
            samples,
            sample_rate,
        }
    }

    #[must_use]
    pub fn duration(&self) -> Duration {
        if self.sample_rate == 0 {
            return Duration::ZERO;
        }
        // Capture is capped at ten minutes, so a count that will not fit in a
        // u32 (74 hours at 16 kHz) cannot come from this app.
        let Ok(frames) = u32::try_from(self.samples.len()) else {
            return Duration::ZERO;
        };
        Duration::from_secs_f64(f64::from(frames) / f64::from(self.sample_rate))
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_duration_matches_sample_count() {
        let buf = AudioBuffer::new(vec![0.0; 16_000], 16_000);
        assert_eq!(buf.duration(), Duration::from_secs(1));
    }

    #[test]
    fn zero_sample_rate_does_not_divide_by_zero() {
        let buf = AudioBuffer::new(vec![0.0; 16_000], 0);
        assert_eq!(buf.duration(), Duration::ZERO);
    }

    #[test]
    fn real_time_factor_is_inference_over_audio() {
        let t = Transcription {
            text: "hello".into(),
            language: None,
            duration: Duration::from_millis(500),
            audio_duration: Duration::from_secs(10),
            chunked: false,
        };
        assert!((t.real_time_factor() - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn whitespace_only_transcription_counts_as_empty() {
        let t = Transcription {
            text: "  \n ".into(),
            language: None,
            duration: Duration::from_millis(1),
            audio_duration: Duration::from_secs(1),
            chunked: false,
        };
        assert!(t.is_empty());
    }

    #[test]
    fn language_selection_defaults_to_auto() {
        assert_eq!(LanguageSelection::default(), LanguageSelection::Auto);
    }
}
