# 0013 — Alternate heard forms for a dictionary entry

**Status:** accepted
**Date:** 2026-08-31
**Applies to:** the dictionary; extends decision 0005's note that a rule only helps once
the user has written it, and leaves 0012's guard and ordering untouched

The question this came from was whether the cleanup model could be fine-tuned per user, so
that a speaker who dictates about technical things stops getting their proper nouns
mangled. The answer is no, and the more useful finding is that it would not have helped.
This decision covers where the error actually is, what the rest of the field does about it,
and the small change that follows.

## The cleanup model is the wrong layer

S1-mini is a normaliser: fillers out, punctuation and capitalisation in, numbers and dates
written out. It is not a proofreader. Proofreading was Qwen2.5's job under decision 0005,
and 0005 measured it doing that badly enough to need a guard, because at 0.5B it could not
tell `Kubernetes` from `Cuber Nuts`.

Observed in use, cleanup applies normally and the nouns are still wrong afterwards. The
stage was then measured directly rather than assumed, on a dictation of `"please run the
command bun install and bun tauri dev"` through Canary 180M Flash:

| | |
|---|---|
| given the correct text | `...the command bun install and bun tauri dev.` |
| given what the recogniser returned | `...the command Bun Install and Bun Tauridev.` |

Handed clean input S1-mini leaves a shell command entirely alone, and handed mangled input
it passes the mangling through, adding a comma and a full stop. Both the capitalisation and
the joining are the recogniser's. The cleanup stage is behaving correctly at both ends, so
teaching S1-mini the word would be teaching the right answer to the wrong stage, and
`guard.rs` would reject it anyway: a word with no source in the transcript is exactly what
`novel_word_rate` exists to stop.

One caveat worth recording, because it looks like a contradiction and is not: on a short
input the model *does* mangle the same phrase, returning `"Run Buntai dev."` for
`"run bun tauri dev"`. The guard rejects that one as `Invented`. Context is what saves the
long case, and the guard is what saves the short one.

Three further things close the fine-tuning route independently of that.

- **The runtime cannot train.** `ort` is pinned to the version `transcribe-rs` resolves to
  so the tree links one ONNX Runtime. ONNX Runtime training is a different native binary,
  and its artifacts are generated from an fp32 graph. What ships is `model_q4.onnx` with
  external weights, which cannot be differentiated and cannot be re-quantised on device.
- **There is no training data.** `HistoryEntry` stores one text field, and it holds what
  was inserted, after refinement and after the dictionary. No raw transcription is kept, so
  no pair of what was heard and what was meant exists anywhere in the app.
- **Nobody does this.** Across the field, personalisation lives at the decoder or in
  post-processing, never in per-user weights. Dragon NaturallySpeaking was the last
  mainstream product to genuinely adapt per user, and it paid for it with twenty minutes of
  enrolment and by scanning the user's documents and sent email, which is the opposite of
  what this app's privacy invariant permits.

## What the field does instead

Five mechanisms, spanning the pipeline: pronunciation lexicon injection (Kaldi, Vosk,
Azure), decoder biasing (Deepgram `keyterm`, AssemblyAI `word_boost`, Google
`speechContexts`, Speechmatics `additional_vocab`), contextual attention biasing (Google
CLAS, NVIDIA Riva), prompt conditioning (Whisper's `initial_prompt`, capped at 224 tokens),
and post-processing replacement. Superwhisper splits the first and last of those into two
separate features, Vocabulary and Replacements. This app has the last one.

Two findings from that survey bear directly on what follows. The first is that only the
twenty to fifty words that are *repeatedly* mis-transcribed belong in a biasing layer, and
that a large glossary makes results worse rather than better. The second is that fuzzy
rules are where spurious replacements come from.

## Three routes, and why only one is taken

### Feeding the vocabulary to the model: closed

Decision 0005 put the user's terms in the prompt and measured them helping, and being
ignored, at roughly equal rates. Decision 0012 removed the hint because S1-mini's control
line has no slot for one. The model card settles it more firmly than 0012 could: S1-mini
"is not a chat model and will not follow general instructions", and its `Context` control
accepts only `general` and `email`. There is no technical register to select and no
instruction it would honour. The terms stay where 0012 put them, in the guard.

### Matching by sound: rejected per word, and open per phrase

Phonetic matching would catch forms the user never typed. Double Metaphone codes were
scored against the 235 974 alphabetic entries in `/usr/share/dict/words`, counting how many
English words share a term's code, and how many pairs of English words concatenate to it.

| term | code | colliding words | colliding pairs |
|---|---|---|---|
| kubernetes | `KPRNTS` | 1 | 5 |
| postgres | `PSTKRS` | 0 | 5 |
| kubectl | `KPKTL` | 0 | 4 |
| whisperfree | `ASPRFR` | 0 | 4 |
| typescript | `TPSKPT` | 1 | 3 |
| **rust** | `RST` | **59** | 2 |
| **bun** | `PN` | **125** | 1 |
| **tauri** | `TR` | **180** | 1 |

It works, and it works on the wrong half of the vocabulary. Long codes are safe:
`"cuber netties"`, `"kew bee cuttle"` and `"post gres"` all resolve correctly. Short ones
are unusable, and they are the common case. `tauri` shares `TR` with *tree*, *tour*,
*terry* and 177 others, so matching it by sound would rewrite ordinary English.

A code-length floor of five separates the two groups cleanly on this sample, so a guarded
version is possible.

**That measurement scored single words, and the terms that actually need this are
phrases.** Scored again over whole phrases, the picture inverts. Two dictations of the same
sentence through Canary 180M Flash returned `"Bun Tauridev"` and `"Bun Towery Dev"`: the
common words mangle the same way every time, and the hard word does not mangle the same way
twice. Every observed and invented form of the phrase nevertheless carries one code.

| heard as | code |
|---|---|
| bun tauri dev | `PNTRTF` |
| bun tauridev | `PNTRTF` |
| bun towery dev | `PNTRTF` |
| bun tory dev | `PNTRTF` |
| bun torrey dev | `PNTRTF` |

| term | code | colliding words | colliding pairs |
|---|---|---|---|
| tauri | `TR` | 180 | 1 |
| bun tauri dev | `PNTRTF` | **2** | 5 |
| bun install | `PNNSTL` | **0** | 5 |

A phrase code is long, so it collides with almost nothing, and it absorbs exactly the
variation a fixed alias list cannot keep up with. Phonetic matching is therefore rejected
**per word** and left open **per phrase**, as the natural extension of this decision rather
than as an alternative to it: the aliases are still where a user records a form, and a
phrase-level phonetic match would simply stop them having to record every one.

It is not built here because it needs what every threshold in this repo needs, a corpus of
real output rather than five invented forms, and because the aliases are useful on their
own and independently correct.

### Biasing the recogniser: the real fix, out of reach

Correcting the term after the fact is treating the symptom. NeMo supports context biasing
for TDT models upstream without retraining, and FluidAudio ships it locally for Parakeet at
99.3 % vocabulary precision by running a second 110M CTC model as a keyword spotter
alongside the 0.6B TDT and aligning them at 40 ms frames.

Neither is reachable from here. That machinery lives in NeMo's Python decoding stack rather
than in the exported graph, and `transcribe_rs::TranscribeOptions` carries four fields
(`language`, `translate`, `leading_silence_ms`, `trailing_silence_ms`), none of them a
biasing hook. Reaching it means a second model download and a rescorer inside a decode loop
this app does not own. Recorded here as the honest ceiling, not as work planned.

## Decision

**A dictionary entry carries an optional list of alternate heard forms, all replaced by the
same word.**

`DictionaryEntry` gains `aliases: Vec<String>`, and `apply` emits one rule per heard form
rather than one per entry. Every form joins the existing longest-first sort, so a long
alias still beats a short `input` belonging to another entry, exactly as `"react native"`
already beats `"react"`. Case-insensitivity, word-boundary anchoring and the no-rescan rule
are unchanged, because the scan itself is unchanged.

This is the shape the field already uses: Speechmatics calls it `sounds_like`, FluidAudio
calls it `aliases` beside a canonical form. The `input` field was always a one-slot version
of it, since what the user types there is the misrecognition they keep hearing.

### What deliberately did not change

- **`replacement_terms()` still returns only the replacement side.** Aliases are misheard
  forms, and 0012's reasoning holds with more force now that there are more of them:
  showing a model what a mistake looks like invites it to produce one.
- **The dictionary still runs after refinement.** Running it before as well was considered,
  so the model would see the corrected noun, but the case for it evaporated once cleanup
  turned out to be applying normally. It would also have moved the guard's baseline, since
  `judge` compares against what the model was given.

## Consequences

- **Backward compatibility is `#[serde(default)]`, not a migration.** A `dictionary.json`
  written before this has no `aliases` key and loads with an empty list, the same mechanism
  `Settings` relies on. Pinned by a test that deserialises a literal pre-change entry.
- **`add` and `update` changed signature**, so the two commands and `pipeline_check` pass
  the extra argument. Blank and duplicate aliases are dropped rather than refused: a stray
  comma in a comma-separated field is not worth an error dialog.
- **The enable checkbox round-trips the whole entry.** `toggle` in `DictionarySettings.svelte`
  sends every field back, so it has to carry `aliases` or ticking the box would erase them.
  That is the one easy regression here and it has a test on the Rust side.
- **This does not help the first time a word is misheard.** Decision 0005's limitation
  survives intact: the user still has to hear the mistake once and write it down. What
  changes is that hearing it a second, differently mangled way now costs one field instead
  of a second entry.
- **An unstable misrecognition still costs a field every time.** The measurement above says
  a hard word does not mangle the same way twice, so an alias list for one term is never
  finished. That is the argument for the phrase-level phonetic match, and the reason it is
  recorded as open rather than closed.
