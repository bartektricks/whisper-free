//! A local record of what was dictated, kept only if the user asks for it
//! (decision 0011).
//!
//! This is the one place in the app that writes transcription text to disk, and
//! it exists because the alternative users reach for is worse: re-dictating a
//! paragraph that went into the wrong window, or keeping a scratch document
//! open to paste things into. It is off by default, it says what it is keeping
//! and for how long, and it can be emptied in one click.
//!
//! Nothing here is ever logged. `tracing` gets counts, and the text goes to the
//! file the user asked for and nowhere else.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::settings::HistoryRetention;

/// The most entries kept, whatever the retention says.
///
/// A ceiling on the file rather than on the feature: "forever" with a busy
/// hotkey would otherwise grow without limit, and an entry a thousand
/// dictations old is not what anyone means by history. The oldest go first.
const MAX_ENTRIES: usize = 500;

/// Seconds in a day, as the retention arithmetic wants it.
const DAY: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: u64,
    /// Exactly what was inserted: after refinement and after the dictionary, so
    /// re-pasting an entry gives back what the user actually saw.
    pub text: String,
    /// Unix seconds. Absolute rather than relative so a file that has been
    /// sitting on disk for a week prunes correctly on the next launch.
    pub at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("could not save the history: {0}")]
    Write(#[from] std::io::Error),
    #[error("could not encode the history: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("no history entry with id {0}")]
    NotFound(u64),
}

impl HistoryError {
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::NotFound(_) => "That transcription is no longer in the history.".into(),
            Self::Write(_) | Self::Encode(_) => {
                "The history could not be saved. Check that there is enough free disk space."
                    .into()
            }
        }
    }
}

/// Where the history file lives, beside `settings.json` and `dictionary.json`.
#[must_use]
pub fn history_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("history.json")
}

/// The current time as unix seconds.
///
/// A clock before 1970 is not worth an error path; it reads as zero, which
/// makes every entry look old rather than making the app fail.
#[must_use]
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

impl HistoryRetention {
    /// The oldest timestamp still worth keeping, or `None` when age is not the
    /// thing that decides.
    ///
    /// Pure, so the rule can be checked without a clock or a file.
    /// [`Self::Forever`] keeps everything by definition, and [`Self::Session`]
    /// keeps everything it has because it only ever has this run's entries:
    /// what makes it different is that it never reaches the disk, which is
    /// [`Self::persists`], not this.
    #[must_use]
    pub const fn cutoff(self, now: u64) -> Option<u64> {
        let days: u64 = match self {
            Self::Session | Self::Forever => return None,
            Self::OneDay => 1,
            Self::SevenDays => 7,
            Self::ThirtyDays => 30,
        };
        // Saturating rather than checked: a clock that has not reached the
        // retention window yet means nothing is old enough to drop.
        Some(now.saturating_sub(days.saturating_mul(DAY)))
    }

    /// Whether entries under this retention are written to disk at all.
    #[must_use]
    pub const fn persists(self) -> bool {
        !matches!(self, Self::Session)
    }
}

/// Bring the kept history up at launch, matching what the settings ask for.
///
/// Reads the file only when the settings say it should exist, and removes it
/// whenever they say it should not: a user who chose `Session`, or switched the
/// feature off, may have done so after a spell of keeping things on disk, and
/// the file left behind is the part that would matter to them.
///
/// Pruning happens here rather than on first read so that entries which aged
/// out while the app was closed are gone before anything can show them.
#[must_use]
pub fn open(enabled: bool, retention: HistoryRetention, path: &Path) -> History {
    let keeps_on_disk = enabled && retention.persists();

    let mut history = if keeps_on_disk {
        History::load(path)
    } else {
        History::default()
    };

    if !keeps_on_disk {
        if let Err(e) = History::forget_on_disk(path) {
            tracing::warn!(error = %e, "could not remove the history file");
        }
    }

    let dropped = history.prune(retention, now());
    if dropped > 0 {
        tracing::info!(event = "history_pruned", dropped);
    }
    history
}

impl History {
    /// Read the history from `path`.
    ///
    /// Fault-tolerant in the same way settings and the dictionary are: an
    /// unreadable or corrupt file starts empty with a warning rather than
    /// blocking startup. The warning names the file and never its contents.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, "could not read the history, starting empty");
                return Self::default();
            }
        };
        serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "history file is invalid, starting empty");
            Self::default()
        })
    }

    /// Write the history to `path` via a temporary file and a rename.
    ///
    /// # Errors
    ///
    /// [`HistoryError::Write`] when the file cannot be created or renamed,
    /// [`HistoryError::Encode`] when serialisation fails.
    pub fn save(&self, path: &Path) -> Result<(), HistoryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Remove the file, for a retention that must not leave one behind.
    ///
    /// Missing is success: the point is that nothing is there afterwards.
    ///
    /// # Errors
    ///
    /// [`HistoryError::Write`] when a file that does exist cannot be removed.
    pub fn forget_on_disk(path: &Path) -> Result<(), HistoryError> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(HistoryError::Write(e)),
        }
    }

    fn next_id(&self) -> u64 {
        self.entries
            .iter()
            .map(|e| e.id)
            .max()
            .map_or(1, |highest| highest.saturating_add(1))
    }

    /// Add what was just inserted, newest first.
    ///
    /// Blank text is dropped rather than recorded: an empty transcription is
    /// already surfaced as its own outcome and never reaches insertion, so an
    /// empty entry here could only be noise.
    pub fn record(&mut self, text: &str, at: u64) {
        if text.trim().is_empty() {
            return;
        }
        let entry = HistoryEntry {
            id: self.next_id(),
            text: text.to_owned(),
            at,
        };
        self.entries.insert(0, entry);
        self.entries.truncate(MAX_ENTRIES);
    }

    /// Drop everything older than `retention` allows.
    ///
    /// Returns how many went, so the caller can decide whether the file is
    /// worth rewriting.
    pub fn prune(&mut self, retention: HistoryRetention, now: u64) -> usize {
        let before = self.entries.len();
        if let Some(cutoff) = retention.cutoff(now) {
            self.entries.retain(|entry| entry.at >= cutoff);
        }
        self.entries.truncate(MAX_ENTRIES);
        before.saturating_sub(self.entries.len())
    }

    /// Remove one entry.
    ///
    /// # Errors
    ///
    /// [`HistoryError::NotFound`] when nothing has that id.
    pub fn remove(&mut self, id: u64) -> Result<(), HistoryError> {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.id != id);
        if self.entries.len() == before {
            return Err(HistoryError::NotFound(id));
        }
        Ok(())
    }

    /// The text of one entry, for putting back on the clipboard.
    #[must_use]
    pub fn text_of(&self, id: u64) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.text.as_str())
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(entries: &[(u64, u64)]) -> History {
        History {
            entries: entries
                .iter()
                .map(|(id, at)| HistoryEntry {
                    id: *id,
                    text: format!("entry {id}"),
                    at: *at,
                })
                .collect(),
        }
    }

    #[test]
    fn forever_and_session_never_drop_anything_for_age() {
        assert_eq!(HistoryRetention::Forever.cutoff(1_000_000), None);
        assert_eq!(HistoryRetention::Session.cutoff(1_000_000), None);
    }

    #[test]
    fn each_window_cuts_at_its_own_age() {
        let now = 100 * DAY;
        assert_eq!(HistoryRetention::OneDay.cutoff(now), Some(99 * DAY));
        assert_eq!(HistoryRetention::SevenDays.cutoff(now), Some(93 * DAY));
        assert_eq!(HistoryRetention::ThirtyDays.cutoff(now), Some(70 * DAY));
    }

    #[test]
    fn a_clock_younger_than_the_window_keeps_everything() {
        // Saturating, not panicking: a machine whose clock reads a few hours
        // past the epoch must not take the app down with it.
        assert_eq!(HistoryRetention::ThirtyDays.cutoff(60), Some(0));
    }

    #[test]
    fn only_the_session_retention_stays_off_the_disk() {
        assert!(!HistoryRetention::Session.persists());
        for retention in [
            HistoryRetention::OneDay,
            HistoryRetention::SevenDays,
            HistoryRetention::ThirtyDays,
            HistoryRetention::Forever,
        ] {
            assert!(retention.persists(), "{retention:?} should reach the disk");
        }
    }

    #[test]
    fn pruning_keeps_an_entry_exactly_on_the_boundary() {
        let now = 100 * DAY;
        let mut history = at(&[(1, 93 * DAY), (2, 93 * DAY - 1)]);
        assert_eq!(history.prune(HistoryRetention::SevenDays, now), 1);
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries.first().map(|e| e.id), Some(1));
    }

    #[test]
    fn newest_is_first_and_ids_do_not_repeat() {
        let mut history = History::default();
        history.record("first", 10);
        history.record("second", 20);
        let ids: Vec<u64> = history.entries.iter().map(|e| e.id).collect();
        let texts: Vec<&str> = history.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, vec!["second", "first"]);
        assert_eq!(ids, vec![2, 1]);
    }

    #[test]
    fn an_empty_transcription_is_never_recorded() {
        let mut history = History::default();
        history.record("   \n ", 10);
        assert!(history.entries.is_empty());
    }

    #[test]
    fn the_cap_drops_the_oldest_and_never_the_newest() {
        let mut history = History::default();
        for i in 0..(MAX_ENTRIES + 10) {
            history.record(&format!("entry {i}"), 100);
        }
        assert_eq!(history.entries.len(), MAX_ENTRIES);
        assert_eq!(
            history.entries.first().map(|e| e.text.as_str()),
            Some(format!("entry {}", MAX_ENTRIES + 9).as_str())
        );
    }

    #[test]
    fn forever_is_still_bounded_by_the_cap() {
        // "Forever" is about age, not about an unbounded file.
        let mut history = History::default();
        for i in 0..(MAX_ENTRIES + 5) {
            history.record(&format!("entry {i}"), 100);
        }
        assert_eq!(history.prune(HistoryRetention::Forever, 1_000_000), 0);
        assert_eq!(history.entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn removing_something_that_is_not_there_says_so() {
        let mut history = at(&[(1, 10)]);
        assert!(history.remove(2).is_err());
        assert!(history.remove(1).is_ok());
        assert!(history.entries.is_empty());
    }

    #[test]
    fn user_messages_never_leak_internals() {
        let errors = [
            HistoryError::NotFound(7),
            HistoryError::Encode(serde_json::from_str::<History>("{").unwrap_err()),
        ];
        for e in errors {
            let msg = e.user_message();
            for leak in ["serde", "json", "EOF", "io::"] {
                assert!(!msg.contains(leak), "leaked internals: {msg}");
            }
        }
    }

    #[test]
    fn saves_and_loads_round_trip() {
        let dir = std::env::temp_dir().join("whisperfree-history-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = history_path(&dir);

        let mut history = History::default();
        history.record("something said out loud", 42);
        history.save(&path).unwrap();
        assert_eq!(History::load(&path), history);

        History::forget_on_disk(&path).unwrap();
        assert_eq!(History::load(&path), History::default());
        // Removing what is already gone is success, not an error.
        assert!(History::forget_on_disk(&path).is_ok());
    }
}
