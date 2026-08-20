//! Deterministic post-processing of transcriptions (plan §12).
//!
//! This is not training and not learning — it is a list of replacements the
//! user controls, applied to the text before it is inserted.
//!
//! Matching is word-boundary aware on purpose. A naive `str::replace` of
//! "type script" would happily corrupt "prototype scripting", and a rule for
//! "go" would rewrite the middle of "google".

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryEntry {
    pub id: u64,
    /// What the model tends to produce.
    pub input: String,
    /// What it should say instead.
    pub replacement: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dictionary {
    pub entries: Vec<DictionaryEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum DictionaryError {
    #[error("could not save the dictionary: {0}")]
    Write(#[from] std::io::Error),
    #[error("could not encode the dictionary: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("no dictionary entry with id {0}")]
    NotFound(u64),
    #[error("the recognised text cannot be empty")]
    EmptyInput,
}

impl DictionaryError {
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::EmptyInput => {
                "Enter the text the model produces before saving the entry.".into()
            }
            Self::NotFound(_) => "That dictionary entry no longer exists.".into(),
            Self::Write(_) | Self::Encode(_) => {
                "The dictionary could not be saved. Check that there is enough free disk space."
                    .into()
            }
        }
    }
}

/// Is `c` part of a word for boundary purposes?
///
/// Unicode-aware, so Polish letters count as word characters and a rule for
/// "kot" does not fire inside "kotłownia".
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Dictionary {
    pub fn load(path: &Path) -> Self {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, "could not read the dictionary, starting empty");
                return Self::default();
            }
        };
        serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "dictionary file is invalid, starting empty");
            Self::default()
        })
    }

    /// Write the dictionary to `path` via a temporary file and a rename.
    ///
    /// # Errors
    ///
    /// [`DictionaryError::Write`] when the file cannot be created or renamed,
    /// [`DictionaryError::Encode`] when serialisation fails.
    pub fn save(&self, path: &Path) -> Result<(), DictionaryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    fn next_id(&self) -> u64 {
        self.entries
            .iter()
            .map(|e| e.id)
            .max()
            .map_or(1, |highest| highest.saturating_add(1))
    }

    /// Append a new entry.
    ///
    /// # Errors
    ///
    /// [`DictionaryError::EmptyInput`] when `input` is blank.
    pub fn add(
        &mut self,
        input: &str,
        replacement: &str,
    ) -> Result<DictionaryEntry, DictionaryError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(DictionaryError::EmptyInput);
        }
        let entry = DictionaryEntry {
            id: self.next_id(),
            input: input.to_string(),
            replacement: replacement.trim().to_string(),
            enabled: true,
        };
        self.entries.push(entry.clone());
        Ok(entry)
    }

    /// Replace the contents of the entry with `id`.
    ///
    /// # Errors
    ///
    /// [`DictionaryError::EmptyInput`] when `input` is blank, or
    /// [`DictionaryError::NotFound`] when no entry has that id.
    pub fn update(
        &mut self,
        id: u64,
        input: &str,
        replacement: &str,
        enabled: bool,
    ) -> Result<DictionaryEntry, DictionaryError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(DictionaryError::EmptyInput);
        }
        let entry = self
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or(DictionaryError::NotFound(id))?;
        entry.input = input.to_string();
        entry.replacement = replacement.trim().to_string();
        entry.enabled = enabled;
        Ok(entry.clone())
    }

    /// Delete the entry with `id`.
    ///
    /// # Errors
    ///
    /// [`DictionaryError::NotFound`] when no entry has that id.
    pub fn remove(&mut self, id: u64) -> Result<(), DictionaryError> {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        if self.entries.len() == before {
            return Err(DictionaryError::NotFound(id));
        }
        Ok(())
    }

    /// The correctly-spelled side of every enabled rule.
    ///
    /// Handed to the refinement model as the words this speaker uses, so a
    /// name it has never seen is not "corrected" into something it has
    /// (decision 0005). The `input` side is deliberately left out: those are
    /// the misheard forms, and showing a model what a mistake looks like
    /// invites it to produce one.
    #[must_use]
    pub fn replacement_terms(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.enabled && !e.replacement.trim().is_empty())
            .map(|e| e.replacement.trim().to_owned())
            .collect()
    }

    /// Apply every enabled entry to `text`.
    ///
    /// Matching is case-insensitive and anchored to word boundaries. Longer
    /// inputs are tried first, so "react native" beats a separate "react" rule.
    /// Replacements are never re-scanned, so a rule cannot feed itself.
    #[must_use]
    pub fn apply(&self, text: &str) -> String {
        let mut rules: Vec<(&str, &str)> = self
            .entries
            .iter()
            .filter(|e| e.enabled && !e.input.trim().is_empty())
            .map(|e| (e.input.as_str(), e.replacement.as_str()))
            .collect();

        if rules.is_empty() {
            return text.to_string();
        }
        rules.sort_by_key(|(input, _)| std::cmp::Reverse(input.chars().count()));

        let lowered: Vec<char> = text.to_lowercase().chars().collect();
        let chars: Vec<char> = text.chars().collect();
        // to_lowercase can change the character count (e.g. 'İ'), which would
        // desynchronise the two buffers. Fall back to the untouched text.
        if lowered.len() != chars.len() {
            return text.to_string();
        }

        let lowered_rules: Vec<(Vec<char>, &str)> = rules
            .iter()
            .map(|(input, replacement)| (input.to_lowercase().chars().collect(), *replacement))
            .collect();

        let mut out = String::with_capacity(text.len());
        let mut i = 0usize;

        'outer: while let Some(&current) = chars.get(i) {
            let at_start_boundary = i
                .checked_sub(1)
                .and_then(|previous| chars.get(previous))
                .is_none_or(|c| !is_word_char(*c));
            if at_start_boundary {
                for (needle, replacement) in &lowered_rules {
                    let end = i.saturating_add(needle.len());
                    if lowered.get(i..end) != Some(needle.as_slice()) {
                        continue;
                    }
                    // The character after the match must not continue the word.
                    if chars.get(end).is_some_and(|c| is_word_char(*c)) {
                        continue;
                    }
                    out.push_str(replacement);
                    i = end;
                    continue 'outer;
                }
            }
            out.push(current);
            i = i.saturating_add(1);
        }

        out
    }
}

#[must_use]
pub fn dictionary_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("dictionary.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(pairs: &[(&str, &str)]) -> Dictionary {
        let mut d = Dictionary::default();
        for (input, replacement) in pairs {
            d.add(input, replacement).unwrap();
        }
        d
    }

    #[test]
    fn an_empty_dictionary_leaves_text_alone() {
        assert_eq!(Dictionary::default().apply("I'm using cotlin"), "I'm using cotlin");
    }

    #[test]
    fn replaces_a_whole_word() {
        let d = dict(&[("cotlin", "Kotlin")]);
        assert_eq!(d.apply("I'm using cotlin"), "I'm using Kotlin");
    }

    #[test]
    fn matching_ignores_case() {
        let d = dict(&[("cotlin", "Kotlin")]);
        assert_eq!(d.apply("Cotlin is nice"), "Kotlin is nice");
        assert_eq!(d.apply("COTLIN is nice"), "Kotlin is nice");
    }

    #[test]
    fn does_not_replace_inside_a_longer_word() {
        // The whole point of word boundaries.
        let d = dict(&[("go", "Go")]);
        assert_eq!(d.apply("google is not go"), "google is not Go");
    }

    #[test]
    fn does_not_replace_a_prefix_of_a_longer_word() {
        let d = dict(&[("type script", "TypeScript")]);
        assert_eq!(d.apply("prototype scripting"), "prototype scripting");
    }

    #[test]
    fn replaces_multi_word_inputs() {
        let d = dict(&[("react native", "React Native")]);
        assert_eq!(d.apply("built in react native"), "built in React Native");
    }

    #[test]
    fn longer_rules_win_over_shorter_overlapping_ones() {
        let d = dict(&[("react", "React"), ("react native", "React Native")]);
        assert_eq!(d.apply("react native app"), "React Native app");
        assert_eq!(d.apply("a react app"), "a React app");
    }

    #[test]
    fn punctuation_counts_as_a_boundary() {
        let d = dict(&[("cotlin", "Kotlin")]);
        assert_eq!(d.apply("Use cotlin, please."), "Use Kotlin, please.");
        assert_eq!(d.apply("(cotlin)"), "(Kotlin)");
    }

    #[test]
    fn replaces_every_occurrence() {
        let d = dict(&[("cotlin", "Kotlin")]);
        assert_eq!(d.apply("cotlin and cotlin"), "Kotlin and Kotlin");
    }

    #[test]
    fn disabled_entries_are_skipped() {
        let mut d = dict(&[("cotlin", "Kotlin")]);
        d.update(1, "cotlin", "Kotlin", false).unwrap();
        assert_eq!(d.apply("using cotlin"), "using cotlin");
    }

    #[test]
    fn polish_words_respect_boundaries() {
        // "kot" must not fire inside "kotłownia" — is_alphanumeric has to be
        // Unicode-aware for this to hold.
        let d = dict(&[("kot", "kot")]);
        assert_eq!(d.apply("kotłownia"), "kotłownia");
    }

    #[test]
    fn polish_diacritics_match_case_insensitively() {
        let d = dict(&[("łódź", "Łódź")]);
        assert_eq!(d.apply("jadę do łódź"), "jadę do Łódź");
        assert_eq!(d.apply("jadę do ŁÓDŹ"), "jadę do Łódź");
    }

    #[test]
    fn a_replacement_is_not_rescanned() {
        // Otherwise "c" -> "cc" would loop forever, and rules could cascade in
        // ways the user never asked for.
        let d = dict(&[("a", "a a")]);
        assert_eq!(d.apply("a"), "a a");
    }

    #[test]
    fn an_empty_replacement_deletes_the_word() {
        let d = dict(&[("um", "")]);
        assert_eq!(d.apply("so um yes"), "so  yes");
    }

    #[test]
    fn entries_need_a_non_empty_input() {
        let mut d = Dictionary::default();
        assert!(matches!(d.add("   ", "x"), Err(DictionaryError::EmptyInput)));
    }

    #[test]
    fn ids_are_unique_and_survive_deletion() {
        let mut d = dict(&[("a", "A"), ("b", "B")]);
        d.remove(1).unwrap();
        let third = d.add("c", "C").unwrap();
        // Reusing id 1 would let a stale UI reference the wrong entry.
        assert_eq!(third.id, 3);
    }

    #[test]
    fn removing_a_missing_entry_is_an_error() {
        let mut d = Dictionary::default();
        assert!(matches!(d.remove(42), Err(DictionaryError::NotFound(42))));
    }

    #[test]
    fn saves_and_loads_round_trip() {
        let dir = std::env::temp_dir().join("whisperfree-dict-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dictionary_path(&dir);

        let d = dict(&[("cotlin", "Kotlin"), ("type script", "TypeScript")]);
        d.save(&path).unwrap();
        assert_eq!(Dictionary::load(&path), d);
    }

    #[test]
    fn a_corrupt_file_starts_empty_instead_of_failing() {
        let dir = std::env::temp_dir().join("whisperfree-dict-corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dictionary_path(&dir);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Dictionary::load(&path), Dictionary::default());
    }

    #[test]
    fn the_plan_example_works_end_to_end() {
        let d = dict(&[("cotlin", "Kotlin")]);
        assert_eq!(d.apply("I'm using cotlin"), "I'm using Kotlin");
    }
}
