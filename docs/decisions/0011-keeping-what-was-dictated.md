# 0011 — Keeping what was dictated

**Status:** accepted
**Date:** 2026-08-28
**Applies to:** `settings/`, the new `history/`, `text_insertion/`, `dictation.rs`, `commands/`, Settings › History, onboarding
**Amends:** the privacy invariant in `CLAUDE.md`

Two things users do by hand that the app can do for them. They dictate a paragraph into
the wrong window and have to say it again, because the clipboard was put back and the
words are gone. And they keep a scratch document open to paste transcriptions into,
because there is nowhere else for them to live.

This decision covers both: an option to leave the transcription on the clipboard, and an
opt-in local list of what was dictated.

## This amends the privacy invariant, and the amendment is narrow

The invariant said "never log or persist audio samples, transcription text, clipboard
contents, or dictionary entries". The history writes transcription text to disk, so the
sentence is now wrong as written, and pretending otherwise would be worse than changing
it.

What is amended is exactly one clause, and only under a setting:

- **Audio is untouched.** It stays in memory and never reaches disk, as before. Nothing
  here gets near a sample.
- **Logging is untouched.** `tracing` still gets shapes only. `history/` logs counts and
  event names, never text, and the modules that handle the entries say so where someone
  might be tempted.
- **The network is untouched.** Nothing here leaves the machine.
- **Transcription text may be persisted, if the user switches it on.** That is the whole
  of the change.

The precedent is decision 0006, which amended the same paragraph's "no network call"
sentence for update checks, and did it the same way: off by default, one setting, and the
sentence in `CLAUDE.md` rewritten rather than quietly falsified.

## Off by default, and it stays off by default

`mute_while_recording` defaults to `true` so existing installs pick it up (decision 0009).
`history_enabled` must not, and the reasoning is the mirror image of that one. Muting asks
nothing of the user, shows itself the first time it happens, and is one checkbox to undo.
Writing down what someone said is none of those things. Inheriting it from `Default` would
mean an install that never asked for it starts keeping a file of the user's words, and the
first they would know is finding it.

So it is switched on by a person who has read what it does. That person is given the
sentence to read twice: on the onboarding panel and in Settings › History.

`keep_on_clipboard` is off by default too, for a smaller reason. It is the one setting
here that takes something away rather than adding it: the clipboard the user was holding.

## `Session` is a retention, not a separate feature

The obvious four windows are 24 hours, 7 days, 30 days and forever. The fifth,
"Until I quit WhisperFree", is the one worth explaining: entries are kept in memory and
never written to disk at all.

It is there because the two things a user might want from this feature are not the same
want. "Where did that paragraph go" is answered within the hour and needs no file.
"What did I dictate last Tuesday" needs one. Folding the first into the second would make
anyone with the first want pay for it in a file they did not need, and the honest way to
offer it is as the shortest window rather than as a second checkbox.

`HistoryRetention::persists` is the one place that distinction lives, and it is a
different question from `cutoff`, which is why they are two methods. Choosing `Session`,
or switching the feature off, **deletes the file** rather than just stopping writes: a
user who has been keeping things for a month and then changes their mind means the file,
not the future.

Both are pure and carry their own tests, the same split as `hotkey::decide` and
`state::is_valid`.

## The list is bounded twice

By age, which the user chooses, and by count, which they do not: `MAX_ENTRIES` is 500 and
the oldest go first. "Forever" is about age and was never a promise of an unbounded file,
and an entry a thousand dictations old is not what anyone means by history. A test pins
that `Forever` is still capped, because that is the pair a future reader is most likely to
assume contradicts itself.

## Keeping a transcription can never cost a dictation

`remember` in `dictation.rs` runs *after* the insertion has succeeded, and it is advisory
in the sense refinement is (decision 0005) and muting is (decision 0009): a poisoned lock
or a full disk is a log line and nothing else. The words are already in the user's
document by then. Telling them their dictation failed, when what actually failed was the
copy of it, would be a worse outcome than losing the copy.

The text recorded is the final text, after refinement and after the dictionary, because
that is what the user saw and what re-pasting should give back.

## The clipboard option is a parameter, not a second method

`TextInserter::insert` takes `keep_on_clipboard`. The paste is identical either way; the
only difference is whether the last step happens. When it is set, the backend does not
even take the snapshot decision 0010 added, since reading every flavour costs real work
for something about to be discarded, and the outcome is `ClipboardOutcome::Kept`, which
`lost_the_clipboard` correctly reports as no loss: the previous clipboard was given up on
purpose.

The setting is read once in `insert`, before the paste, so a settings change landing
mid-insertion governs the next dictation rather than changing this one halfway.

## One onboarding panel for both

Onboarding was already six or seven steps and these are one question asked over two
timescales: whether the text is still to hand a second later, and whether it is still
there next week. A single "Your text" panel carries both, placed after Cleanup, which is
the other step about what happens to words rather than about permissions or downloads.

Like the muting step it is a preference rather than a task, so the primary button always
reads "Continue": there is nothing here to skip, because leaving both boxes alone is a
complete answer.

## Consequences

- `settings.json` gains `keep_on_clipboard`, `history_enabled` and `history_retention`.
  All three are `#[serde(default)]` like the rest, so an older file picks up the defaults,
  and all three defaults are the conservative direction.
- `history.json` exists only while the settings say it should. `history::open` at launch
  reads it, prunes it, and removes it when they say it should not exist, so switching the
  feature off and on again does not resurrect entries that had aged out.
- `get_history` prunes on the way out as well, so a window left open overnight does not go
  on offering entries that have passed the retention.
- `copy_history_entry` is a Rust command rather than the clipboard plugin's JS half,
  because the settings window holds no clipboard capability and Rust owns the clipboard
  everywhere else in the app.
- `history_changed` is its own event, so a dictation performed while the window is open
  appears in the list without the user navigating away and back.
- The history is never logged. Counts and event names only, like everything else.
