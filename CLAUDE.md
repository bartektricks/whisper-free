# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

WhisperFree: a macOS menu-bar / Windows notification-area app (Tauri 2 + Rust backend,
Svelte 5 + TS frontend) that records on a global hotkey, transcribes locally with Parakeet
TDT 0.6B v3 through ONNX Runtime, and pastes the text into the focused app. The directory,
product, and crate are all `whisper-free`; the bundle id is `com.bartek.whisperfree`.

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
cargo run --release --example refine_check            # installs the cleanup model, then measures it
cargo tree -d | grep ort                              # must show one ort and one ort-sys, never two
```

A **lefthook pre-commit hook** (`lefthook.yml`) runs those same checks before every
commit, filtered by what is staged: `bun run check` for frontend files, then `cargo
clippy --all-targets -- -D warnings` piped into `cargo test` for anything under
`src-tauri/`. Clippy and the tests are piped rather than parallel because they share the
target-dir lock. `bun install` installs the hook through the `prepare` script; if it is
ever missing, `bunx lefthook install`. Bypass a run with `git commit -n`. It cannot cover
the CI matrix — a commit made on macOS never compiles `platform/windows/`.

Releases are **manual only**: `.github/workflows/release.yml` has no push or tag
trigger, and is dispatched from the Actions tab with a choice of release / prerelease /
draft / artifacts-only, and which platforms to build. It produces a code-signed (but not
notarized) Apple Silicon `.dmg` and an unsigned Windows NSIS `-setup.exe`. **The app version lives only in
`src-tauri/Cargo.toml`** — `tauri.conf.json` has no `version` key on purpose, so Tauri
falls back to the crate version; `package.json` mirrors it and the run fails if the two,
or the chosen tag, disagree. Publishing needs two **unrecoverable** secrets.
`TAURI_SIGNING_PRIVATE_KEY` is the update key, unrelated to Apple or Authenticode signing:
no later build could produce a signature the installed copies accept. `APPLE_CERTIFICATE`
is the macOS code-signing identity, and it is what holds the app's *designated requirement*
stable across versions — an ad-hoc build's requirement is only the cdhash of the
executable, so macOS keys the Accessibility and Microphone grants to those exact bytes and
shipping one revokes both on every copy that updates, while System Settings goes on showing
them granted. The `resolve` job refuses to start without either. **Neither
`APPLE_CERTIFICATE` nor its password may reach the Tauri CLI's environment**: given both,
the CLI imports the `.p12` itself and then resolves the identity through a list that only
recognises Apple-issued certificate names, so a self-signed one dies at `failed to resolve
signing identity` *after* importing cleanly. The `Set up code signing` step in
`release.yml` installs it instead — into a throwaway keychain, trusted for code signing,
without which `codesign` reports the identity as missing rather than untrusted — and
exports only `APPLE_SIGNING_IDENTITY`. A local `bun run tauri build` reads that name, and
the update key pair, from a gitignored `.env` — the `tauri` script is `dotenv -- tauri`. Hardened runtime comes with signing, so
`src-tauri/entitlements.plist` is load-bearing: without
`com.apple.security.device.audio-input` capture fails in a way that reads exactly like a
denied permission. See `docs/RELEASING.md` and
`docs/decisions/0006-in-app-updates.md`.

There is no root `Cargo.toml`; cargo commands run from `src-tauri/`. The examples are
deliberately outside `cargo test` — they need a microphone, ~671 MB on disk, and network
on first run. `WHISPER_FREE_LOG=whisper_free_lib=debug` overrides the log filter;
logs land in `~/Library/Logs/com.bartek.whisperfree/`, user data in
`~/Library/Application Support/com.bartek.whisperfree/` (`settings.json`,
`dictionary.json`, `models/`) — `%APPDATA%\com.bartek.whisperfree\` on Windows. Paths
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
`refine_text` sits between the second cancel gate and the dictionary, and returns `String`
rather than `Result` on purpose: **refinement is advisory**, so an absent model, a load
error, a rejected rewrite or a cancelled run all resolve to the raw transcription. Order
matters — the model runs first, the dictionary second, so a replacement the user wrote by
hand is never second-guessed.

**Updates (`update/`)** are advisory in the same sense refinement is: the module never
touches `AppState`, so a failed check is a line in Settings › Updates rather than an error
banner over an app that still transcribes. Status travels on its own `update_status_changed`
event and its own `AppContext` field. Two rules are load-bearing. **Nothing here may run on
the main thread** — when the app cannot be overwritten in place, the plugin's macOS path
raises an admin prompt through `run_on_main_thread` and blocks on the reply, so calling it
from the tray menu handler (which *is* the main thread on macOS) deadlocks exactly the way
registering a shortcut from the hotkey handler does; both public verbs return immediately
and work on the async runtime. And **the app never restarts itself**: a finished download
waits at `ReadyToRestart`, and `install` refuses outright while `ctx.audio.is_recording()`
or the state machine is mid-pipeline — checked against the real thing, not `AppState`. The
updater endpoint is `releases/latest/download/latest.json`, which GitHub resolves to
neither a prerelease nor a draft, so choosing `release` in the workflow is the act that
ships to users. See `docs/decisions/0006-in-app-updates.md`.

**Overlay (`overlay.rs`)** is the floating indicator, a second window labelled
`overlay` declared in `tauri.conf.json`. Rust decides *whether* it is on screen and
*where*; the webview (`src/overlay/`, its own Vite entry) decides what it looks like,
from the same `state_changed` broadcast. `publish_state` emits before it calls
`overlay::apply`, so the webview has the snapshot before the window appears, and
`apply` posts its window work with `run_on_main_thread` rather than blocking the
caller — which may be the hotkey handler. `place` is pure and tested per anchor, and *which*
display it is placed on comes from `platform::active_monitor` — the focused
window's screen, then the pointer's, then the primary. Not Tauri's
`monitor_from_point`: on macOS it compares against `CGDisplayBounds` in points
while `cursor_position` returns points times the primary scale factor, so it
misses on any Retina display. The backends supply only the two measurements and
the `SCREEN_UNIT` they are in; the fallback order lives in `platform/mod.rs` and
is tested there. **`place`'s output goes through `platform::window_position`
before `set_position`** — it is in the *target* monitor's pixels, while tao on
macOS converts against the display the window is still *on*, so a pill moving
between displays of different densities lands on the wrong one.
`platform::float_above_other_windows` raises the window level; note that the
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

**Four load-bearing boundaries** (keep them intact):

- `asr/` — outside this module the app only knows `audio -> transcription` via the
  `SpeechRecognizer` trait. `asr/parakeet.rs` is the only file that may name
  `transcribe-rs`, ONNX, quantisation, or chunk sizes. A second engine means a new
  `EngineKind` variant and a new arm in `build_recognizer` (`lib.rs`), nothing else.
- `refine/` — outside this module the app only knows `text -> text`, via the
  `TextRefiner` trait. `refine/onnx.rs` is the only file that may name `ort`,
  `tokenizers`, a KV cache, or a graph input name. `refine/guard.rs` and
  `refine/prompt.rs` are pure and carry most of the tests. A second refinement
  model is a new `EngineKind` arm in `build_refiner` (`lib.rs`) and possibly a
  new `prompt::Template` variant. See `docs/decisions/0005-local-refinement-model.md`.
- `update/` — outside this module the app knows a status and three verbs (`check`,
  `install`, `restart`); `update/plugin.rs` is the only file that may name
  `tauri_plugin_updater`, a manifest, a signature or an archive. `update/schedule.rs` is
  pure and carries its own tests. The JS half of the plugin is deliberately **not**
  installed and the settings window holds no `updater` capability: the tray must be able
  to start a check with no window open, and a download must survive the window closing.
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
the `state_changed` event, and `stores/update.ts` is the same shape over
`get_update_status` + `update_status_changed` — it has to be a store rather than component
state, because `App.svelte` renders one section at a time with no keepalive and switching
away mid-download would otherwise destroy the progress; `stores/settings.ts` writes optimistically and then replaces
state with whatever `update_settings` returns, since the backend may reject or normalise.
There are two entry points, `index.html` and `overlay.html`, listed in
`vite.config.ts` — a new window means a new entry there, a `label` in
`tauri.conf.json`, and a capability file naming that label, or `listen`/`invoke` are
denied in it. `src/overlay/` must not import `app.css`: it paints `body` opaque.

## Invariants worth not breaking

- **Privacy is architectural.** Audio stays in memory and never touches disk. Never log or
  persist audio samples, transcription text, clipboard contents, or dictionary entries —
  log shapes only (durations, sample counts, char counts, event names). No telemetry, and
  no network call other than an explicitly requested model download **or an update check
  the user has switched on** — `settings.check_for_updates` is off by default and
  `update::watch` re-reads it every tick, so nothing reaches the network until someone
  presses the button or ticks the box. See `docs/decisions/0006-in-app-updates.md`, which
  is what amended this sentence.
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
- **A refinement never costs the user their words.** `refine/guard.rs` judges every
  proposal against the raw transcription and rejects anything that strays — the
  thresholds are measured, not guessed, and `measured_corrections_and_rewrites_stay_separated`
  is the test that keeps them honest. The guard bounds *how much* may change, never
  whether the change is right; a wrong substitution scores the same as a right one, which
  is why the stage is opt-in and the dictionary still runs after it. Never make
  refinement able to fail a dictation.
- Parakeet v3 detects language but cannot be pinned, hence two separate `Capability`
  values; `LanguageSelection::Fixed` is refused up front rather than silently ignored.
- Closing the settings window hides it — a tray app must not quit on window close
  (`on_window_event` in `lib.rs`).
- **The overlay is never focused, and `set_focus` is never called on it.** Insertion
  pastes into whatever app has focus, so an overlay that became key would make itself
  the paste target. It stays harmless because it is built `focusable: false`, which
  tao turns into a `canBecomeKeyWindow` override on macOS and `WS_EX_NOACTIVATE` on
  Windows. Keep that key in `tauri.conf.json`, and keep `apply` calling only `show`.
- **The activation macOS defers at launch has to be settled before the first click.**
  tao ends `applicationDidFinishLaunching` with `activateIgnoringOtherApps:`, and macOS
  holds a request from a window-less app until it first puts something on screen. For a
  menu-bar app that is the tray menu — so the activation lands ~250 ms after the menu
  opens and closes it again, on the first click of every run.
  `platform::settle_launch_activation`, called from the `RunEvent::Ready` arm in `run`,
  asks a second time while nothing is open, which settles the request without the app
  ever becoming active or taking focus from anyone. The deprecated call is the one that
  works: `activate`, its replacement, leaves the pending activation exactly as it was.
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
