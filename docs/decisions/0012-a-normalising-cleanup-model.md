# 0012 — A normalising cleanup model

**Status:** accepted
**Date:** 2026-08-30
**Applies to:** the refinement stage; replaces decision 0005's model and guard rule, and
keeps everything else it decided

Decision 0005 chose Qwen2.5 0.5B Instruct to proofread transcriptions, and built a guard
that throws away anything straying too far from what was said. It works, and it costs about
a second per dictation to fix a word every few dozen utterances, which is why it shipped off
by default and mostly stayed off.

[S1-mini](https://huggingface.co/superwhisper/s1-mini) is a 0.6B model fine-tuned for this
exact task by Superwhisper: it takes a raw ASR transcript and returns clean written text,
with fillers removed, false starts resolved to whatever the speaker landed on, punctuation
and capitalisation applied, and spoken numbers, dates, times, currency and email addresses
written out. This decision covers adopting it, the guard it needed, and where the second
went.

## Measurement setup

| | |
|---|---|
| Machine | Apple M1 Pro, 8 performance + 2 efficiency cores, 32 GB, macOS 15.7.3 (arm64) |
| Runtime | `ort` 2.0.0-rc.12, CPU execution provider, greedy decoding |
| Harness | `cargo run --release --example refine_check` |
| Cases | 18: 10 needing cleanup, 3 already correct, 2 Polish, 3 baiting the model into answering |

The corpus is wider than 0005's eleven, and deliberately so. A normaliser has to be judged
on filler removal, false starts and written-out numbers, none of which the old corpus
contained; and on paragraph-length input, because the old one averaged 9.6 generated tokens
and hid everything decode-bound.

## It is a Qwen3, which is why this is cheap

S1-mini is a fine-tune of Qwen3-0.6B, so the decode loop decision 0005 wrote fits it
unchanged: same cached decoder-only shape, same greedy argmax, same `present.N.*` fed back
as `past_key_values.N.*`. `onnx-community/s1-mini-ONNX` publishes the export. No new
dependency, nothing added to the bundle, and 0005's reasoning for `ort` over `llama-cpp-2`
and `candle` carries over untouched.

Three things about the export were **not** what 0005 recorded for the Qwen2.5 one, all found
by loading the graph before writing any Rust, and all worth writing down because guessing
wrong is a hard refusal from the runtime rather than a silent fallback:

- **There is no `position_ids` input.** Positions are derived inside the graph from
  `attention_mask`. The word does not appear anywhere in the graph.
- **`num_logits_to_keep` is required**, and is not a formality. The language-model head is
  151 936 × 1 024; without it that runs over every prompt position rather than the one whose
  logits are read.
- **It uses `com.microsoft.GroupQueryAttention`**, which 0005 specifically noted the Qwen2.5
  export did not. It needs no extra graph inputs, so nothing has to be reconstructed.

`refine/onnx.rs` now reads which of those inputs exist off the graph rather than assuming.

## Measured results

| | Qwen2.5 0.5B (0005) | **S1-mini** |
|---|---|---|
| Download | 490 MB | **412 MB** |
| Cases passed | 9/11 | **17/18** |
| Load | 906 ms | **462 ms** |
| Mean latency | 1 063 ms | **571 ms** |
| Prefill | 546 ms / 192 tok | **79 ms / 92 tok** |
| Decode | 517 ms | 493 ms |
| Decode rate | 18.5 tok/s | **28.2 tok/s** |
| Mean new tokens | 9.6 | 13.9 |

Faster on a harder corpus, on every axis, at 78 MB less. The one failure is
`"whisper free"` coming back as `"Whisper Free"` rather than `"WhisperFree"`; see
*Consequences*.

### Where the second went

Four levers, measured one at a time. The order matters: two of them are worth more than the
model change.

| | mean | prefill | decode |
|---|---|---|---|
| `q4f16`, 6 threads | 981 ms | 142 ms | 22.0 tok/s |
| `q4`, runtime's own thread choice | 2 067 ms | 653 ms | 13.1 tok/s |
| `q4`, 6 threads | 774 ms | 215 ms | 30.4 tok/s |
| `q4`, 6 threads, cached prefix | **639 ms** | **84 ms** | **33.3 tok/s** |

**Thread count is the largest single lever and the least obvious.** Left alone the runtime
picks something near the logical core count, and on a machine with efficiency cores that is
**2.8× slower** than six threads, because every thread waits for the slowest. The sweep: 1 → 3 832 ms,
2 → 1 930, 4 → 998, 5 → 909, 6 → 774, 7 → 822, 8 → 1 315, 10 → 2 067. `MAX_INTRA_THREADS` is
six, capped by `available_parallelism`, and it is one machine's number in one named constant.

**`q4` beats `q4f16`** at 33.3 tok/s against 22.0, and loads in 658 ms against 1 141. The
CPU provider has no fp16 kernels to reach for and pays for the casts. It is 48 MB larger to
download and keeps the cache and logits in f32, which is what the decode loop already reads.

**The cached prefix** takes prefill from 215 ms to 84 ms. The system turn and control line
are fixed for a given styling and are 69 of the 78 fixed prompt tokens, so they run through
the graph once and the tensors are passed back as views on every dictation after that.

**The prompt collapsed from 192 tokens to 78.** 0005 found that a worked example was the
single biggest quality lever at that size; a model fine-tuned on the task does not need one,
and the vocabulary block had nowhere to go in S1-mini's control line.

### Prompt-lookup speculative decoding, built and rejected

A normalised transcript is mostly a copy of its input, so drafting the next tokens from the
prompt and verifying them in one pass should have been the biggest win of all. It was built,
and verified token-identical to greedy on every case.

It returned **11 %**, and was *slower* on the shortest case (368 ms → 423 ms), at a 4–25 %
draft acceptance rate.

The premise was wrong. A cleanup copies its input at the *character* level but not at the
*token* level: `so` → `So` and `windows` → `Windows` retokenise, and casing changes land at
exactly the points an n-gram lookup anchors on. 11 % does not buy ~150 lines of the one
component here whose bugs are silent (a cache trimmed to the wrong length produces fluent,
wrong text rather than an error), so it is out.

## The guard had to change shape, not threshold

0005's guard bounds *how much* changed: length ratio 0.5–1.6, normalised character
divergence ≤ 0.18. That is the right question for a proofreader and the wrong one for a
normaliser, which is *supposed* to change a lot. Scored against those numbers, S1-mini's own
model-card example (`"so um i need to like send the the report by uh friday no wait make
that thursday"` → `"I need to send the report by Thursday."`) is rejected twice over, at
0.537 divergence and a 0.475 length ratio.

Run over the corpus, the light-touch rule rejects three real cleanups outright and scores
**14/18** against full cleanup's 17/18.

So full cleanup asks a different question: **deletions are free, invention is not.** Two
measures over normalised words:

- **Novel-word rate**: the fraction of candidate words absent from the transcript. A word
  is not novel when it carries a digit (`"twenty five"` → `"25"`, `"three thirty p m"` →
  `"3:30pm"`), when it is two or three consecutive transcript words run together
  (`"git hub"` → `"GitHub"`), or when it matches one of the user's dictionary replacements.
- **Word growth**: candidate words over transcript words.
- **Tail survival**: whether any of the transcript's last few content words
  still appears near the end of the candidate.

Calibrated against 21 dictations of **real S1-mini output**, not plausible-looking output.
That distinction earned its keep immediately: a hand-written corpus had the model turning
`"cuber netties"` into `"Kubernetes"`, and the real model returns `"CuberNet's"`.

| | novel rate | growth |
|---|---|---|
| every cleanup the model got right (19 cases) | **0.000** | 0.500 – 1.000 |
| a proper noun it had never seen, mangled | 0.167 | 0.545 |
| a Polish sentence, damaged | 0.333 | 1.000 |

Only two cases in the whole corpus scored above zero, and **both are the model getting it
wrong**. The threshold is 0.10, and it separates good output from bad rather than large
edits from small, a much wider gap than the 0.105/0.190 one 0005 had to work with. Growth
is bounded to 0.35–1.10; nothing measured came near the floor, so that one is a backstop
against a summary rather than a boundary, and it is the weakest of the three numbers.

A content-retention measure was tried and dropped: it scored 0.400 on a legitimate
number-conversion case, below several rewrites, so it separates nothing the growth floor
does not already catch.

### Why growth alone was not enough

The first version of this rule had only the two measures above, and the growth floor was
set at 0.35 because nothing in the corpus came near it. That corpus was the problem: every
case in it was one or two sentences.

Measured again on long dictation, the picture changes completely. Real S1-mini output on
80-word transcripts:

| | growth |
|---|---|
| long, moderate filler | 0.923 – 0.976 |
| long, heavy filler and repetition (correct output, nothing lost) | **0.600 – 0.627** |
| the same inputs truncated to their first half or two-thirds | 0.287 – 0.571 |

**The ranges overlap.** A filler-heavy paragraph legitimately comes back at 0.60 of its
word count, and a paragraph missing its last two sentences scores 0.571, so no threshold
on *how much* survived can separate them. Raising the floor to catch the truncation
rejects correct output; leaving it low accepts losing a third of a paragraph in silence,
which on a long dictation is a lost thought rather than a lost word.

What separates them is *where* the loss falls. Removing fillers thins a sentence evenly and
still ends where the speaker ended; a truncation stops early. So `tail_survives` asks the
only question that distinguishes the two: do any of the transcript's last three content
words turn up in the candidate's last six? Fillers are skipped when picking those words, so
a sentence trailing off in "and uh you know" is judged on its last real word. The one
legitimate way for an ending to vanish is inverse text normalisation rewriting it, and "by
three thirty p m" becoming "by 3:30pm" leaves none of those words behind, so a digit in
the candidate's tail counts as the ending being accounted for.

Over the combined corpus this separates 14 of 14, including three truncations that keep
*more* of the text than the accepted filler-heavy cases do. The growth floor stays at 0.35
as the cheap first filter and as a backstop against a summary.

### The two rules are not ordered, deliberately

Each catches something the other misses, and it is worth being clear that the looser setting
is not simply the tighter one plus more.

- Light touch **accepts** the damaged Polish sentence, because 0.089 divergence is a small
  edit however wrong it is. Containment rejects it: two of the words are new.
- Containment **rejects** a first-time proper-noun correction that light touch accepts,
  unless the user's dictionary already carries the word. Which is the honest behaviour:
  0005 pinned that the magnitude rule cannot tell `"Kubernetes"` from `"Cuber Nuts"`, and
  containment declines to guess.

**What full cleanup cannot do**, pinned by its own test: it cannot see words that were
merely dropped. `"translate this into german the meeting is at noon"` comes back as
`"The meeting is at noon."` with the instruction eaten: nothing invented, 0.556 of the
words remaining, so it is accepted. Light touch catches that one on length. Neither rule
bounds correctness, only shape.

## Decision

**Replace Qwen2.5 0.5B Instruct with S1-mini `q4`, six threads, a cached prompt prefix and
a containment guard, keeping 0005's `ort` runtime, its `TextRefiner` boundary, and its rule
that every failure falls through to the raw transcription.**

The stage stays **off by default**. It is faster and better, and it is still a language
model rewriting what someone said.

### Why replace rather than add

One refiner, one download to maintain, one set of thresholds to keep honest. S1-mini is
English-only in v1, so this narrows the feature, but 0005 measured that Qwen2.5 gave Polish
nothing usable either (it translated, and the guard rejected it), so what is lost is a
capability that never worked. `refine_text` now skips the stage outright when the user has
pinned a non-English language, rather than spending half a second finding out.

### Licence

S1-mini is Apache 2.0 with a naming clause: it must be identified as **"S1-mini" by
"Superwhisper"**, with that exact capitalisation, wherever it is used. The descriptor's
`name` and the Settings copy that shows it are therefore not free to be prettified.

## Consequences and risks

- **The vocabulary hint is gone from the prompt.** 0005 fed the user's dictionary to the
  model; S1-mini's control line has no slot for one, and appending it would push the prompt
  off the distribution it was trained on. The terms moved to the guard, where they mark a
  word as the speaker's own rather than invented. The measured cost is exactly the failing
  case: `"whisper free"` stays two words, where Qwen2.5 joined it. The dictionary still runs
  afterwards and fixes it, which is what the dictionary was always for.
- **Growth's floor is thinly sampled**, and that is now a known quantity rather than a
  worry: the corpus was widened to long dictation, the floor turned out not to be the
  measure that matters, and `tail_survives` was added because of it. The floor remains a
  cheap filter and a backstop, not the defence.
- **The tail check has a blind spot in the middle.** It bounds losses at the *end*, which
  is where a truncation shows up. A clause dropped from the middle of a long paragraph
  still passes every measure here, exactly as decision 0005's guard could not tell a right
  substitution from a wrong one. Neither rule bounds correctness.
- **The thread cap is one machine's measurement.** Six is right for an M1 Pro. It is a named
  constant so a measurement on other hardware has one place to change.
- **First model with external weights.** `model_q4.onnx_data` is found by the location
  string recorded inside the graph, resolved next to the `.onnx`, so the local filename is
  not ours to choose and `is_available` has to check for both.
- **The old download is deleted on first launch.** With the descriptor gone, Settings ›
  Models could neither show nor remove 490 MB of Qwen2.5. `ModelStore::remove_retired`
  sweeps model directories no descriptor claims; model files are a cache of something
  re-downloadable, never user data.
- **An existing `settings.json` names the old model.** `Settings` is `#[serde(default)]`, so
  without a migration `models::find` would return `None`, `sync_refiner` would skip, and
  cleanup would stop working with the checkbox still ticked. `Settings::migrate` rewrites it.

## Model artefacts

Repo `onnx-community/s1-mini-ONNX`. Digests computed from the files these measurements were
taken against, and cross-checked against the sizes HuggingFace reports.

| File | Local name | Size | SHA-256 |
|---|---|---|---|
| `onnx/model_q4.onnx` | `model_q4.onnx` | 0.37 MB | `be5f0d8d03ac387bdd2d2582e4e114ca3c23a44b70bf03be609844542107745c` |
| `onnx/model_q4.onnx_data` | `model_q4.onnx_data` | 403.01 MB | `85bcddf9b558e4881215c32652bc9345672530d77a432d2aee7f2e0c1ee62869` |
| `tokenizer.json` | `tokenizer.json` | 9.12 MB | `40ae5d1ee027b985684a3bbeef4ee16b2b5697d1d90658bec5bc5d2a73018bd7` |
