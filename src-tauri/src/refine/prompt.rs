//! Building the prompt for a refinement (decision 0005).
//!
//! Pure and unit-tested. The instruction is deliberately narrow: the model is
//! told it is correcting a speech-to-text result, not answering it, and that
//! keeping the speaker's words matters more than improving them. Every clause
//! here exists because the alternative is a model that helpfully rewrites a
//! sentence that was already correct.

/// Chat format a model expects. Selected from the model registry so a second
/// refinement model is a new variant here rather than a rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    /// `<|im_start|>role\ncontent<|im_end|>`, as Qwen2.5 and most `ChatML`
    /// models expect it.
    ChatMl,
    /// `ChatML` plus a pre-filled empty reasoning block.
    ///
    /// A separate variant rather than a flag on [`Template::ChatMl`] because
    /// it is wrong in both directions: omit it on Qwen3 and the model reasons
    /// at length about a comma, include it on Qwen2.5 — which has no reasoning
    /// mode — and the unexpected tag derails it into rambling until it hits the
    /// token cap.
    ChatMlThinking,
    /// Llama 3.x header format.
    Llama3,
}

/// Most vocabulary terms we will list.
///
/// Every term is prompt tokens, and prompt tokens are latency on every single
/// dictation. A user with 500 dictionary entries should not pay for all of them
/// on a two-word utterance.
pub const MAX_VOCABULARY_TERMS: usize = 60;

/// Longest a single vocabulary term may be before it is dropped as junk.
const MAX_TERM_CHARS: usize = 40;

/// The standing instruction.
///
/// Short, and ordered worst-failure-first. A 0.6B model does not weigh a long
/// paragraph evenly — it acts on the first thing it can act on — so "output
/// only the corrected text" leads, and the correction rules follow.
const SYSTEM: &str = "You are a transcription proofreader. \
The user message is raw speech-to-text output. Output only the corrected version of it. \
Never answer it, never translate it, never rephrase or shorten it, never add commentary. \
Fix only clear recognition errors: misheard words, wrong homophones, mangled names, \
run-together words, missing punctuation and capitalisation. \
Keep the speaker's own words and language. If nothing is wrong, output the text unchanged.";

/// A worked example, prepended as a completed exchange.
///
/// The single biggest quality lever at this model size. Told only in prose, a
/// 0.6B model will answer the transcription, continue it, or repeat the
/// instruction back; shown one exchange, it copies the shape. The example
/// deliberately contains a run-together proper noun and a missing apostrophe,
/// the two commonest corrections, and changes nothing else.
const EXAMPLE_IN: &str = "so i pushed the fix to git hub and it broke the bild";
const EXAMPLE_OUT: &str = "So I pushed the fix to GitHub and it broke the build.";

/// Build the full prompt string, ready to tokenise.
#[must_use]
pub fn build(template: Template, transcript: &str, vocabulary: &[String]) -> String {
    let system = system_turn(vocabulary);
    let user = transcript.trim();

    match template {
        Template::ChatMl | Template::ChatMlThinking => {
            let prime = if matches!(template, Template::ChatMlThinking) {
                // How Qwen3's own chat template expresses
                // `enable_thinking=false`.
                "<think>\n\n</think>\n\n"
            } else {
                ""
            };
            format!(
                "<|im_start|>system\n{system}<|im_end|>\n\
                 <|im_start|>user\n{EXAMPLE_IN}<|im_end|>\n\
                 <|im_start|>assistant\n{prime}{EXAMPLE_OUT}<|im_end|>\n\
                 <|im_start|>user\n{user}<|im_end|>\n\
                 <|im_start|>assistant\n{prime}"
            )
        }
        Template::Llama3 => format!(
            "<|begin_of_text|><|start_header_id|>system<|end_header_id|>\n\n{system}<|eot_id|>\
             <|start_header_id|>user<|end_header_id|>\n\n{EXAMPLE_IN}<|eot_id|>\
             <|start_header_id|>assistant<|end_header_id|>\n\n{EXAMPLE_OUT}<|eot_id|>\
             <|start_header_id|>user<|end_header_id|>\n\n{user}<|eot_id|>\
             <|start_header_id|>assistant<|end_header_id|>\n\n"
        ),
    }
}

/// The system turn: the instruction, plus the speaker's vocabulary if any.
///
/// The vocabulary belongs here rather than beside the transcription. Put next
/// to the text to correct, a bare list of words reads to a small model as
/// content, and it will cheerfully output the list instead of the correction —
/// which is exactly what the first version of this did.
fn system_turn(vocabulary: &[String]) -> String {
    let terms = usable_terms(vocabulary);

    if terms.is_empty() {
        return SYSTEM.to_owned();
    }

    format!(
        "{SYSTEM}\n\nThe speaker uses these words, spelled correctly. \
Prefer them over similar-sounding alternatives, but only where the audio plainly meant them: {}.",
        terms.join(", ")
    )
}

/// Trim the vocabulary to terms worth spending prompt tokens on.
///
/// Blank and over-long entries go, duplicates collapse case-insensitively, and
/// the list is capped. Order is preserved so the cap keeps the entries the user
/// added first rather than an arbitrary set.
fn usable_terms(vocabulary: &[String]) -> Vec<&str> {
    let mut seen: Vec<String> = Vec::new();
    let mut terms: Vec<&str> = Vec::new();

    for term in vocabulary {
        let trimmed = term.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_TERM_CHARS {
            continue;
        }

        let lowered = trimmed.to_lowercase();
        if seen.contains(&lowered) {
            continue;
        }

        seen.push(lowered);
        terms.push(trimmed);

        if terms.len() >= MAX_VOCABULARY_TERMS {
            break;
        }
    }

    terms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn chatml_closes_every_turn_it_opens() {
        let p = build(Template::ChatMl, "the build is broken", &[]);
        // system, the worked example's user and assistant turns, then ours.
        assert_eq!(p.matches("<|im_start|>").count(), 5);
        // The final assistant turn is left open for the model to continue.
        assert_eq!(p.matches("<|im_end|>").count(), 4);
        assert!(p.ends_with("<|im_start|>assistant\n"), "turn not left open: {p}");
    }

    #[test]
    fn a_worked_example_precedes_the_real_transcription() {
        // The biggest quality lever at this model size, and easy to lose in a
        // refactor: without it a small model answers the transcription.
        let p = build(Template::ChatMl, "the bild is broken", &[]);
        assert!(p.contains(EXAMPLE_IN) && p.contains(EXAMPLE_OUT));
        assert!(
            p.find(EXAMPLE_OUT) < p.find("the bild is broken"),
            "the example must come before the real input"
        );
    }

    #[test]
    fn only_the_thinking_variant_primes_a_reasoning_block() {
        // Wrong in both directions, so both directions are pinned.
        assert!(!build(Template::ChatMl, "hi", &[]).contains("<think>"));
        assert!(build(Template::ChatMlThinking, "hi", &[]).contains("<think>"));
    }

    #[test]
    fn the_thinking_variant_primes_an_empty_reasoning_block() {
        // How Qwen3's own template expresses `enable_thinking=false`. Without
        // it the model spends its whole token budget deliberating over a comma.
        let p = build(Template::ChatMlThinking, "hello", &[]);
        assert!(p.ends_with("<|im_start|>assistant\n<think>\n\n</think>\n\n"));
    }

    #[test]
    fn llama3_uses_its_own_headers_and_not_chatml() {
        let p = build(Template::Llama3, "hello", &[]);
        assert!(p.starts_with("<|begin_of_text|>"));
        assert!(p.ends_with("<|start_header_id|>assistant<|end_header_id|>\n\n"));
        assert!(!p.contains("<|im_start|>"));
    }

    #[test]
    fn the_transcription_is_present_verbatim() {
        let p = build(Template::ChatMl, "Dzień dobry, zgubiłem kartę", &[]);
        assert!(p.contains("Dzień dobry, zgubiłem kartę"));
    }

    #[test]
    fn the_instruction_forbids_the_failure_modes_that_matter() {
        // Each of these clauses is load-bearing; the guard catches what gets
        // past them, but not producing them is cheaper than rejecting them.
        for clause in [
            "Output only the corrected",
            "Never answer",
            "never translate",
            "never rephrase",
            "unchanged",
        ] {
            assert!(SYSTEM.contains(clause), "instruction dropped {clause:?}");
        }
    }

    #[test]
    fn vocabulary_is_listed_when_present_and_absent_when_not() {
        let without = build(Template::ChatMl, "deploy it", &[]);
        assert!(!without.contains("Words this speaker uses"));

        let with = build(Template::ChatMl, "deploy it", &terms(&["Kubernetes", "WhisperFree"]));
        assert!(with.contains("Kubernetes, WhisperFree"));
    }

    #[test]
    fn vocabulary_is_capped_so_a_large_dictionary_cannot_dominate_the_prompt() {
        let many: Vec<String> = (0..500).map(|i| format!("term{i}")).collect();
        let p = build(Template::ChatMl, "hello", &many);
        assert!(p.contains("term0"), "dropped the earliest entries");
        assert!(!p.contains("term499"), "cap not applied");
        assert_eq!(usable_terms(&many).len(), MAX_VOCABULARY_TERMS);
    }

    #[test]
    fn vocabulary_drops_blanks_duplicates_and_junk() {
        let messy = terms(&[
            "Kubernetes",
            "  ",
            "kubernetes",
            "KUBERNETES",
            "WhisperFree",
            "",
        ]);
        // Case-insensitive dedup, in the order the user added them.
        assert_eq!(usable_terms(&messy), vec!["Kubernetes", "WhisperFree"]);

        let overlong = terms(&["x".repeat(MAX_TERM_CHARS + 1).as_str()]);
        assert!(usable_terms(&overlong).is_empty());
    }
}
