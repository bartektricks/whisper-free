//! Deciding whether a refinement is safe to paste (decision 0005).
//!
//! This is the load-bearing half of the refinement stage. A small language
//! model asked to correct a transcription will sometimes answer it, summarise
//! it, translate it, or helpfully rewrite a sentence that was already right —
//! and post-ASR correction is measurably worst at exactly the point our speech
//! model is best, on clean input with few errors to find.
//!
//! So the model's output is a *proposal*, and everything here exists to throw
//! it away. Rejection is the safe direction: the caller falls back to the raw
//! transcription, and the user loses a correction rather than their words.
//!
//! Pure — no model, no I/O, no clock. All of it is unit-tested.

/// Why a candidate was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// Nothing survived unwrapping and trimming.
    Empty,
    /// Too much shorter or longer than the original to be a correction.
    LengthRatio,
    /// Too many words differ. A correction edits; this rewrote.
    TooDivergent,
    /// The model talked to us instead of answering.
    Meta,
}

impl RejectReason {
    /// Stable label for structured logs. Never contains user text.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::LengthRatio => "length_ratio",
            Self::TooDivergent => "too_divergent",
            Self::Meta => "meta",
        }
    }
}

/// What to do with a candidate refinement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Safe to use, cleaned of any wrapping the model added.
    Accept(String),
    /// The model returned the transcription unchanged. Not a failure — most
    /// dictations need no correction at all — but worth counting separately
    /// from a real edit so the logs show how often the stage does nothing.
    Unchanged,
    /// Use the raw transcription instead.
    Reject(RejectReason),
}

/// How far a candidate may stray before it stops being a correction.
///
/// Measured, not guessed. Across the sample in
/// `measured_corrections_and_rewrites_stay_separated`, real corrections score
/// 0.000-0.105 and rewrites 0.190-0.762, so the threshold sits in that gap,
/// nearer the rewrites to leave headroom for a sentence with more errors than
/// any tested here.
///
/// The gap is narrower than it looks, and re-measuring beats nudging the
/// number: if a legitimate correction ever lands above the line, the fix is to
/// widen the *sample* and see where the boundary really is.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Lower bound on `candidate chars / original chars`.
    pub min_length_ratio: f64,
    /// Upper bound on the same ratio. Above 1.0 because punctuation,
    /// capitalisation and expanded numerals all add characters.
    pub max_length_ratio: f64,
    /// Upper bound on normalised character edit distance over the longer side.
    pub max_divergence: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            min_length_ratio: 0.5,
            max_length_ratio: 1.6,
            max_divergence: 0.18,
        }
    }
}

/// Openers that mean the model addressed us rather than answering.
///
/// Matched only against the start of the candidate, lowercased. A tight list on
/// purpose: "note" and "sorry" are ordinary dictation words in the middle of a
/// sentence, and the length and divergence checks are the real defence.
const META_OPENERS: &[&str] = &[
    "here is",
    "here's",
    "sure,",
    "sure!",
    "certainly",
    "of course",
    "i cannot",
    "i can't",
    "i'm sorry",
    "i am sorry",
    "as an ai",
    "the corrected",
    "corrected text",
    "corrected version",
];

/// Lead-ins the model wraps an otherwise good answer in, stripped rather than
/// rejected when the rest of the line survives the other checks.
const STRIPPABLE_PREFIXES: &[&str] = &[
    "here is the corrected text:",
    "here is the corrected version:",
    "here is the correction:",
    "corrected text:",
    "corrected version:",
    "correction:",
    "output:",
];

/// Judge a model's proposed rewrite against the transcription it came from.
///
/// Returns the text to use on [`Verdict::Accept`]; on anything else the caller
/// pastes `original`.
#[must_use]
pub fn judge(original: &str, candidate: &str, limits: &Limits) -> Verdict {
    let cleaned = unwrap_candidate(candidate);

    if cleaned.trim().is_empty() {
        return Verdict::Reject(RejectReason::Empty);
    }

    if starts_with_meta(cleaned) {
        return Verdict::Reject(RejectReason::Meta);
    }

    if !length_ratio_ok(original, cleaned, limits) {
        return Verdict::Reject(RejectReason::LengthRatio);
    }

    if divergence(original, cleaned) > limits.max_divergence {
        return Verdict::Reject(RejectReason::TooDivergent);
    }

    if cleaned == original.trim() {
        return Verdict::Unchanged;
    }

    Verdict::Accept(cleaned.to_owned())
}

/// Peel off the packaging a chat model puts around an answer: a reasoning
/// block, a code fence, a lead-in line, a pair of quotes.
///
/// Borrowed rather than allocated, so this stays cheap on the common path where
/// there is nothing to peel.
#[must_use]
pub fn unwrap_candidate(candidate: &str) -> &str {
    let mut text = candidate.trim();

    // Qwen3 and friends emit a reasoning block even when asked not to. Keep
    // what comes after it; a stray opener with no close means the model never
    // stopped thinking, and there is no answer to salvage.
    if let Some(rest) = text.strip_prefix("<think>") {
        text = rest.split_once("</think>").map_or("", |(_, after)| after).trim();
    }

    text = strip_code_fence(text);
    text = strip_lead_in(text);
    text = strip_matched_quotes(text);

    text.trim()
}

/// Drop a fenced code block's delimiters, keeping the contents.
fn strip_code_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    // The opening fence may carry a language tag; the body starts after the
    // first newline.
    let body = rest.split_once('\n').map_or(rest, |(_, after)| after);
    body.trim_end().strip_suffix("```").unwrap_or(body).trim()
}

/// Drop a recognised "here is the corrected text:" style opener.
fn strip_lead_in(text: &str) -> &str {
    let lowered = text.to_lowercase();
    for prefix in STRIPPABLE_PREFIXES {
        if lowered.starts_with(prefix) {
            // Same byte length in both, since the prefixes are all ASCII and
            // lowercasing ASCII cannot change a character's width.
            if let Some(rest) = text.get(prefix.len()..) {
                return rest.trim();
            }
        }
    }
    text
}

/// Drop one pair of quotes wrapping the whole string.
fn strip_matched_quotes(text: &str) -> &str {
    for (open, close) in [('"', '"'), ('\'', '\''), ('“', '”'), ('„', '”')] {
        if let Some(inner) = text.strip_prefix(open).and_then(|t| t.strip_suffix(close)) {
            // Only when the quotes wrap everything — a quoted phrase inside a
            // longer sentence is the user's, not the model's.
            if !inner.contains(close) {
                return inner.trim();
            }
        }
    }
    text
}

/// Does the candidate open by talking about the task rather than doing it?
fn starts_with_meta(text: &str) -> bool {
    let lowered = text.trim_start().to_lowercase();
    META_OPENERS.iter().any(|marker| lowered.starts_with(marker))
}

/// Is the candidate's length plausible for a correction of `original`?
fn length_ratio_ok(original: &str, candidate: &str, limits: &Limits) -> bool {
    let original_chars = original.trim().chars().count();
    let candidate_chars = candidate.chars().count();

    if original_chars == 0 {
        return candidate_chars == 0;
    }

    // Character counts here are sentence-length, nowhere near the 2^53 at which
    // an integer stops being exactly representable, and the result only decides
    // a comparison against a ratio.
    #[allow(clippy::as_conversions, clippy::cast_precision_loss)]
    let ratio = candidate_chars as f64 / original_chars as f64;

    ratio >= limits.min_length_ratio && ratio <= limits.max_length_ratio
}

/// Fraction of the text that changed, in `0.0..=1.0`.
///
/// Measured over [`normalise`]d characters rather than whole words, because
/// the corrections worth making do not respect word boundaries. "cuber
/// netties" becoming "Kubernetes" merges two words into one; a word-level
/// distance charges two edits for it, and a third for the apostrophe in
/// "let's", which is enough to push an ideal correction of a short sentence
/// past any threshold loose enough to be useful. Character distance charges
/// what the change actually costs.
fn divergence(original: &str, candidate: &str) -> f64 {
    let a: Vec<char> = normalise(original).chars().collect();
    let b: Vec<char> = normalise(candidate).chars().collect();

    let longest = a.len().max(b.len());
    if longest == 0 {
        return 0.0;
    }

    // Both are the length of one utterance, far below the 2^53 at which an
    // integer stops being exactly representable; the result only decides a
    // comparison against a ratio.
    #[allow(clippy::as_conversions, clippy::cast_precision_loss)]
    let fraction = levenshtein(&a, &b) as f64 / longest as f64;

    fraction
}

/// Reduce text to the part a correction is not supposed to change: lowercase
/// words, single-spaced, with punctuation gone.
///
/// Everything stripped here is something the model is *invited* to change —
/// adding a comma or a capital is the job, so it must not register as
/// divergence. Apostrophes are dropped rather than spaced, so "let's" and
/// "lets" compare as the same word.
#[must_use]
pub fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut gap = false;

    for c in text.chars().flat_map(char::to_lowercase) {
        if matches!(c, '\'' | '\u{2019}' | '\u{02BC}') {
            continue;
        }
        if crate::dictionary::is_word_char(c) {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }

    out
}

/// Character-level edit distance.
///
/// A single-row DP rather than a matrix: it is the shape that satisfies the
/// crate's `indexing_slicing` and `arithmetic_side_effects` lints without
/// fighting them, and it keeps the allocation to two rows. The length check in
/// [`judge`] runs first, so the two sides are already within a small factor of
/// each other by the time this is reached.
fn levenshtein(a: &[char], b: &[char]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    // `prev[j]` is the distance from the first `i` characters of `a` to the
    // first `j` of `b`; the row starts as the distance from the empty prefix.
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; prev.len()];

    for (i, ca) in a.iter().enumerate() {
        if let Some(slot) = curr.first_mut() {
            *slot = i.saturating_add(1);
        }

        for (j, cb) in b.iter().enumerate() {
            let next = j.saturating_add(1);
            let substitution = prev
                .get(j)
                .copied()
                .unwrap_or(usize::MAX)
                .saturating_add(usize::from(ca != cb));
            let deletion = prev.get(next).copied().unwrap_or(usize::MAX).saturating_add(1);
            let insertion = curr.get(j).copied().unwrap_or(usize::MAX).saturating_add(1);

            if let Some(slot) = curr.get_mut(next) {
                *slot = substitution.min(deletion).min(insertion);
            }
        }

        std::mem::swap(&mut prev, &mut curr);
    }

    prev.last().copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn judge_default(original: &str, candidate: &str) -> Verdict {
        judge(original, candidate, &Limits::default())
    }

    fn rejected(original: &str, candidate: &str) -> RejectReason {
        match judge_default(original, candidate) {
            Verdict::Reject(reason) => reason,
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    /// The measurement the default threshold rests on.
    ///
    /// | | divergence |
    /// |---|---|
    /// | punctuation and capitalisation only | 0.000 |
    /// | one run-together proper noun | 0.026 |
    /// | one misheard word | 0.032-0.095 |
    /// | a merge plus an apostrophe | 0.105 |
    /// | a small paraphrase (dropped words, changed person) | 0.190 |
    /// | an answer to the transcription | 0.581 |
    /// | a full rewrite | 0.761 |
    /// | a translation | 0.762 |
    ///
    /// This is a guard against the metric quietly stopping to separate the two
    /// groups — a change that would not fail any other test here, because
    /// every other test asserts a verdict, and a verdict can stay right for a
    /// while after the number behind it has gone wrong.
    #[test]
    fn measured_corrections_and_rewrites_stay_separated() {
        let corrections: &[(&str, &str)] = &[
            ("lets deploy to cuber netties on friday", "Let's deploy to Kubernetes on Friday."),
            ("the servis is broken", "The service is broken."),
            ("the bild is broken again today", "The build is broken again today."),
            ("so i pushed it to git hub this morning", "So I pushed it to GitHub this morning."),
            ("dzień dobry zgubiłem swoją kartę kredytową", "Dzień dobry, zgubiłem swoją kartę kredytową."),
            ("can you send me the whisper free logs", "Can you send me the WhisperFree logs?"),
        ];
        let rewrites: &[(&str, &str)] = &[
            // The tightest of these, and the reason the threshold is 0.18 and
            // not 0.30: dropped words and a change of person, in Polish.
            ("dzień dobry zgubiłem swoją kartę kredytową", "Dzień dobry zgubiono kartę kredytową."),
            ("um so the thing is broken again", "The system is currently experiencing an outage."),
            ("what is the capital of poland", "The capital of Poland is Warsaw."),
            ("Dzień dobry, zgubiłem swoją kartę kredytową.", "Hello, I lost my credit card."),
        ];

        let limit = Limits::default().max_divergence;
        for (original, candidate) in corrections {
            let d = divergence(original, candidate);
            assert!(d <= limit, "correction scored {d:.3}, over the {limit} limit: {candidate:?}");
        }
        for (original, candidate) in rewrites {
            let d = divergence(original, candidate);
            assert!(d > limit, "rewrite scored {d:.3}, under the {limit} limit: {candidate:?}");
        }
    }

    /// What the guard cannot do, pinned so nobody comes to rely on it.
    ///
    /// A wrong substitution of the same size as a right one is indistinguishable
    /// to any measure of *how much* changed — "cuber netties" becoming "Cuber
    /// Nuts" scores 0.105, exactly what it scores becoming "Kubernetes". The
    /// guard bounds the size of a change, never its correctness; keeping the
    /// model from making that substitution is the prompt's job and the model's,
    /// not this module's.
    #[test]
    fn the_guard_cannot_tell_a_wrong_substitution_from_a_right_one() {
        let right = divergence("lets deploy to cuber netties on friday", "Let's deploy to Kubernetes on Friday.");
        let wrong = divergence("lets deploy to cuber netties on friday", "Let's deploy to Cuber Nuts on Friday.");
        assert!(
            (right - wrong).abs() < 0.01,
            "these are {right:.3} and {wrong:.3}; if they have diverged, the comment above is stale"
        );
    }

    #[test]
    fn a_genuine_correction_is_accepted() {
        let verdict = judge_default(
            "lets deploy to cuber netties on friday",
            "Let's deploy to Kubernetes on Friday.",
        );
        assert_eq!(
            verdict,
            Verdict::Accept("Let's deploy to Kubernetes on Friday.".to_owned())
        );
    }

    #[test]
    fn punctuation_and_capitalisation_alone_are_accepted() {
        // Nothing changes at word level here, so this passes on divergence and
        // has to survive the length check instead.
        let verdict = judge_default("the build is broken", "The build is broken.");
        assert_eq!(verdict, Verdict::Accept("The build is broken.".to_owned()));
    }

    #[test]
    fn an_untouched_transcription_reports_unchanged_rather_than_accept() {
        // Most dictations need no correction. That is a distinct outcome from
        // an edit, so the logs can show how often the stage does nothing.
        assert_eq!(
            judge_default("the build is broken", "the build is broken"),
            Verdict::Unchanged
        );
    }

    #[test]
    fn a_paraphrase_is_rejected_however_reasonable_it_sounds() {
        // The failure this whole module exists to prevent: fluent, plausible,
        // and not what the user said.
        assert_eq!(
            rejected(
                "um so the thing is broken again",
                "The system is currently experiencing an outage."
            ),
            RejectReason::TooDivergent
        );
    }

    #[test]
    fn an_answer_to_the_transcription_is_rejected() {
        assert_eq!(
            rejected(
                "what is the capital of poland",
                "The capital of Poland is Warsaw."
            ),
            RejectReason::TooDivergent
        );
    }

    #[test]
    fn a_truncated_rewrite_is_rejected_on_length() {
        assert_eq!(
            rejected(
                "we need to bump the dependencies before we ship this on friday",
                "Bump the deps."
            ),
            RejectReason::LengthRatio
        );
    }

    #[test]
    fn a_padded_rewrite_is_rejected_on_length() {
        assert_eq!(
            rejected(
                "ship it",
                "Ship it. Please let me know if you would like me to expand on any of this."
            ),
            RejectReason::LengthRatio
        );
    }

    #[test]
    fn an_empty_candidate_is_rejected() {
        assert_eq!(rejected("the build is broken", "   \n  "), RejectReason::Empty);
    }

    #[test]
    fn a_refusal_is_rejected_as_meta() {
        assert_eq!(
            rejected(
                "the build is broken again today",
                "I cannot help with that request."
            ),
            RejectReason::Meta
        );
    }

    #[test]
    fn a_lead_in_is_stripped_rather_than_rejected() {
        // Worth keeping: the answer underneath is good, and throwing it away
        // for its packaging would lose a correction for no reason.
        assert_eq!(
            judge_default("the bild is broken", "Corrected text: The build is broken."),
            Verdict::Accept("The build is broken.".to_owned())
        );
    }

    #[test]
    fn a_reasoning_block_is_stripped() {
        assert_eq!(
            judge_default(
                "the bild is broken",
                "<think>\nThe user misspelled build.\n</think>\n\nThe build is broken."
            ),
            Verdict::Accept("The build is broken.".to_owned())
        );
    }

    #[test]
    fn a_model_that_never_stops_thinking_yields_nothing() {
        assert_eq!(
            rejected("the build is broken", "<think>\nHmm, let me consider"),
            RejectReason::Empty
        );
    }

    #[test]
    fn a_code_fence_is_stripped() {
        assert_eq!(
            judge_default("the bild is broken", "```\nThe build is broken.\n```"),
            Verdict::Accept("The build is broken.".to_owned())
        );
    }

    #[test]
    fn wrapping_quotes_are_stripped_but_inner_ones_are_kept() {
        assert_eq!(
            judge_default("he said hello to me", "\"He said hello to me.\""),
            Verdict::Accept("He said hello to me.".to_owned())
        );
        // The user's own quotation must survive intact.
        assert_eq!(
            judge_default("he said hello to me", "He said \"hello\" to me."),
            Verdict::Accept("He said \"hello\" to me.".to_owned())
        );
    }

    #[test]
    fn normalising_keeps_polish_letters_as_letters() {
        // The dictionary's Unicode rule, reused: diacritics are word
        // characters, so this is four words and not a stream of fragments.
        assert_eq!(
            normalise("Dzień dobry, zgubiłem   swoją kartę!"),
            "dzień dobry zgubiłem swoją kartę"
        );
    }

    #[test]
    fn normalising_erases_exactly_what_the_model_is_asked_to_add() {
        // Punctuation and capitalisation are the job, so they must not read as
        // divergence at all.
        assert_eq!(
            normalise("the build is broken"),
            normalise("The build is broken.")
        );
        // Apostrophes are dropped rather than spaced, so the single most
        // common correction in English costs nothing.
        assert_eq!(normalise("lets go"), normalise("Let's go"));
        assert_eq!(normalise("dont stop"), normalise("don\u{2019}t stop"));
    }

    #[test]
    fn a_polish_correction_is_accepted() {
        assert_eq!(
            judge_default(
                "dzień dobry zgubiłem swoją kartę kredytową",
                "Dzień dobry, zgubiłem swoją kartę kredytową."
            ),
            Verdict::Accept("Dzień dobry, zgubiłem swoją kartę kredytową.".to_owned())
        );
    }

    #[test]
    fn a_translation_is_rejected() {
        assert_eq!(
            rejected(
                "dzień dobry zgubiłem swoją kartę kredytową",
                "Good morning, I have lost my credit card."
            ),
            RejectReason::TooDivergent
        );
    }

    #[test]
    fn rejection_reasons_have_stable_log_labels() {
        // These land in structured logs, so they are part of the interface.
        for (reason, label) in [
            (RejectReason::Empty, "empty"),
            (RejectReason::LengthRatio, "length_ratio"),
            (RejectReason::TooDivergent, "too_divergent"),
            (RejectReason::Meta, "meta"),
        ] {
            assert_eq!(reason.as_str(), label);
        }
    }

    #[test]
    fn an_empty_original_accepts_only_an_empty_candidate() {
        // Defensive: the pipeline checks for an empty transcription before it
        // ever gets here, but the guard must not divide by zero if that
        // changes.
        assert_eq!(rejected("", "something"), RejectReason::LengthRatio);
    }

    #[test]
    fn character_distance_counts_single_edits() {
        let chars = |s: &str| -> Vec<char> { s.chars().collect() };

        assert_eq!(levenshtein(&chars("kitten"), &chars("sitting")), 3);
        assert_eq!(levenshtein(&chars("same"), &chars("same")), 0);
        assert_eq!(levenshtein(&[], &chars("abcd")), 4);
        assert_eq!(levenshtein(&chars("abcd"), &[]), 4);
        assert_eq!(levenshtein(&[], &[]), 0);
    }

    #[test]
    fn divergence_is_bounded_at_both_ends() {
        assert!(divergence("the quick brown fox", "the quick brown fox").abs() < f64::EPSILON);
        assert!((divergence("the quick brown fox", "") - 1.0).abs() < f64::EPSILON);
        assert!(divergence("", "").abs() < f64::EPSILON);
    }

    #[test]
    fn merging_two_misheard_words_into_one_stays_cheap() {
        // The exact failure that word-level distance got wrong: two words
        // becoming one, plus an apostrophe appearing, in a short sentence.
        // Both are ideal corrections and must sit far below the threshold.
        let moved = divergence(
            "lets deploy to cuber netties on friday",
            "Let\u{2019}s deploy to Kubernetes on Friday.",
        );
        assert!(moved < 0.20, "an ideal correction scored {moved}");
    }

    #[test]
    fn a_rewrite_and_a_correction_land_on_opposite_sides_of_the_threshold() {
        // The separation the default threshold depends on. If these ever
        // converge, the metric is no longer doing its job and the number
        // cannot simply be nudged.
        let correction = divergence("the servis is broken", "The service is broken.");
        let rewrite = divergence(
            "um so the thing is broken again",
            "The system is currently experiencing an outage.",
        );
        assert!(correction < 0.15, "correction scored {correction}");
        assert!(rewrite > 0.45, "rewrite scored {rewrite}");
    }

    #[test]
    fn the_divergence_threshold_tolerates_one_word_in_four() {
        // A single misheard word in a short sentence is the single most common
        // thing this stage exists to fix. If the default limit rejected it the
        // feature would do nothing at all.
        let verdict = judge_default(
            "deploy the servis on friday",
            "Deploy the service on Friday.",
        );
        assert!(matches!(verdict, Verdict::Accept(_)), "got {verdict:?}");
    }
}
