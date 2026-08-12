# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

LocalDictation: a macOS menu-bar app (Tauri 2 + Rust backend, Svelte 5 + TS frontend) that
records on a global hotkey, transcribes locally with Parakeet TDT 0.6B v3 through ONNX
Runtime, and pastes the text into the focused app. Directory name is `whisper-free`; the
product, crate (`local-dictation`), and bundle id (`com.bartek.localdictation`) are not.

## Commands

```sh
bun install
bun run tauri dev             # the real app; `bun run dev` is Vite alone, where every invoke() fails
bun run tauri build           # .app + .dmg
bun run check                 # svelte-check — the only frontend check; there is no linter or JS test runner

cd src-tauri
cargo test                    # all unit tests (inline #[cfg(test)] modules)
cargo test hold_to_talk       # one test or module by name substring
cargo run --example mic_check 3                      # 3 s capture, prints duration/rate/peak
cargo run --release --example pipeline_check a.wav   # installs model if missing, then model → ASR → dictionary
```

There is no root `Cargo.toml`; cargo commands run from `src-tauri/`. The examples are
deliberately outside `cargo test` — they need a microphone, ~671 MB on disk, and network
on first run. `LOCAL_DICTATION_LOG=local_dictation_lib=debug` overrides the log filter;
logs land in `~/Library/Logs/com.bartek.localdictation/`, user data in
`~/Library/Application Support/com.bartek.localdictation/` (`settings.json`,
`dictionary.json`, `models/`).

## Architecture

Rust owns everything that matters. The Svelte layer is a settings window that renders
backend state and sends commands — it holds no application logic and no source of truth.

**`AppContext` (`lib.rs`)** is the single Tauri-managed struct holding every piece of
shared state behind `Mutex`, plus the audio thread handle and the two trait objects
(`hotkeys`, `inserter`). Adding a `#[tauri::command]` also means registering it in the
`invoke_handler!` list in `lib.rs`.

**State machine (`state/mod.rs`)** is authoritative and validates transitions
(`Ready → Recording → Transcribing → Inserting → Ready`; anything → `Error`; self →
self is a no-op so repeated events are harmless). Never mutate it directly: go through
`set_app_state` / `fail_app_state` in `lib.rs`, which also update the tray status line and
emit `state_changed` to the UI. Invalid transitions are logged and dropped, never
propagated. `Error → Recording` is legal on purpose — pressing the hotkey again is how a
user retries, and requiring a `Ready` hop raced with failures reported from worker threads.

**Pipeline (`dictation.rs`)** knows the order of steps and nothing about how any step
works. It runs `Stop` handling on a spawned `dictation` thread because inference plus the
paste-settle sleep would otherwise block the thread delivering hotkey events. Preconditions
are checked against the real thing (`recognizer.is_some()`, `ctx.audio.is_recording()`)
rather than against `AppState`, since state can be stale or `Error` after a failure.

**Audio (`audio/capture.rs`)** — `cpal::Stream` is `!Send` on macOS, so one thread owns
the stream for its whole life and everything else sends `Command`s over a channel and
waits for a reply. The audio contract is fixed at 16 kHz mono f32 in `[-1, 1]`; downmix
and resampling happen on stop.

**Two load-bearing boundaries** (keep them intact):

- `asr/` — outside this module the app only knows `audio -> transcription` via the
  `SpeechRecognizer` trait. `asr/parakeet.rs` is the only file that may name
  `transcribe-rs`, ONNX, quantisation, or chunk sizes. A second engine means a new
  `EngineKind` variant and a new arm in `build_recognizer` (`lib.rs`), nothing else.
- `platform/` — application code calls the free functions in `platform/mod.rs` and never
  imports `platform::macos::*`. Non-macOS builds currently `compile_error!`, but the seam
  is what makes Windows/Linux reachable later.

**Models (`models/`)** are static `ModelDescriptor`s with per-file pinned SHA-256; nothing
is bundled and nothing downloads without the user asking. `download_model` returns
immediately and the worker thread emits `model_download_progress` /
`model_download_completed` / `model_download_failed`, then reloads the recogniser.

**Text insertion (`text_insertion/`, `platform/macos/text.rs`)** is clipboard + synthetic
Cmd+V via CGEvent — no per-app integrations, by design. The previous clipboard is restored
after a 150 ms settle, and `ClipboardOutcome` reports what happened to it so a lost image
is surfaced rather than silent.

**Frontend** — `src/types/index.ts` mirrors the Rust serde shapes by hand (snake_case,
`LanguageSelection` is tag/content). Change a Rust type that crosses IPC and change the TS
type in the same edit. `stores/appState.ts` is a `readable` fed by `get_recording_state` +
the `state_changed` event; `stores/settings.ts` writes optimistically and then replaces
state with whatever `update_settings` returns, since the backend may reject or normalise.

## Invariants worth not breaking

- **Privacy is architectural.** Audio stays in memory and never touches disk. Never log or
  persist audio samples, transcription text, clipboard contents, or dictionary entries —
  log shapes only (durations, sample counts, char counts, event names). No telemetry, and
  no network call other than an explicitly requested model download.
- **Errors that reach the UI are written for a person.** Each error enum has a
  `user_message()` returning plain language that names where to fix the problem; raw detail
  goes to `tracing`. Existing tests assert those messages leak no internals (`ort::`,
  `CoreAudio`, `NSPasteboard`, …) — a new variant needs an arm and usually a test.
- **An empty transcription is a surfaced outcome, never a silent no-op**, and audio over
  `CHUNK_THRESHOLD_SECS` (18 s) must be chunk-decoded. Both come from a measured failure
  mode; see `docs/decisions/0001-parakeet-inference-runtime.md` before touching the
  constants in `asr/parakeet.rs`. That ADR also documents why the execution provider is
  CPU and not CoreML (2.9× slower, 4.5× the memory on this int8 graph).
- Parakeet v3 detects language but cannot be pinned, hence two separate `Capability`
  values; `LanguageSelection::Fixed` is refused up front rather than silently ignored.
- Closing the settings window hides it — a menu-bar app must not quit on window close
  (`on_window_event` in `lib.rs`).
- Settings/dictionary loading is fault-tolerant on purpose: a corrupt or partial file falls
  back to defaults with a warning rather than blocking startup. Saves are write-tmp-rename.

## Conventions

- Tests live inline in each module and are named as sentences describing the behaviour
  (`hold_to_talk_ignores_key_repeat`). Pure decision logic is deliberately separated from
  OS calls (`hotkey::decide`, `state::is_valid`) so it can be tested without a window server.
- Comments cite `plan §N` sections of a specification that is **not in this repo** — treat
  those references as intent markers, not as files to open. The in-repo record is
  `docs/decisions/`.
- Prose in comments, UI strings, and docs uses British spelling ("recognise", "behaviour").
