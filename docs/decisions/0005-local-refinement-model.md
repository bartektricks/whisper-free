# 0005 — The local refinement model

**Status:** accepted, and superseded in part by
[0012](0012-a-normalising-cleanup-model.md)
**Date:** 2026-08-20
**Applies to:** the refinement stage; extends decision 0001, which chose the speech runtime

> **What 0012 changed.** The runtime choice below (`ort`, pinned, CPU, greedy, behind our own
> `TextRefiner` trait), the requirement that the model's output is a *proposal*, and the rule
> that every failure falls through to the raw transcription all still stand; 0012 is built on
> them. Two things do not: the **model** is now S1-mini rather than Qwen2.5 0.5B Instruct, and
> the **guard rule** below is now only one of two, kept as the "light touch" setting. The
> thresholds, the corpus and the reasoning in *The guard* are unchanged and still measured;
> they simply no longer describe the default. The prompt findings are history: 0012's model is
> fine-tuned on the task and needs neither the worked example nor the vocabulary block.

Parakeet returns clean audio verbatim, but it mishears proper nouns, jargon and
homophones — "cuber netties" for "Kubernetes". The dictionary only helps once the user
has written a rule for the exact misheard form, so it never helps the first time a word
comes out wrong. This decision covers running a small language model over the
transcription before it is pasted: which runtime, which model, and — the part that took
the most work — what stops it making things worse.

## Measurement setup

| | |
|---|---|
| Machine | Apple M1 Pro, 10 cores, 32 GB, macOS 15.7.3 (arm64) |
| Runtime | `ort` 2.0.0-rc.12, CPU execution provider, greedy decoding |
| Harness | `cargo run --release --example refine_check` |
| Cases | 11 transcriptions: 4 needing a fix, 3 already correct, 1 Polish needing punctuation, 3 baiting the model into answering or translating |
| Prompt | system instruction, one worked example, five vocabulary terms |

The "already correct" and "baiting" cases are not padding. Leaving correct text alone is
most of the job, and a model that scores well only on the cases that need fixing is
useless.

## The requirement that shaped everything

Published work on post-ASR correction reports that language models **hallucinate on
already-correct ASR output and over-correct when the error rate is low**. Parakeet's
normal case *is* the low-error regime — decision 0001 measured 0.0 % WER on clean clips.

So the model's output is treated as a *proposal*, and `refine/guard.rs` exists to throw
it away. Every failure — absent model, load error, rejected rewrite, cancelled run,
over-long input — resolves to pasting the raw transcription. Losing a correction
disappoints; losing the user's words is a bug.

## Options evaluated

Cross-platform parity was the deciding constraint: the app ships on macOS and Windows,
and decision 0002 keeps a third platform one directory away.

| | macOS | Windows | Linux | New build dependency | Bundle growth | Code we write |
|---|---|---|---|---|---|---|
| **A. `ort`, pinned** | CPU | CPU | CPU | none | 0 MB | decode loop, ~250 lines |
| B. `llama-cpp-2` | Metal | CPU/Vulkan | CPU/CUDA | cmake + C++ on every platform | ggml artefacts | ~150 lines |
| C. `candle` | Metal | **CPU only** | CPU | none | 0 MB | ~200 lines |

Option A adds no native dependency at all: ONNX Runtime is *already statically linked
into the binary* for the speech model, and `transcribe-rs` pins `ort` to exactly
`2.0.0-rc.12`, so pinning the same version links one runtime rather than two.
`cargo tree -d` is the check, and it is worth keeping in the verification list.

## Measured results

Three exports, same harness, same prompt, same guard:

| Model | On disk | Cases passed | Load | Mean latency | Prefill | Decode | Decode rate |
|---|---|---|---|---|---|---|---|
| Qwen3 0.6B | 570 MB | 8/11 | 945 ms | 1 395 ms | 797 ms / 200 tok | 598 ms | 15.7 tok/s |
| **Qwen2.5 0.5B Instruct** | **483 MB** | **9/11** | **906 ms** | **1 063 ms** | **546 ms / 192 tok** | **517 ms** | **18.5 tok/s** |
| Qwen2.5 1.5B Instruct | 1 222 MB | 10/11 | 1 902 ms | 3 167 ms | 1 746 ms | 1 421 ms | 7.7 tok/s |

**Qwen2.5 0.5B is both smaller and better than Qwen3 0.6B.** That is not the intuitive
result. Qwen3 spends capacity on a reasoning mode this task actively disables, and the
0.5B model beats it on every axis at 85 MB less. The 1.5B model is the most accurate by
one case and three times the latency, which for a dictation app is the wrong trade: 3.2 s
between releasing the hotkey and seeing text is long enough to check whether the app has
hung.

**None of them can correct Polish.** Asked to proofread a Polish sentence, the 0.5B model
translates it into English and the 1.5B produces fluent nonsense — "I was sharpened by
your credit card". The guard rejects both, so Polish dictation degrades to no correction
rather than to damage, but this is a limitation of small instruction models rather than of
model size, and the 1.5B's extra 2 s buys nothing here. For a user who dictates Polish,
most of this feature's value is in the English half of their work.

Two prompt findings, both worth more than the model choice:

- **A worked example is the single biggest quality lever at this size.** Told only in
  prose, a 0.5B model answers the transcription or repeats the instruction back. Shown
  one completed exchange, it copies the shape. It costs about 60 prompt tokens, which is
  most of the difference between a 546 ms prefill and a shorter one — and buys more than
  the difference between two model sizes.
- **Where the vocabulary goes changes what it does.** Listed next to the transcription,
  a bare list of words reads as content: the first version of this returned
  `"Kubernetes, WhisperFree"` as the correction. Moved into the system turn, it stops
  competing with the text.

## The guard

Character-level edit distance over normalised text — lowercased, punctuation stripped,
apostrophes dropped, whitespace collapsed — against the raw transcription.

Word-level distance was tried first and is wrong for this job. It charges two edits for
"cuber netties" becoming "Kubernetes" and two more for "lets" becoming "Let's", which is
enough to push an *ideal* correction of a short sentence past any threshold loose enough
to be useful. Stripping the punctuation the model is *invited* to add, then measuring
characters, charges what the change actually costs.

Measured over the same corpus:

| | divergence |
|---|---|
| punctuation and capitalisation only | 0.000 |
| one run-together proper noun | 0.026 |
| one misheard word | 0.032–0.095 |
| a merge plus an apostrophe | 0.105 |
| **a small paraphrase — dropped words, changed person** | **0.190** |
| an answer to the transcription | 0.581 |
| a full rewrite | 0.761 |
| a translation | 0.762 |

The threshold is **0.18**, in the gap. The tightest rejection is the one that set it: a
Polish transcription came back with `zgubiłem` ("I lost") changed to `zgubiono` ("was
lost") and two words dropped — fluent, plausible, and not what the speaker said. At the
0.30 threshold the first draft used, it was accepted.

The gap is narrower than it looks. If a legitimate correction ever lands above the line,
the fix is to widen the sample and re-measure, not to nudge the number.

**What the guard cannot do**, pinned by its own test: it bounds how much may change,
never whether the change is right. "cuber netties" becoming "Cuber Nuts" scores 0.105 —
exactly what it scores becoming "Kubernetes". Keeping the model from making that
substitution is the prompt's job and the model's. This is why the feature is off by
default and why the dictionary still runs afterwards.

Length ratio (0.5–1.6) runs first as a cheap filter, and known lead-ins, code fences,
reasoning blocks and wrapping quotes are stripped rather than rejected — the answer
underneath is usually good, and discarding it for its packaging loses a correction for
no reason.

## Decision

**Adopt Option A: `ort` 2.0.0-rc.12 pinned to the version already in the tree, running
Qwen2.5 0.5B Instruct `q4f16`, greedy, behind our own `TextRefiner` trait — with the
stage off by default and every failure falling through to the raw transcription.**

### Why

- **No new native dependency, on any platform.** The runtime is already linked and
  already shipping. Nothing is added to the bundle, the installers, or CI.
- **Identical on macOS and Windows.** CPU on both, so there is no platform where the
  feature is good and another where it is an apology. Option C would have given Metal on
  macOS and CPU-only on Windows.
- **Greedy decoding is deterministic.** The same transcription refines to the same text
  every time, which is what makes the guard's thresholds testable at all.
- **Interruptible.** The cancel flag is checked between tokens, so Escape during
  refinement lands within one token — better than the speech stage, where
  `dictation.rs` records that inference cannot be interrupted part-way.

### Why hand-writing the decode loop is consistent with decision 0001

Decision 0001 rejected "raw `ort` with our own decoder" because that meant mel
extraction and a duration-head transducer decode — exotic, delicate, and already correct
in `transcribe-rs`. This loop is not that. A cached decoder-only forward pass is
textbook: prefill, take the last position's logits, argmax, feed the returned cache back.
Greedy means there is no sampler. The graph turned out to be a plain export with no
`com.microsoft.GroupQueryAttention` contrib ops, so there are no special inputs to
reconstruct.

Two things were still learned the hard way and are worth keeping written down:

- `ort` refuses to build a tensor from a raw slice with a zero-length dimension, which is
  exactly the shape an unseeded KV cache has. The array path accepts it.
- **The cache element type is not implied by the weight quantisation.** A `q4f16` export
  of Qwen3 carries a float16 cache; the same quantisation of Qwen2.5 carries float32.
  Feeding the wrong one is a hard refusal from the runtime, so `refine/onnx.rs` reads the
  type off the graph rather than assuming it.

### Why not the others

- **B (`llama-cpp-2`)** — the fastest option and the least code, but it puts a cmake/C++
  toolchain into every platform's build and artefacts into every installer. Decision 0001
  rejected sherpa-onnx on exactly this ground, and nothing here justifies reversing that
  for a feature that is off by default.
- **C (`candle`)** — pure Rust and genuinely portable, but Metal on macOS and CPU-only on
  Windows (a shippable CUDA build would need the toolkit present at build time). An
  asymmetry between the two platforms we ship is the opposite of the requirement.

### Consequences and risks

- **Roughly 1 s is added to every dictation when the feature is on**, against ~0.4 s for
  transcription today. It is off by default for this reason, and the setting says so.
- **A second model resident.** Parakeet's ~1.4 GB plus ~0.5 GB. The refiner is loaded
  when the setting is switched on and dropped when it is switched off, but decision
  0001's note about an idle-unload timeout is now considerably more pressing.
- **9/11, not 11/11.** The remaining two failures are the model being weak, not the
  guard: it did not use a vocabulary term it was given, and it left one typo. Both are
  harmless — the guard bounds the damage — but the feature does not fix everything, and
  the Settings copy should not imply that it does.
- **Polish gets no benefit**, per the measurement above. The README says so plainly rather
  than letting a bilingual user discover it. If this is worth fixing later, the lever is a
  model with real Polish instruction-following, not a bigger one of these.
- **The vocabulary hint is unreliable at this size.** It measurably helps, and it is
  measurably ignored some of the time. The dictionary still runs *after* the model for
  this reason: a rule the user wrote by hand is never second-guessed.
- The prompt is tuned to a 0.5B model. A second refinement model is a new `EngineKind`
  arm and possibly a new `Template` variant — and its own row in the table above, because
  none of these numbers transfer.

## Model artefacts

Repo `onnx-community/Qwen2.5-0.5B-Instruct`. Digests computed from the files these
measurements were taken against, and cross-checked against the sizes HuggingFace reports.

| File | Local name | Size | SHA-256 |
|---|---|---|---|
| `onnx/model_q4f16.onnx` | `model_q4f16.onnx` | 483.00 MB | `b11c1dd99efd57e6c6e5bc4443a019931a5fbd5dd500d48644d8225f5ce0b2cb` |
| `tokenizer.json` | `tokenizer.json` | 7.03 MB | `a8506e7111b80c6d8635951a02eab0f4e1a8e4e5772da83846579e97b16f61bf` |

The remote path and the local name differ, which is why `ModelFile` gained a `remote`
field: HuggingFace keeps ONNX exports in a subdirectory while the tokeniser sits at the
root, and both are flattened into one model directory locally.
