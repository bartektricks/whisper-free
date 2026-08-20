# WhisperFree

Fully local dictation for macOS and Windows. Press a hotkey, speak, and the text
appears where your cursor is. Polish and English, detected automatically.

Nothing you say leaves your machine.

## Privacy guarantees

These are architectural commitments, not settings you have to find and switch on:

- Microphone audio is held in memory and **never written to disk**.
- Transcription runs **on your machine**, through a local ONNX model. There is no
  cloud API, no server, and no account.
- Your dictionary and settings stay in your own user profile.
- **No telemetry, no analytics, no crash reporting.**
- Logs record durations, sample counts and event names — never audio,
  transcription text, or clipboard contents.
- The only time the app touches the network is when **you** click Download on a
  model. After that it works fully offline; you can verify by pulling the
  Wi-Fi and dictating.

## Download

Built installers live on the [Releases page](https://github.com/bartektricks/whisper-free/releases):
`WhisperFree_<version>_aarch64.dmg` for Apple Silicon Macs, and
`WhisperFree_<version>_x64-setup.exe` for 64-bit Windows.

Both are **unsigned**, so each platform warns you once. On macOS, drag the app to
Applications and then clear the quarantine flag, or it is reported as damaged:

```sh
xattr -cr /Applications/WhisperFree.app
```

On Windows, SmartScreen blocks the installer until you choose **More info** →
**Run anyway**. It installs for the current user, so no admin rights are needed.

The installers are small because the ~671 MB speech model is not bundled — see
[The speech model](#the-speech-model).

## Requirements

- Apple Silicon Mac on macOS 13 or later, or 64-bit Windows 10/11
- [Rust](https://rustup.rs) and [Bun](https://bun.sh); Xcode command line tools on
  macOS, the MSVC build tools on Windows — to build from source
- ~700 MB of disk for the speech model

## Running it

```sh
bun install
bun run tauri dev
```

The app has no Dock icon by design — it lives in the macOS menu bar, or the
Windows notification area. On first launch it opens its settings window to
introduce itself.

The default hotkey is ⌥Space on macOS and Ctrl+Alt+Space on Windows, where
Alt+Space belongs to the system window menu.

The hotkey can also be a two-step chord — press ⌘K, then K — written the way
VS Code writes them. The first combination is held system-wide, as any global
shortcut is; if you do not follow it with the second key within 800 ms it is
passed on to whichever app you were using, so ⌘K keeps working everywhere else.

### Useful commands

```sh
bun run check                  # typecheck the Svelte/TS frontend
cd src-tauri && cargo test     # Rust unit tests
cd src-tauri && cargo run --example mic_check 3   # record 3s and report levels
```

`mic_check` is the quickest way to tell whether microphone permission and device
selection are working — it prints duration, sample rate and peak level, and says
plainly when it heard only silence.

Releases are cut by hand from the Actions tab — bump the version, then run the
Release workflow and pick release, prerelease or draft. See
[`docs/RELEASING.md`](docs/RELEASING.md).

## The speech model

WhisperFree runs [NVIDIA Parakeet TDT 0.6B v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3),
which handles 25 European languages, detects the spoken language on its own, and
produces punctuation and capitalisation.

The model is **not bundled** — it is ~671 MB, and downloading that behind your
back on first launch would be rude. You trigger the download from
Settings › Models, and every file is checked against a pinned SHA-256 before it
is used.

Models are installed to:

```
macOS    ~/Library/Application Support/com.bartek.whisperfree/models/
Windows  %APPDATA%\com.bartek.whisperfree\models\
```

Why this model and this runtime — including the measurements behind the choice,
and the failure mode that shaped the ASR design — is written up in
[`docs/decisions/0001-parakeet-inference-runtime.md`](docs/decisions/0001-parakeet-inference-runtime.md).
The short version, measured on an M1 Pro:

| | |
|---|---|
| Speed | 23× faster than real time (a 10 s utterance transcribes in ~0.4 s) |
| Memory | ~1.4 GB while loaded |
| Execution provider | **CPU** — CoreML measured 2.9× *slower* and used 4.5× the memory |

## Cleaning up transcriptions

Parakeet returns clean audio verbatim, but it mishears names and jargon —
"cuber netties" for "Kubernetes". Switching on **Settings › Cleanup** runs each
transcription past [Qwen2.5 0.5B Instruct](https://huggingface.co/onnx-community/Qwen2.5-0.5B-Instruct)
(483 MB, downloaded on request like the speech model) before it is pasted.

It is **off by default**, because it costs about a second per dictation and half a
gigabyte of memory on top of the speech model. With it off, nothing about the
pipeline changes.

**A cleanup is only ever a suggestion.** The model's output is checked against what
you actually said, and thrown away if it strays — measured on the corpus in
[decision 0005](docs/decisions/0005-local-refinement-model.md), real corrections
move 0–11 % of the text while paraphrases, answers and translations move 19 % or
more. Anything past 18 % is refused and the raw transcription is pasted. Your
dictionary is applied afterwards either way, so a replacement you wrote by hand is
never second-guessed.

Measured on an M1 Pro, over 11 transcriptions:

| | |
|---|---|
| Added latency | ~1.1 s (0.57 s reading the prompt, 0.53 s writing the correction) |
| Memory | ~0.5 GB while loaded, on top of Parakeet's 1.4 GB |
| Cases handled correctly | 9 of 11 |

**It is much weaker in Polish than in English.** Asked to correct Polish, this model
tends to translate it instead — the guard catches that and falls back, so Polish
dictation is no worse than with the feature off, but it is not much better either.
The two remaining English failures are the model ignoring a dictionary term it was
given, and leaving one typo alone. It does not fix everything.

## Permissions

**macOS** asks for two:

- **Microphone** — to record what you say. Requested the first time you record.
- **Accessibility** — to paste into the app you are typing in. Requested when
  text insertion is first used.

Both are grantable in System Settings › Privacy & Security.

**Windows** asks for neither. Microphone access is governed by the toggle in
Settings › Privacy & security › Microphone, and pasting needs no permission at
all — with one exception: Windows will not let WhisperFree paste into a window
belonging to a program running as administrator. When that happens the
transcription is left on the clipboard and the app says so, so nothing is lost.

## Layout

```
src/                     Svelte + TypeScript UI (settings only; small on purpose)
  components/Settings/   one component per settings section
  stores/                state mirrored from the backend
src-tauri/src/
  asr/                   SpeechRecognizer trait — the speech model boundary
  refine/                TextRefiner trait — the cleanup model boundary
  audio/                 microphone capture, downmix, resampling
  state/                 the authoritative application state machine
  overlay.rs             the floating indicator: whether it shows, and where
  settings/              persisted user settings
  platform/              the OS seam: mod.rs is the whole API
  platform/macos/        \_ one directory per platform, selected at compile time
  platform/windows/      /
docs/decisions/          architecture decision records
```

Two boundaries are load-bearing and worth preserving:

- **`asr/`** — the rest of the app only knows `audio -> transcription`. Swapping
  in Whisper or another model should touch one file.
- **`refine/`** — the rest of the app only knows `text -> text`. `refine/onnx.rs`
  is the only file that may name an ONNX session, a tokeniser or a KV cache, and
  `refine/guard.rs` is pure, so the rule that decides whether a correction is safe
  to paste is testable without a model.
- **`platform/`** — no OS API is called from application logic. The backend
  module is private, so a platform directory cannot be reached around; adding
  Linux means adding one directory and one `cfg_attr` line.
  See [`docs/decisions/0002-cross-platform-platform-layer.md`](docs/decisions/0002-cross-platform-platform-layer.md).

The overlay is deliberately *not* one of them: it is an ordinary Tauri window
built `focusable: false`, which is what stops it stealing the focus the paste
depends on, on both platforms and without a line of OS-specific code. See
[`docs/decisions/0004-dictation-overlay.md`](docs/decisions/0004-dictation-overlay.md).

## Status

The full dictation loop is implemented: hotkey → record → transcribe →
[clean up] → dictionary → paste.

Working today:

- Menu-bar / notification-area app with a settings window, and an authoritative
  state machine
- A floating indicator while dictation runs — it never takes focus, clicks pass
  straight through it, its corner is yours to choose, and it can be switched off
  in Settings › General. It cannot yet draw inside another app's full-screen
  space; see the decision record for why
- Escape abandons a dictation in progress: the recording is dropped, and a
  transcription already running is discarded rather than pasted
- Configurable global hotkey, including two-step chords, hold-to-talk or toggle
- Microphone capture with device selection and a level test
- Model download with progress, SHA-256 verification, and removal
- Local transcription with automatic language detection and punctuation
- Word-boundary-aware dictionary replacements
- Optional local cleanup: a second, small language model checks each transcription
  over and fixes words the speech model misheard. Off by default, and every
  correction it proposes has to get past a guard before it is used
- Clipboard-based insertion into the focused app, with the clipboard restored
- Start at login

Not verified yet: the reliability sweep of §21 milestone 10 has been worked
through for rapid hotkey presses, empty and over-short recordings, missing
models and repeated failures, but **sleep/wake, unplugging the microphone
mid-recording, and very long recordings** still need a pass. Dictation into
real apps (VS Code, Terminal, browsers) needs a human at the keyboard — the
insertion code path is exercised, but only you can confirm the text lands
where you expect.

**The Windows backend has not yet been run on Windows.** It compiles and its
decision logic is unit-tested, but it cannot be cross-compiled from macOS (the
`ring` C build needs the MSVC headers), so CI and a real machine are the first
places it is exercised. The parts most worth watching are the synthetic paste
with modifiers still held, and the tray icon, which is currently a macOS
monochrome template image.

### Checking it yourself

```sh
cd src-tauri
cargo test                                          # 219 unit tests
cargo run --example mic_check 3                     # capture path
cargo run --release --example pipeline_check a.wav  # model + ASR + dictionary
cargo run --release --example refine_check          # cleanup model, measured
```

`pipeline_check` installs the model if it is missing, so it doubles as a way to
verify the download and checksum path from a terminal.

## Licence

MIT. The Parakeet model weights are CC-BY-4.0 and are downloaded separately.
