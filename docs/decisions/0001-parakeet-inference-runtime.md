# 0001 — Parakeet inference runtime on Apple Silicon

**Status:** accepted
**Date:** 2026-08-12
**Applies to:** plan §5, §6, §25 (milestone 4)

This is the technical decision required before the ASR layer is implemented. It is
based on measurements taken on the target machine, not on vendor claims.

## Measurement setup

| | |
|---|---|
| Machine | Apple M1 Pro, 32 GB, macOS 15.7.3 (arm64) |
| Model | `nvidia/parakeet-tdt-0.6b-v3`, ONNX export `istupakov/parakeet-tdt-0.6b-v3-onnx`, int8 |
| Harness | standalone Rust binary, `transcribe-rs` 0.3.11 + `ort` 2.0.0-rc.12 |
| Audio | 9 clips / 108.6 s — 7× Polish + 1× English spontaneous speech (MINDS-14 `pl-PL`), 1× English read speech (JFK) |
| Method | 3 runs per clip, median reported; separate process per execution provider |

Polish material is real human speech with ground-truth transcripts, so the numbers
below are measured rather than eyeballed. See "Accuracy caveat" for why the WER
figures are pessimistic.

## Options evaluated

| Option | Runtime | Model format | Apple Silicon accel | CPU fallback |
|---|---|---|---|---|
| **A. `transcribe-rs` + `ort`** | ONNX Runtime via `ort` (Rust) | ONNX int8 | CoreML EP (measured) | native, default |
| B. Raw `ort`, own TDT decoder | ONNX Runtime via `ort` (Rust) | ONNX int8 | CoreML EP | native |
| C. `sherpa-rs` / sherpa-onnx | C++ sherpa-onnx via FFI | ONNX int8 | CoreML EP | native |
| D. `parakeet-rs` (Candle) | Candle | SafeTensors | Metal | native |
| E. CoreML `.mlpackage` (FluidAudio) | CoreML | mlpackage | ANE / GPU | n/a |

## Measured results — Option A

| Execution provider | Model load | Warm-up | Aggregate RTF | Speed | Peak RSS |
|---|---|---|---|---|---|
| **CPU only** | **823 ms** | 1 270 ms | **0.043** | **23.3× real time** | **1 413 MB** |
| CoreML | — | 4 299 ms | 0.124 | 8.1× real time | 6 390 MB |

**CoreML is 2.9× slower and uses 4.5× the memory.** The int8-quantised graph is
largely unsupported by the CoreML EP, so it partitions the graph, falls back to CPU
for most nodes, and pays conversion and compile costs on top. This is the single most
important finding: the intuitive "use the Neural Engine" choice is the wrong one here.

Per-clip CPU latency, which is what the user actually feels:

| Audio length | Inference |
|---|---|
| 3.3 s | 155 ms |
| 5.4 s | 255 ms |
| 11.0 s | 435 ms |
| 29.3 s | 1 167 ms |

A typical dictation utterance of 5–10 s transcribes in **0.2–0.4 s**.

## Accuracy

Verbatim output, no language flag passed:

- **Polish** — `"Dzień dobry, zgubiłem swoją kartę kredytową, proszę o jej zablokowanie."` (0.0 % WER)
- **English** — `"And so, my fellow Americans, ask not what your country can do for you. Ask what you can do for your country."` (0.0 % WER)
- **Language detection** — a clip of English speech inside the Polish dataset was transcribed as English, and Polish clips as Polish, with no language hint. Automatic detection works.
- **Punctuation and capitalisation** — present and correct in both languages, including Polish diacritics.

Mean measured WER: Polish 25.9 % (n=7), English 16.7 % (n=2).

**Accuracy caveat:** those means substantially overstate the real error rate. The
MINDS-14 references are loose human annotations with no punctuation, and the audio is
telephone-band 8 kHz upsampled to 16 kHz — far from a clean 16 kHz desktop microphone.
Three of seven Polish clips scored exactly 0.0 %. On the worst clip the model's output
is visibly *more* coherent than the reference transcript it is scored against. The
figure to trust for this application is the qualitative one: clean clips come back
verbatim with correct punctuation. Real-world accuracy must be re-validated on the
user's own voice and microphone.

## Failure mode found (important)

One clip (29.3 s, Polish) returned an **empty string** from single-shot decoding,
despite containing clear speech. Isolated:

- first 15 s alone → transcribes correctly
- any window *including* the second half → **entire output collapses to empty**, losing the speech that decoded fine on its own
- reproduced identically on CPU and CoreML, so it is a model/decode property, not a runtime bug
- decoding the same audio in ~15 s chunks **recovers the full sentence**

Ruled out as the cause: trailing silence and trailing room noise. JFK audio padded with
2/5/10/20 s of digital silence and of pink noise all transcribed perfectly. The trigger
is specific pathological content in that segment, not silence in general.

Both requirements below were later confirmed to work: the same 29 s clip that
returned empty now transcribes in full through the shipped pipeline, because it
crosses the chunking threshold.

Two requirements follow, and both are architectural rather than cosmetic:

1. The ASR layer **chunks long audio** (~15 s target, split on low-energy frames) instead of decoding one long buffer.
2. An empty transcription is treated as a **distinct, surfaced outcome** — never a silent no-op. The user must never hold the key, speak, and get nothing with no explanation.

## Sensitivity on marginal audio

While validating the pipeline, one Polish clip decoded as `"Nie dobrze gubiłem"`
instead of `"Dzień dobry, zgubiłem"`. The only difference between the two runs was
the scale factor converting 16-bit samples to floats — `/32768` versus `/32767`, a
relative amplitude change of 3×10⁻⁵. Restoring the second value restored the
correct transcription exactly.

Two things follow:

- **Do not read much into a single clip.** On borderline audio the int8 model sits
  close enough to a decision boundary that noise-level differences flip words. Judge
  changes on a set of clips, not on one.
- **Live capture is unaffected.** The microphone path is f32 end to end (cpal →
  resampler → model) and never round-trips through 16-bit, so this particular
  sensitivity does not arise in the app. It was an artefact of a WAV-reading test
  helper. It is a reason to be careful about adding gain or normalisation steps
  later, though.

## Decision

**Adopt Option A: `transcribe-rs` on `ort` (ONNX Runtime), Parakeet v3 int8, CPU
execution provider by default, behind our own `SpeechRecognizer` trait.**

The execution provider is a runtime setting, not a compile-time one, so CoreML remains
switchable for re-measurement on future hardware and OS versions without code changes.

### Why

- **Fastest and leanest of everything measured** — 23× real time, 1.4 GB, on the CPU path.
- **No Python at runtime**, no C++ toolchain, no server. Pure Rust dependency tree; `cargo build` is the whole story. Satisfies §23.4.
- **Proven in the identical use case.** `transcribe-rs` was extracted from [Handy](https://github.com/cjpais/Handy), an open-source Tauri + Rust offline dictation app running this exact model. MIT-licensed, actively maintained (0.3.11, April 2026).
- **Multi-model for free.** The crate already implements Whisper, Canary, Moonshine and others behind one trait, which is the §22 "additional ASR models" requirement largely pre-solved.
- **Chunked decoding is already available** (`EnergyAdaptiveChunked`, `VadChunked`) — the mitigation for the failure mode above is a library feature, not something we write.

### Why not the others

- **B (raw `ort`)** — means hand-writing mel extraction, the TDT greedy decode loop with duration heads, and token detokenisation. That is the delicate part of the pipeline, it is already correct in A, and reimplementing it buys nothing.
- **C (sherpa-onnx)** — mature and capable, but pulls a C++ build/FFI dependency into a pure-Rust tree for no measured performance gain.
- **D (`parakeet-rs`)** — Candle + Metal is architecturally attractive, but the project is very early (9 commits), ships no published crate, is CLI-shaped rather than library-shaped, and its own reported RTF of 0.131 is **3× slower than what Option A does on CPU**.
- **E (CoreML `.mlpackage`)** — would require a Swift/ObjC bridge, and the CoreML measurement above gives no reason to expect a win.

### Consequences and risks

- `transcribe-rs` is a young crate (0.3.x, breaking changes between minors). **Mitigated by the `SpeechRecognizer` trait**: model-specific types stay inside `asr/parakeet.rs` and never leak into the app core, per §23.7. Swapping the crate out later touches one file.
- The crate reports Parakeet `capabilities().languages` as `["en"]`, which is stale metadata from the v2 era and contradicts observed behaviour. **Our own model registry declares supported languages**; we do not consult the crate's value.
- ~1.4 GB resident while loaded argues for an idle-unload timeout in a later milestone. Not a v1 blocker on 16 GB+ machines.
- Model is **not bundled** (§23.13). Fetched from HuggingFace at user request: 4 files, ~671 MB int8, CC-BY-4.0, each verified against a pinned SHA-256.

## Model artefacts

Repo `istupakov/parakeet-tdt-0.6b-v3-onnx`, layout consumed directly by `transcribe-rs`:

| File | Size | SHA-256 |
|---|---|---|
| `encoder-model.int8.onnx` | 652.18 MB | `6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09` |
| `decoder_joint-model.int8.onnx` | 18.20 MB | `eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70` |
| `nemo128.onnx` (mel preprocessor) | 0.14 MB | `a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f` |
| `vocab.txt` | 0.09 MB | *(computed at packaging time)* |

Audio contract: **16 kHz, mono, f32 in [-1, 1]**. This fixes the microphone capture
format in milestone 2.

Declared language set (25, from the v3 model card): `bg cs da de el en es et fi fr hr
hu it lt lv mt nl pl pt ro ru sk sl sv uk`.
