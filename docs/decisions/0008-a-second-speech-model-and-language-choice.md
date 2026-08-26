# 0008 — A second speech model, and choosing a language

**Status:** accepted
**Date:** 2026-08-25
**Applies to:** `asr/`, `models/`, Settings › Speech; extends decision 0001, which chose the speech runtime

Until now the app shipped one speech model and one way of picking a language, which was
not to pick one: Parakeet v3 detects the spoken language and cannot be told what it is.
`Capability::LanguageSelection` and `LanguageSelection::Fixed` existed and had tests, but
nothing in the registry could reach them. This decision covers adding models the user can
choose between, and making the language setting mean something.

## What was already available

`transcribe-rs` compiles six ONNX engines under the `onnx` feature the app already
enables: canary, cohere, gigaam, moonshine, parakeet, sense_voice. They all implement
`SpeechModel`, and `EnergyAdaptiveChunked::feed` takes `&mut dyn SpeechModel`, so the
existing recogniser body was already engine-agnostic in everything but its `load` call.

Distribution decided most of the shortlist. Moonshine and SenseVoice are published only
as `.tar.gz`, and `models/download.rs` fetches individual files against pinned per-file
digests; supporting archives would mean a digest over something the app then unpacks,
which is a larger change than this decision wanted to make. GigaAM is Russian only and
Cohere's exports use external-data files. istupakov publishes Canary as individual files
on HuggingFace, laid out exactly like the Parakeet export already trusted.

## Why Canary is the complement of Parakeet, not a replacement

Canary takes a source language token and never guesses: `transcribe_raw` falls back to
`"en"` when it is given nothing. Parakeet detects and refuses to be pinned. The two
capabilities in `asr::types::Capability` were written for exactly this split, and this is
the first release where both have a model behind them.

That has a consequence the UI cannot hide. One `settings.language` has to serve models
that disagree about what choosing a language even means, and
`asr::check_language_request` turns a mismatch into a failed dictation — correct at the
point of use, and far too late in Settings. So `models::normalise_language` maps a
selection onto the nearest thing the chosen model can honour, and `update_settings` runs
it on every save. It is pure, it is a fixed point, and its output is tested to be
something `check_language_request` would never refuse.

Per-model memory was considered and rejected: it makes `settings.json` harder to read by
hand, which is the one debugging tool that file has, and the case it improves — switching
back and forth between two models with different languages — is rarer than the confusion
of a settings file whose language key is a map.

## Digests without downloading

HuggingFace's tree API reports an LFS `oid` per file, which is the SHA-256 of the
contents. Checked against the four digests decision 0001 pinned by hand: all four match.
So the Canary digests here were pinned from the API and then confirmed against real bytes
for the 180M Flash export, including `vocab.txt`, which is not stored in LFS and was
hashed directly.

Canary needs `nemo128.onnx`, NeMo's 128-band mel preprocessor, and istupakov's Canary
repositories do not ship one. It is byte-identical to Parakeet's, so `ModelFile` gained a
per-file `base_url` override rather than the file being vendored or duplicated. A test
asserts the two descriptors resolve that file to the same URL and the same digest — if
they ever diverge, the encoder is being fed mel features it was not exported for.

## Measured: Canary 180M Flash

Apple M1 Pro, macOS 15.7.3, int8 CPU. Clips generated with `say` and converted to
16 kHz mono, 4-6 s each. Parakeet was run on the same files as a control.

| Language | Canary 180M Flash | Parakeet v3 (control) |
|---|---|---|
| English | correct | correct |
| French | correct | correct |
| German | correct, but a space before every `.` and `?` | correct |
| Spanish | **empty string**, 3 clips of differing length | correct |

Load 329 ms, RTF 0.059 — about seventeen times faster than real time, against Parakeet's
much larger graph.

Two findings came out of this, and both changed the code:

**Spanish is not offered.** NVIDIA's model card lists four languages and `transcribe-rs`
agrees, but this export decodes Spanish to nothing on every clip tried, where the same
model handled the other three and Parakeet handled the same audio. Both vocabularies
contain the full ISO-639-1 token set, so `<|es|>` resolves and the failure is in the
weights rather than the plumbing. An empty transcription is a surfaced failure by design
(decision 0001), so declaring Spanish would be declaring a language that reliably fails.
`CANARY_FLASH_LANGUAGES` lists what was measured to work, not what the card claims.

**Punctuation is closed up.** `asr::onnx::tidy_punctuation` removes the space Canary
leaves before `. , ! ? ; :`. It is safe to apply to every engine because the artefact is
the model emitting punctuation as its own token and never joining it back on: Canary's
English and French output has no such spacing at all, so there is nothing legitimate to
destroy — not even the narrow space French typography puts before `?` and `!`, because
the model does not produce it. Parakeet never produces the artefact either. Quotes and
brackets are deliberately left alone, since fixing those needs to know which side of a
pair each one is and guessing wrong moves the space instead of removing it.

## Decision

Ship Canary 1B v2 (1.03 GB, 25 languages) and Canary 180M Flash (213 MB, English, German,
French) alongside Parakeet, behind one `EngineKind::Canary`. Generalise
`asr::parakeet::ParakeetRecognizer` into `asr::onnx::OnnxRecognizer`, which holds a
`Box<dyn SpeechModel>` and dispatches on an `OnnxEngine` — so `asr/onnx.rs` is now the
one file that may name `transcribe-rs`, and a further model in either family is a
`ModelDescriptor` and nothing else.

### Consequences and risks

- **Canary 1B v2 is pinned but unverified.** Its digests come from the HuggingFace API by
  the method validated above, and its file layout matches the Flash export that was run.
  Its transcription quality, and whether it shares the Flash export's per-language
  failures, has not been measured. Given that its sibling failed one of four declared
  languages outright, its 25 declared languages should be treated as a claim rather than
  a finding until someone runs `pipeline_check --model canary-1b-v2` against them.
- **Canary translates when the language is wrong.** German audio pinned to English came
  back as fluent English prose, not as garbage — `transcribe_raw` sets the target language
  from the source, and a mismatch reads as a translation request. Picking the wrong
  language therefore fails silently and plausibly, which is worse than failing loudly.
  This is why `normalise_language` never leaves a selection the model cannot honour.
- **Chunking for Canary is inherited, not measured.** `Chunking::CANARY` is
  `Chunking::PARAKEET`. Canary is an attention encoder-decoder and has no reason to share
  the failure decision 0001 measured, so this is a safe default rather than a finding:
  shorter chunks cost a little context at the seams and never break a model that could
  have taken longer ones. Measure before raising it.
- **Disk grows quickly.** Three speech models is 1.9 GB if a user installs all of them,
  and nothing prunes. Settings › Models now marks which model is in use, because Remove
  sits next to it.
