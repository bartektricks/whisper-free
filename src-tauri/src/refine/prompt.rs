//! Building the prompt for a refinement (decision 0012).
//!
//! Pure and unit-tested. Unlike the hand-tuned instruction decision 0005 needed
//! for a general instruct model, everything here is dictated by the model card:
//! S1-mini is fine-tuned for this one task and expects a fixed system turn, a
//! control line naming the three settings, and an assistant turn primed with an
//! empty reasoning block. Deviating from that format is not a style choice, it
//! is going off the distribution the model was trained on.

/// Register the cleaned text is written in.
///
/// The one control the user gets. Structure and Context are fixed at `prose`
/// and `general`: lists and email layout change the *shape* of what comes back,
/// which is a bigger promise than a dictation box should make on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Styling {
    /// Lowercase, apostrophes stripped, colloquialisms kept.
    Casual,
    /// The speaker's phrasing, with `I` capitalised.
    SemiCasual,
    /// Standard written English with full punctuation. The model card's
    /// recommended default, and ours.
    #[default]
    SemiFormal,
    /// Contractions expanded: "I am", "cannot".
    Formal,
}

impl Styling {
    /// The value as the control line spells it.
    #[must_use]
    pub const fn as_control_value(self) -> &'static str {
        match self {
            Self::Casual => "casual",
            Self::SemiCasual => "semi-casual",
            Self::SemiFormal => "semi-formal",
            Self::Formal => "formal",
        }
    }
}

/// The standing instruction, verbatim from the model card.
///
/// Not ours to reword. The model was trained against this exact string, and the
/// control line below only means anything because this sentence introduces it.
const SYSTEM: &str = "You are a text normalizer for speech-to-text transcripts. \
The input begins with a control line specifying the styling, structure, and context settings; \
clean the transcript to match those settings and output only the cleaned text.";

/// The two control values we do not vary. See [`Styling`].
const STRUCTURE: &str = "prose";
const CONTEXT: &str = "general";

/// Everything up to and including the control line's newline.
///
/// Split out because it is the same for every dictation at a given
/// [`Styling`], which is what lets `refine::onnx` run it through the graph once
/// and start each dictation with its KV cache already populated. Roughly 69 of
/// the 78 fixed prompt tokens.
#[must_use]
pub fn prefix(styling: Styling) -> String {
    format!(
        "<|im_start|>system\n{SYSTEM}<|im_end|>\n\
         <|im_start|>user\n[Styling: {}] [Structure: {STRUCTURE}] [Context: {CONTEXT}]\n",
        styling.as_control_value()
    )
}

/// The transcript and everything after it.
///
/// The empty `<think>` block is the model card's `enable_thinking=False`, and
/// it is load-bearing: without it the model emits an empty reasoning block and
/// stops, returning nothing usable at all.
#[must_use]
pub fn suffix(transcript: &str) -> String {
    format!(
        "{}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n",
        transcript.trim()
    )
}

/// The whole prompt, for callers that do not split it.
#[must_use]
pub fn build(styling: Styling, transcript: &str) -> String {
    let mut out = prefix(styling);
    out.push_str(&suffix(transcript));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_is_the_prefix_followed_by_the_suffix() {
        // The split exists so the prefix can be cached; if the two halves ever
        // stop concatenating to the whole prompt, the cached run and the
        // uncached one stop agreeing and only the slow path is ever tested.
        let whole = build(Styling::SemiFormal, "hello there");
        assert_eq!(
            whole,
            format!("{}{}", prefix(Styling::SemiFormal), suffix("hello there"))
        );
    }

    #[test]
    fn the_control_line_names_all_three_settings() {
        let built = build(Styling::Formal, "anything");
        assert!(
            built.contains("[Styling: formal] [Structure: prose] [Context: general]\n"),
            "control line is not in the documented form: {built}"
        );
    }

    #[test]
    fn every_styling_has_the_spelling_the_model_card_uses() {
        // Hyphenated, lowercase, and not something a Rust enum name would
        // produce by itself. A wrong value here is not an error, it is the
        // model quietly ignoring the setting.
        assert_eq!(Styling::Casual.as_control_value(), "casual");
        assert_eq!(Styling::SemiCasual.as_control_value(), "semi-casual");
        assert_eq!(Styling::SemiFormal.as_control_value(), "semi-formal");
        assert_eq!(Styling::Formal.as_control_value(), "formal");
    }

    #[test]
    fn the_assistant_turn_is_primed_with_an_empty_reasoning_block() {
        // Leave this out and the model returns an empty think block and stops.
        // The model card is explicit that this is the usual cause of "no
        // usable output at all".
        let built = build(Styling::SemiFormal, "anything");
        assert!(built.ends_with("<|im_start|>assistant\n<think>\n\n</think>\n\n"), "{built}");
    }

    #[test]
    fn the_transcript_is_the_last_thing_in_the_user_turn() {
        let built = build(Styling::SemiFormal, "the build is broken");
        let user = built
            .rsplit_once("<|im_start|>user\n")
            .map(|(_, rest)| rest)
            .unwrap_or_default();
        assert!(user.starts_with("[Styling:"), "control line must come first: {user}");
        assert!(
            user.contains("the build is broken<|im_end|>"),
            "transcript must close the user turn: {user}"
        );
    }

    #[test]
    fn the_turn_structure_is_a_single_exchange() {
        // Decision 0005 carried a worked example, which meant two user turns.
        // A fine-tune does not need one, and the extra turn is pure prefill.
        let built = build(Styling::SemiFormal, "anything");
        assert_eq!(built.matches("<|im_start|>").count(), 3);
        assert_eq!(built.matches("<|im_end|>").count(), 2);
    }

    #[test]
    fn the_system_turn_is_the_model_cards_wording() {
        // Pinned because it is not ours to improve: the model was trained
        // against this string.
        let built = build(Styling::SemiFormal, "x");
        assert!(built.contains("You are a text normalizer for speech-to-text transcripts."));
        assert!(built.contains("output only the cleaned text."));
    }

    #[test]
    fn surrounding_whitespace_never_reaches_the_model() {
        let built = build(Styling::SemiFormal, "  padded  \n");
        assert!(built.contains("general]\npadded<|im_end|>"), "{built}");
    }

    #[test]
    fn the_prefix_is_the_same_for_one_styling_and_differs_between_them() {
        // The cache is keyed on this string, so two stylings sharing a prefix
        // would silently serve one another's cache.
        assert_eq!(prefix(Styling::Casual), prefix(Styling::Casual));
        assert_ne!(prefix(Styling::Casual), prefix(Styling::Formal));
    }
}
