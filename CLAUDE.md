# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

LocalDictation: a macOS menu-bar / Windows notification-area app (Tauri 2 + Rust backend,
Svelte 5 + TS frontend) that records on a global hotkey, transcribes locally with Parakeet
TDT 0.6B v3 through ONNX Runtime, and pastes the text into the focused app. Directory name
is `whisper-free`; the product, crate (`local-dictation`), and bundle id
(`com.bartek.localdictation`) are not.

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
`dictionary.json`, `models/`) — `%APPDATA%\com.bartek.localdictation\` on Windows. Paths
always come from Tauri's resolver, never hardcoded.

The Windows backend **cannot be cross-compiled from macOS** (`ring`'s C build needs the
MSVC headers), so `.github/workflows/ci.yml` runs the matrix; a macOS-only `cargo check`
never compiles `platform/windows/`.

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

**Hotkeys (`hotkey/`)** — `settings.hotkey` is one accelerator, or two separated by a
space for a chord (`"Cmd+K K"`). Only the prefix stays registered; `dictation` claims
the second step when the prefix fires and replays the prefix into the focused app if
the chord is abandoned. `chord::Window` has three states for a reason: once the second
step is *down* (`Held`) the 800 ms timeout must not release it, or hold-to-talk loses
the key-up that ends the recording — a bug toggle mode cannot show you, and neither
can a test that releases faster than the timeout. `chord::classify` and `chord::Arming`
are pure, like `hotkey::decide`. See `docs/decisions/0003-two-step-hotkey-chords.md`.
The recorder in Settings brackets itself with `suspend_hotkey` / `resume_hotkey`, or
the live shortcut would fire instead of being recorded.

**Pipeline (`dictation.rs`)** knows the order of steps and nothing about how any step
works. It runs `Stop` handling on a spawned `dictation` thread because inference plus the
paste-settle sleep would otherwise block the thread delivering hotkey events. Preconditions
are checked against the real thing (`recognizer.is_some()`, `ctx.audio.is_recording()`)
rather than against `AppState`, since state can be stale or `Error` after a failure.

**Overlay (`overlay.rs`)** is the floating indicator, a second window labelled
`overlay` declared in `tauri.conf.json`. Rust decides *whether* it is on screen and
*where*; the webview (`src/overlay/`, its own Vite entry) decides what it looks like,
from the same `state_changed` broadcast. `publish_state` emits before it calls
`overlay::apply`, so the webview has the snapshot before the window appears, and
`apply` posts its window work with `run_on_main_thread` rather than blocking the
caller — which may be the hotkey handler. `place` is pure and tested per anchor.
`platform::float_above_full_screen_apps` raises the window level; note that the
overlay still cannot enter another app's full-screen Space, and that raising the
level further does not fix it. See `docs/decisions/0004-dictation-overlay.md`.

**Escape cancels a run.** It is registered only for the life of a dictation and
released straight after, because a permanently registered Escape would swallow the
key system-wide; `cancel_key_held` keeps the claim single-entry. Whether Escape stops
the recording or merely discards the pending transcription is decided by the
`finishing` gate, not by `AppState`.

**Audio (`audio/capture.rs`)** — `cpal::Stream` is `!Send` on macOS, so one thread owns
the stream for its whole life and everything else sends `Command`s over a channel and
waits for a reply. The audio contract is fixed at 16 kHz mono f32 in `[-1, 1]`; downmix
and resampling happen on stop.

**Two load-bearing boundaries** (keep them intact):

- `asr/` — outside this module the app only knows `audio -> transcription` via the
  `SpeechRecognizer` trait. `asr/parakeet.rs` is the only file that may name
  `transcribe-rs`, ONNX, quantisation, or chunk sizes. A second engine means a new
  `EngineKind` variant and a new arm in `build_recognizer` (`lib.rs`), nothing else.
- `platform/` — application code calls the free functions and `platform::strings` consts in
  `platform/mod.rs`, and nothing else. `mod backend` is private and selected by
  `#[cfg_attr(target_os, path)]`, so `platform::macos::*` is unnameable from outside and a
  backend missing any item of the contract fails to compile. Adding Linux is one directory
  plus one `cfg_attr` line. See `docs/decisions/0002-cross-platform-platform-layer.md`.

**Models (`models/`)** are static `ModelDescriptor`s with per-file pinned SHA-256; nothing
is bundled and nothing downloads without the user asking. `download_model` returns
immediately and the worker thread emits `model_download_progress` /
`model_download_completed` / `model_download_failed`, then reloads the recogniser.

**Text insertion (`text_insertion/`, `platform/*/text.rs`)** is clipboard + a synthetic
paste — no per-app integrations, by design. The previous clipboard is restored after a
150 ms settle, and `ClipboardOutcome` reports what happened to it so a lost image is
surfaced rather than silent. The two backends are not symmetric in one way that matters:
a macOS `CGEvent` sets its modifier flags absolutely, while Windows' `SendInput` is read
against the real keyboard, so `platform/windows/text.rs` must clear modifiers the user is
still holding or a held Alt turns Ctrl+V into Ctrl+Alt+V.

**Frontend** — `src/types/index.ts` mirrors the Rust serde shapes by hand (snake_case,
`LanguageSelection` is tag/content). Change a Rust type that crosses IPC and change the TS
type in the same edit. `src/lib/platform.ts` mirrors `platform::strings` the same way, by
hand, and gets the platform from `@tauri-apps/plugin-os` rather than a custom command. `stores/appState.ts` is a `readable` fed by `get_recording_state` +
the `state_changed` event; `stores/settings.ts` writes optimistically and then replaces
state with whatever `update_settings` returns, since the backend may reject or normalise.
There are two entry points, `index.html` and `overlay.html`, listed in
`vite.config.ts` — a new window means a new entry there, a `label` in
`tauri.conf.json`, and a capability file naming that label, or `listen`/`invoke` are
denied in it. `src/overlay/` must not import `app.css`: it paints `body` opaque.

## Invariants worth not breaking

- **Privacy is architectural.** Audio stays in memory and never touches disk. Never log or
  persist audio samples, transcription text, clipboard contents, or dictionary entries —
  log shapes only (durations, sample counts, char counts, event names). No telemetry, and
  no network call other than an explicitly requested model download.
- **Errors that reach the UI are written for a person.** Each error enum has a
  `user_message()` returning plain language that names where to fix the problem; raw detail
  goes to `tracing`. Existing tests assert those messages leak no internals (`ort::`,
  `CoreAudio`, `NSPasteboard`, `SendInput`, …) — a new variant needs an arm and usually a
  test. Anything naming an OS settings pane or a shortcut comes from `platform::strings`,
  never a literal, or the message is wrong on the other platform.
- **An empty transcription is a surfaced outcome, never a silent no-op**, and audio over
  `CHUNK_THRESHOLD_SECS` (18 s) must be chunk-decoded. Both come from a measured failure
  mode; see `docs/decisions/0001-parakeet-inference-runtime.md` before touching the
  constants in `asr/parakeet.rs`. That ADR also documents why the execution provider is
  CPU and not CoreML (2.9× slower, 4.5× the memory on this int8 graph).
- Parakeet v3 detects language but cannot be pinned, hence two separate `Capability`
  values; `LanguageSelection::Fixed` is refused up front rather than silently ignored.
- Closing the settings window hides it — a tray app must not quit on window close
  (`on_window_event` in `lib.rs`).
- **The overlay is never focused, and `set_focus` is never called on it.** Insertion
  pastes into whatever app has focus, so an overlay that became key would make itself
  the paste target. It stays harmless because it is built `focusable: false`, which
  tao turns into a `canBecomeKeyWindow` override on macOS and `WS_EX_NOACTIVATE` on
  Windows. Keep that key in `tauri.conf.json`, and keep `apply` calling only `show`.
- **Hotkey events can arrive concurrently.** Windows repeats `WM_HOTKEY` while a key is
  held and `global-hotkey` watches for the release on a thread per repeat, so `Released`
  can be delivered several times at once. `dictation::on_hotkey` claims an `AtomicBool`
  before ending a recording; a check-then-act on `is_recording()` is not enough.
- **Never register or unregister a shortcut from the hotkey handler.** The plugin holds
  its shortcut map locked for the whole handler and sends registration to the main
  thread to block on — and on macOS the handler *is* the main thread, so either way it
  deadlocks. Everything in the chord path goes through `dictation::off_handler`.
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
