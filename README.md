# LocalDictation

Fully local dictation for macOS. Press a hotkey, speak, and the text appears
where your cursor is. Polish and English, detected automatically.

Nothing you say leaves your Mac.

## Privacy guarantees

These are architectural commitments, not settings you have to find and switch on:

- Microphone audio is held in memory and **never written to disk**.
- Transcription runs **on your machine**, through a local ONNX model. There is no
  cloud API, no server, and no account.
- Your dictionary and settings stay in your user Library folder.
- **No telemetry, no analytics, no crash reporting.**
- Logs record durations, sample counts and event names — never audio,
  transcription text, or clipboard contents.
- The only time the app touches the network is when **you** click Download on a
  model. After that it works fully offline; you can verify by pulling the
  Wi-Fi and dictating.

## Requirements

- Apple Silicon Mac, macOS 13 or later
- [Rust](https://rustup.rs), [Bun](https://bun.sh), and Xcode command line tools
- ~700 MB of disk for the speech model

## Running it

```sh
bun install
bun run tauri dev
```

The app has no Dock icon by design — it lives in the menu bar. On first launch
it opens its settings window to introduce itself.

### Useful commands

```sh
bun run check                  # typecheck the Svelte/TS frontend
cd src-tauri && cargo test     # Rust unit tests
cd src-tauri && cargo run --example mic_check 3   # record 3s and report levels
```

`mic_check` is the quickest way to tell whether microphone permission and device
selection are working — it prints duration, sample rate and peak level, and says
plainly when it heard only silence.

## The speech model

LocalDictation runs [NVIDIA Parakeet TDT 0.6B v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3),
which handles 25 European languages, detects the spoken language on its own, and
produces punctuation and capitalisation.

The model is **not bundled** — it is ~671 MB, and downloading that behind your
back on first launch would be rude. You trigger the download from
Settings › Models, and every file is checked against a pinned SHA-256 before it
is used.

Models are installed to:

```
~/Library/Application Support/com.bartek.localdictation/models/
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

## Permissions macOS will ask for

- **Microphone** — to record what you say. Requested the first time you record.
- **Accessibility** — to paste into the app you are typing in. Requested when
  text insertion is first used.

Both are grantable in System Settings › Privacy & Security.

## Layout

```
src/                     Svelte + TypeScript UI (settings only; small on purpose)
  components/Settings/   one component per settings section
  stores/                state mirrored from the backend
src-tauri/src/
  asr/                   SpeechRecognizer trait — the model boundary
  audio/                 microphone capture, downmix, resampling
  state/                 the authoritative application state machine
  settings/              persisted user settings
  platform/macos/        everything macOS-specific lives here
docs/decisions/          architecture decision records
```

Two boundaries are load-bearing and worth preserving:

- **`asr/`** — the rest of the app only knows `audio -> transcription`. Swapping
  in Whisper or another model should touch one file.
- **`platform/`** — no macOS API is called from application logic, so Windows and
  Linux remain reachable later.

## Status

Under construction. Working today: menu-bar app and settings window, the state
machine, settings persistence, and microphone capture with device selection and
a level test. Still to come: global hotkey, model download, transcription, text
insertion, and the dictionary.

## Licence

MIT. The Parakeet model weights are CC-BY-4.0 and are downloaded separately.
