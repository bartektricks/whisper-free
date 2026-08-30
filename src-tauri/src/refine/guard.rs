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
    /// Words appeared that the speaker never said. Under
    /// [`Rule::Containment`] this is the whole defence: a normaliser may drop
    /// as much as it likes, but anything it *adds* was invented.
    Invented,
    /// The end of the transcription is missing. A cleanup thins a sentence
    /// throughout; losing the tail is a truncation.
    Truncated,
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
            Self::Invented => "invented",
            Self::Truncated => "truncated",
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

/// How a candidate is judged against the transcription it came from.
///
/// Two rules, because the two models this app has shipped fail in opposite
/// directions. A general instruct model asked to proofread will quietly rewrite
/// a sentence that was already fine, so what matters is *how much* changed. A
/// normaliser is *supposed* to change a lot - fillers out, numbers written -
/// so magnitude says nothing, and what matters is whether anything appeared
/// that the speaker never said.
///
/// Both are measured, not guessed; see decision 0012 for the corpus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rule {
    /// Bound how far the text moved. Decision 0005's rule, kept for the
    /// light-touch setting.
    Magnitude {
        /// Lower bound on `candidate chars / original chars`.
        min_length_ratio: f64,
        /// Upper bound on the same ratio. Above 1.0 because punctuation,
        /// capitalisation and expanded numerals all add characters.
        max_length_ratio: f64,
        /// Upper bound on normalised character edit distance over the longer
        /// side.
        max_divergence: f64,
    },
    /// Bound what the text gained. Deletions are free, invention is not.
    Containment {
        /// Lower bound on `candidate words / original words`. Catches a
        /// summary, which the upper bound cannot see.
        min_growth: f64,
        /// Upper bound on the same. Normalisation only ever shortens or holds,
        /// so anything above ~1 is the model adding its own sentences.
        max_growth: f64,
        /// Upper bound on the fraction of candidate words that never appear in
        /// the transcription. See [`novel_word_rate`] for what does not count.
        max_novel_word_rate: f64,
    },
}

/// The thresholds a candidate is judged against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub rule: Rule,
}

impl Limits {
    /// Accept corrections only: punctuation, capitalisation, a misheard word.
    ///
    /// Decision 0005's numbers, unchanged. Across that decision's sample real
    /// corrections scored 0.000-0.105 and rewrites 0.190-0.762, so the
    /// threshold sits in the gap, nearer the rewrites to leave headroom for a
    /// sentence with more errors than any tested there.
    #[must_use]
    pub const fn light_touch() -> Self {
        Self {
            rule: Rule::Magnitude {
                min_length_ratio: 0.5,
                max_length_ratio: 1.6,
                max_divergence: 0.18,
            },
        }
    }

    /// Accept the whole of what a normaliser does, and nothing more.
    ///
    /// Measured against S1-mini's real output over decision 0012's corpus of
    /// 21 dictations. Every cleanup the model got *right* scored **0.000**
    /// novel words; the only two above zero were both the model getting it
    /// wrong - a mangled proper noun at 0.167 and a garbled Polish sentence at
    /// 0.333 - so the threshold separates good output from bad rather than
    /// large edits from small. Growth ran 0.500-1.000 across every accepted
    /// case.
    ///
    /// The floor is the weakest of the three: nothing in the corpus came near
    /// it, so it is a backstop against a summary rather than a measured
    /// boundary.
    #[must_use]
    pub const fn full_cleanup() -> Self {
        Self {
            rule: Rule::Containment {
                min_growth: 0.35,
                max_growth: 1.10,
                max_novel_word_rate: 0.10,
            },
        }
    }
}

impl Default for Limits {
    /// Light touch. The conservative rule is the safe default for anything that
    /// forgets to choose.
    fn default() -> Self {
        Self::light_touch()
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
///
/// `vocabulary` is the user's dictionary replacements. Decision 0005 fed those
/// to the *model*, as a hint; a fine-tuned normaliser has no slot for them, so
/// they moved here instead, where they say "this word is the speaker's even
/// though they did not say it that way" - which is what they always meant.
#[must_use]
pub fn judge(original: &str, candidate: &str, limits: &Limits, vocabulary: &[String]) -> Verdict {
    let cleaned = unwrap_candidate(candidate);

    if cleaned.trim().is_empty() {
        // S1-mini returns an empty string for filler-only input, and the model
        // card calls that correct. It is not correct *here*: the caller pastes
        // the raw transcription instead, because deleting what someone said is
        // the one outcome this stage may never produce.
        return Verdict::Reject(RejectReason::Empty);
    }

    if starts_with_meta(cleaned) {
        return Verdict::Reject(RejectReason::Meta);
    }

    match limits.rule {
        Rule::Magnitude {
            min_length_ratio,
            max_length_ratio,
            max_divergence,
        } => {
            if !length_ratio_ok(original, cleaned, min_length_ratio, max_length_ratio) {
                return Verdict::Reject(RejectReason::LengthRatio);
            }
            if divergence(original, cleaned) > max_divergence {
                return Verdict::Reject(RejectReason::TooDivergent);
            }
        }
        Rule::Containment {
            min_growth,
            max_growth,
            max_novel_word_rate,
        } => {
            let growth = word_growth(original, cleaned);
            if growth < min_growth || growth > max_growth {
                return Verdict::Reject(RejectReason::LengthRatio);
            }
            if novel_word_rate(original, cleaned, vocabulary) > max_novel_word_rate {
                return Verdict::Reject(RejectReason::Invented);
            }
            if !tail_survives(original, cleaned) {
                return Verdict::Reject(RejectReason::Truncated);
            }
        }
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
fn length_ratio_ok(original: &str, candidate: &str, min: f64, max: f64) -> bool {
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

    ratio >= min && ratio <= max
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

/// Candidate words per original word, over [`normalise`]d text.
///
/// The cheap half of [`Rule::Containment`]. Normalisation only ever shortens
/// or holds a transcript, so this catches both a model that summarised (far
/// below 1) and one that answered or appended commentary (above 1), without
/// looking at *which* words changed.
fn word_growth(original: &str, candidate: &str) -> f64 {
    let source = word_count(original);
    let produced = word_count(candidate);
    if source == 0 {
        return if produced == 0 { 1.0 } else { f64::INFINITY };
    }

    // Both are the length of one utterance, far below the 2^53 at which an
    // integer stops being exactly representable; the result only decides a
    // comparison against a ratio.
    #[allow(clippy::as_conversions, clippy::cast_precision_loss)]
    let ratio = produced as f64 / source as f64;

    ratio
}

fn word_count(text: &str) -> usize {
    normalise(text).split_whitespace().count()
}

/// Fraction of the candidate's words that the speaker never said.
///
/// The load-bearing half of [`Rule::Containment`]. A normaliser is allowed to
/// throw away as much as it likes - that is the job - so the only question
/// worth asking is what it *added*.
///
/// Four things stop a legitimate normalisation being counted as invention, and
/// each is here because it fired on a measured case:
///
/// - a word already in the transcript, which is most of the output;
/// - a word carrying a digit, because "twenty five" becoming "25" and "three
///   thirty p m" becoming "3:30pm" are the feature. Counting only *all*-digit
///   words missed the second of those and failed a good cleanup;
/// - two or three consecutive transcript words run together, because "git hub"
///   becoming "GitHub" and "p m" becoming "pm" are the same feature;
/// - a term from the user's dictionary, which is the speaker telling us how
///   their own vocabulary is spelled.
///
/// What is left is a word with no source in what was said. Decision 0012
/// measured every normalisation in the corpus at exactly zero of them.
fn novel_word_rate(original: &str, candidate: &str, vocabulary: &[String]) -> f64 {
    let normalised = normalise(original);
    let source: Vec<&str> = normalised.split_whitespace().collect();
    let produced = normalise(candidate);
    let words: Vec<&str> = produced.split_whitespace().collect();

    if words.is_empty() {
        return 0.0;
    }

    let mut allowed: std::collections::HashSet<String> =
        source.iter().map(|w| (*w).to_owned()).collect();
    // Runs of two and three, which is as far as a spoken form ever splits a
    // written word in the corpus.
    for run in 2..=3 {
        for window in source.windows(run) {
            allowed.insert(window.concat());
        }
    }
    for term in vocabulary {
        for word in normalise(term).split_whitespace() {
            allowed.insert(word.to_owned());
        }
    }

    let novel = words
        .iter()
        .filter(|word| !allowed.contains(**word))
        .filter(|word| !word.chars().any(|c| c.is_ascii_digit()))
        .count();

    // Word counts of one utterance; see the note in `divergence`.
    #[allow(clippy::as_conversions, clippy::cast_precision_loss)]
    let rate = novel as f64 / words.len() as f64;

    rate
}

/// Words that carry no content, so their absence from the end means nothing.
///
/// Only used to find where the transcript's last *real* word is. Deliberately
/// short: a word wrongly listed here weakens the check, and every one of these
/// is something a normaliser is expected to drop.
const TAIL_FILLERS: &[&str] = &[
    "um", "uh", "er", "ah", "like", "you", "know", "so", "basically", "actually", "i", "mean",
    "well", "right", "just", "really", "yeah", "okay", "sort", "kind", "of",
];

/// How many of the transcript's last content words to look for.
const TAIL_WORDS: usize = 3;
/// How far back from the end of the candidate to look for them.
const TAIL_WINDOW: usize = 6;

/// Does the candidate still reach the end of what was said?
///
/// The measure [`word_growth`] cannot provide. Both a cleanup and a truncation
/// shrink the text, and on long input they shrink it by *similar amounts*:
/// filler-heavy dictation legitimately comes back at 0.60 of its word count,
/// which is the same ratio as losing the last two sentences of a paragraph. No
/// threshold on size can separate those.
///
/// What separates them is *where* the loss falls. Removing fillers thins a
/// sentence evenly and still ends where the speaker ended; a truncation stops
/// early. So this asks the only question that distinguishes them: do any of the
/// transcript's last few content words show up near the end of the candidate?
///
/// The one legitimate way for the ending to disappear is inverse text
/// normalisation rewriting it - "by three thirty p m" becoming "by 3:30pm"
/// leaves none of those words behind - so a digit in the candidate's tail
/// counts as the ending being accounted for.
fn tail_survives(original: &str, candidate: &str) -> bool {
    let normalised = normalise(original);
    let source: Vec<&str> = normalised.split_whitespace().collect();
    let produced = normalise(candidate);
    let words: Vec<&str> = produced.split_whitespace().collect();

    if source.is_empty() || words.is_empty() {
        return true;
    }

    let tail: Vec<&str> = source
        .iter()
        .filter(|word| !TAIL_FILLERS.contains(*word))
        .rev()
        .take(TAIL_WORDS)
        .copied()
        .collect();
    // Nothing but fillers to look for, so nothing to conclude.
    if tail.is_empty() {
        return true;
    }

    let start = words.len().saturating_sub(TAIL_WINDOW);
    let end: &[&str] = words.get(start..).unwrap_or(&words);

    // A rewritten ending, not a missing one.
    if end.iter().any(|word| word.chars().any(|c| c.is_ascii_digit())) {
        return true;
    }

    let mut allowed: std::collections::HashSet<String> =
        end.iter().map(|w| (*w).to_owned()).collect();
    for run in 2..=3 {
        for window in end.windows(run) {
            allowed.insert(window.concat());
        }
    }

    tail.iter().any(|word| allowed.contains(*word))
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
        judge(original, candidate, &Limits::light_touch(), &[])
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

        let Rule::Magnitude { max_divergence: limit, .. } = Limits::light_touch().rule else {
            panic!("light touch must stay a magnitude rule")
        };
        for (original, candidate) in corrections {
            let d = divergence(original, candidate);
            assert!(d <= limit, "correction scored {d:.3}, over the {limit} limit: {candidate:?}");
        }
        for (original, candidate) in rewrites {
            let d = divergence(original, candidate);
            assert!(d > limit, "rewrite scored {d:.3}, under the {limit} limit: {candidate:?}");
        }
    }

    fn judge_full(original: &str, candidate: &str) -> Verdict {
        judge(original, candidate, &Limits::full_cleanup(), &[])
    }

    fn rejected_full(original: &str, candidate: &str) -> RejectReason {
        match judge_full(original, candidate) {
            Verdict::Reject(reason) => reason,
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    /// The measurement that sets the containment thresholds (decision 0012).
    ///
    /// The twin of `measured_corrections_and_rewrites_stay_separated`, and the
    /// reason full cleanup needed a different *shape* of rule rather than a
    /// looser number: most of these score past the 0.18 divergence limit, and
    /// several fail the light-touch length ratio too.
    ///
    /// Every candidate here is **what S1-mini actually returned**, not what a
    /// normaliser might plausibly produce. That distinction earned its keep:
    /// a hand-written corpus had the model turning "cuber netties" into
    /// "Kubernetes", and the real model returns `CuberNet's`.
    #[test]
    fn measured_cleanups_are_accepted_and_bad_output_is_not() {
        // Accepted: fillers dropped, false starts resolved, numbers, currency,
        // email addresses and times written out, and text that was already
        // clean left alone.
        let accepted: &[(&str, &str)] = &[
            (
                "so um i need to like send the the report by uh friday no wait make that thursday",
                "So I need to send the report by Thursday.",
            ),
            (
                "um so i think we should probably ship it on monday",
                "So I think we should probably ship it on Monday.",
            ),
            (
                "send twenty five dollars to bartek at example dot com by three thirty p m",
                "Send $25 to bartek@example.com by 3:30pm.",
            ),
            ("uh the meeting is at noon", "The meeting is at noon."),
            (
                "i pushed the fix to git hub and it broke the build",
                "I pushed the fix to GitHub, and it broke the build.",
            ),
            (
                "hi sarah just wanted to check in about the deck uh can you send it over thanks bartek",
                "Hi Sarah, just wanted to check in about the deck. Can you send it over? Thanks, Bartek",
            ),
            (
                "okay so the plan is um we ship the beta on tuesday then we uh collect feedback for a week and then we do the real launch",
                "Okay, so the plan is we ship the beta on Tuesday, then we collect feedback for a week, and then we do the real launch.",
            ),
            // Already clean, and left that way.
            ("the build is broken on windows", "The build is broken on Windows."),
            (
                "The API returns a 500 error when the body is empty.",
                "The API returns a 500 error when the body is empty.",
            ),
            // A question is punctuated, not answered. The model is a normaliser
            // and does not take the bait, which is most of why this rule can
            // afford to be as loose as it is.
            ("what is the capital of france", "What is the capital of France?"),
            ("how do i sort a list in python", "How do I sort a list in Python?"),
            // Polish is not a language this model claims, and left alone it
            // does no harm.
            (
                "zgubilem karte kredytowa wczoraj wieczorem",
                "Zgubilem karte kredytowa wczoraj wieczorem.",
            ),
        ];
        // Rejected: both of these are the model getting it wrong, and both are
        // the *only* two cases in the corpus that scored above zero novel
        // words. The threshold separates good output from bad, not big edits
        // from small.
        let rejected: &[(&str, &str)] = &[
            // A proper noun it had never seen, mangled rather than fixed.
            (
                "lets deploy to cuber netties on friday no actually on monday",
                "Let's deploy to CuberNet's on Monday",
            ),
            // Polish, damaged: "dzien" became "dziennym" and "karte" "karta".
            (
                "dzien dobry zgubilem swoja karte kredytowa",
                "Dziennym dobry, zgubilem swoja karta kredytowa.",
            ),
        ];

        for (original, candidate) in accepted {
            let verdict = judge_full(original, candidate);
            assert!(
                matches!(verdict, Verdict::Accept(_) | Verdict::Unchanged),
                "a good cleanup was rejected as {verdict:?}: {candidate:?}"
            );
        }
        for (original, candidate) in rejected {
            assert!(
                matches!(judge_full(original, candidate), Verdict::Reject(_)),
                "bad output was accepted: {candidate:?}"
            );
        }
    }

    /// The measurement that added the tail check.
    ///
    /// The three "heavy filler" cases are real S1-mini output on long,
    /// disfluent dictation, and they come back at 0.600-0.627 of their word
    /// count with nothing lost. The three truncations sit at 0.287-0.571. The
    /// ranges *overlap*, which is the whole point: no growth threshold can
    /// separate a paragraph with its fillers removed from a paragraph missing
    /// its last two sentences. Where the loss falls is what separates them.
    #[test]
    fn a_cleanup_that_loses_the_end_is_rejected_however_much_it_keeps() {
        // Real output: filler-heavy dictation, correctly cleaned, ~0.6 growth.
        let kept: &[(&str, &str)] = &[
            (
                "um so uh i think that we should uh you know maybe um look at the the thing that we talked about uh yesterday i mean the the the deployment thing um because uh you know it is it is kind of um blocking us right now and uh i think that maybe we should just um you know go ahead and and do it uh this week if that is if that is okay with everyone um yeah",
                "So I think that we should look at the thing that we talked about yesterday, the deployment thing, because it is kind of blocking us right now. And I think that maybe we should just go ahead and do it this week, if that is okay with everyone. Yeah",
            ),
            (
                "okay so um i mean uh the thing is that we we have we have this this issue where um where the where the tests are are flaky uh you know and and i think that uh that maybe it is a timing thing um but i am not i am not totally sure uh so what i what i want to do is is um just add some retries and and see if that if that helps at all",
                "Okay, so the thing is, we have this issue where the tests are flaky, and I think that maybe it is a timing thing, but I am not totally sure. So what I want to do is just add some retries and see if that helps at all.",
            ),
        ];
        // Stopped early, and *keeping more of the text* than the cases above:
        // 0.571 of the words against their 0.600. Only the tail check can tell
        // these apart, and it is the reason each is refused.
        let truncated_past_the_growth_floor: &[(&str, &str)] = &[
            (
                "let us see i will try to create a longer transcription and see how fast it is and then i want to check the second part as well",
                "Let's see. I will try to create a longer transcription and see how fast it is.",
            ),
            (
                "i pushed the fix to git hub and it broke the build so i had to revert it again this morning",
                "I pushed the fix to GitHub, and it broke the build.",
            ),
        ];
        // Stopped early enough that the growth floor catches them first. Still
        // rejected, just for the cheaper reason.
        let truncated_below_the_growth_floor: &[(&str, &str)] = &[
            (
                "okay so um i mean uh the thing is that we we have we have this this issue where um where the where the tests are are flaky uh you know and and i think that uh that maybe it is a timing thing um but i am not i am not totally sure uh so what i what i want to do is is um just add some retries and and see if that if that helps at all",
                "Okay, so the thing is, we have this issue where the tests are flaky.",
            ),
            (
                "send twenty five dollars to bartek at example dot com and then call me back tomorrow morning about the invoice",
                "Send $25 to bartek@example.com.",
            ),
        ];

        for (original, candidate) in kept {
            let verdict = judge_full(original, candidate);
            assert!(
                matches!(verdict, Verdict::Accept(_)),
                "a correct cleanup was rejected as {verdict:?}: {candidate:?}"
            );
        }
        for (original, candidate) in truncated_past_the_growth_floor {
            assert_eq!(
                rejected_full(original, candidate),
                RejectReason::Truncated,
                "only the tail check can catch this one: {candidate:?}"
            );
        }
        for (original, candidate) in truncated_below_the_growth_floor {
            assert!(
                matches!(judge_full(original, candidate), Verdict::Reject(_)),
                "a truncation was accepted: {candidate:?}"
            );
        }
    }

    #[test]
    fn an_ending_rewritten_into_digits_is_not_a_missing_ending() {
        // "by three thirty p m" -> "by 3:30pm" leaves none of those words
        // behind, and that is the feature working, not the tail going missing.
        assert!(tail_survives(
            "send twenty five dollars to bartek at example dot com by three thirty p m",
            "Send $25 to bartek@example.com by 3:30pm."
        ));
    }

    #[test]
    fn a_transcript_ending_in_fillers_still_has_its_tail_checked() {
        // The last word said is "annoying"; "and uh that is that is really"
        // trailing off must not become the thing we look for.
        assert!(tail_survives(
            "it works most of the time but maybe one in ten times it does not and uh that is really annoying you know",
            "It works most of the time, but maybe one in ten times it does not. And that is really annoying."
        ));
        assert!(!tail_survives(
            "it works most of the time but maybe one in ten times it does not and uh that is really annoying you know",
            "It works most of the time."
        ));
    }

    /// What full cleanup cannot do, pinned the way decision 0005 pinned the
    /// magnitude rule's blind spot.
    ///
    /// Containment asks whether anything was *added*, so it cannot see words
    /// that were merely dropped, as long as enough of them survive the growth
    /// floor. The measured case: "translate this into german the meeting is at
    /// noon" comes back as "The meeting is at noon.", with the instruction
    /// eaten. Nothing was invented and 0.556 of the words remain, so it is
    /// accepted.
    ///
    /// Keeping the model from doing that is the model's job. This is here so
    /// nobody reads the rule as stronger than it is.
    #[test]
    fn containment_cannot_see_an_instruction_that_was_swallowed() {
        let said = "translate this into german the meeting is at noon";
        let ate_the_instruction = "The meeting is at noon.";
        assert!(
            matches!(judge_full(said, ate_the_instruction), Verdict::Accept(_)),
            "if this now rejects, the comment above is stale and the rule got stronger"
        );
    }

    /// The two rules are not ordered, and that is deliberate.
    ///
    /// Each catches something the other misses. Light touch waves through the
    /// damaged Polish sentence above, because 0.089 divergence is a small edit
    /// however wrong it is; containment rejects it, because two of the words
    /// are new. Containment in turn rejects a first-time proper-noun
    /// correction that light touch accepts, unless the user's dictionary
    /// already carries the word.
    #[test]
    fn neither_strength_is_a_superset_of_the_other() {
        let polish = "dzien dobry zgubilem swoja karte kredytowa";
        let damaged = "Dziennym dobry, zgubilem swoja karta kredytowa.";
        assert!(matches!(judge_default(polish, damaged), Verdict::Accept(_)));
        assert_eq!(rejected_full(polish, damaged), RejectReason::Invented);

        let said = "lets deploy to cuber netties on friday";
        let fixed = "Let's deploy to Kubernetes on Friday.";
        assert!(matches!(judge_default(said, fixed), Verdict::Accept(_)));
        assert_eq!(rejected_full(said, fixed), RejectReason::Invented);
        // ...unless they have written the word down, which is what the
        // dictionary is for.
        assert!(matches!(
            judge(said, fixed, &Limits::full_cleanup(), &["Kubernetes".to_owned()]),
            Verdict::Accept(_)
        ));
    }

    /// The rule the two strengths disagree about, on one input.
    #[test]
    fn the_two_strengths_disagree_about_filler_removal() {
        let said = "um so i think we should probably ship it on monday";
        let cleaned = "I think we should ship it on Monday.";
        // Light touch keeps the user's fillers rather than trusting a change
        // this large; full cleanup is what the model was installed for.
        assert_eq!(rejected(said, cleaned), RejectReason::TooDivergent);
        assert!(matches!(judge_full(said, cleaned), Verdict::Accept(_)));
    }

    /// Both strengths agree about the thing that actually matters.
    ///
    /// S1-mini does not do this - handed a question it punctuates it - but the
    /// guard is what makes that safe to rely on rather than hope for.
    #[test]
    fn neither_strength_accepts_an_answer_to_the_transcription() {
        let said = "what is the capital of france";
        let answered = "The capital of France is Paris.";
        assert!(matches!(judge_default(said, answered), Verdict::Reject(_)));
        assert_eq!(rejected_full(said, answered), RejectReason::Invented);
    }

    #[test]
    fn spoken_numbers_written_as_digits_do_not_count_as_invented() {
        // "twenty five" becoming "25" is the feature, not a hallucination, and
        // the digits appear nowhere in the transcript.
        assert!(novel_word_rate("i need twenty five of them", "I need 25 of them.", &[]).abs() < f64::EPSILON);
    }

    #[test]
    fn words_run_together_do_not_count_as_invented() {
        // "git hub" -> "GitHub" and "p m" -> "pm": the written form of two or
        // three spoken words, which is most of what inverse text normalisation
        // does to a proper noun.
        assert!(novel_word_rate("i pushed it to git hub", "I pushed it to GitHub.", &[]).abs() < f64::EPSILON);
        assert!(novel_word_rate("meet at three thirty p m", "Meet at 3:30 pm.", &[]).abs() < f64::EPSILON);
    }

    #[test]
    fn a_dictionary_term_is_the_speakers_word_even_when_they_did_not_say_it() {
        // The user has written down that they mean "Kubernetes"; the model
        // spelling it that way is them being understood, not invention.
        let said = "lets deploy to cuber netties";
        let cleaned = "Let's deploy to Kubernetes.";
        let vocabulary = vec!["Kubernetes".to_owned()];
        assert!(novel_word_rate(said, cleaned, &vocabulary).abs() < f64::EPSILON);
        assert!(novel_word_rate(said, cleaned, &[]) > 0.0, "without the term it is novel");
    }

    #[test]
    fn a_summary_is_rejected_even_though_it_invents_almost_nothing() {
        // The growth floor catches what the novel-word rate cannot: dropping
        // most of a sentence adds no words at all, so containment needs a
        // length check of its own. Nothing in the measured corpus came near
        // this line, so it is a backstop rather than a boundary.
        let said = "we talked about the api the database migration and the new billing screen and agreed to ship the api work first";
        assert_eq!(
            rejected_full(said, "We talked about several things."),
            RejectReason::LengthRatio
        );
    }

    #[test]
    fn an_empty_candidate_is_rejected_under_both_strengths() {
        // S1-mini returns an empty string for filler-only input and its model
        // card calls that correct. It is not correct here: the caller pastes
        // what was said instead.
        assert_eq!(rejected("um uh you know like", ""), RejectReason::Empty);
        assert_eq!(rejected_full("um uh you know like", ""), RejectReason::Empty);
    }

    #[test]
    fn growth_is_measured_in_words_not_characters() {
        // Punctuation and capitalisation add characters and no words, which is
        // why the containment rule counts words: a correct cleanup must not
        // drift towards the ceiling just for adding full stops.
        let growth = word_growth("the build is broken", "The build is broken!!!");
        assert!((growth - 1.0).abs() < f64::EPSILON, "scored {growth}");
    }

    #[test]
    fn containment_reasons_have_stable_log_labels() {
        assert_eq!(RejectReason::Invented.as_str(), "invented");
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
