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
cargo run --release --example pipeline_check -- --model canary-180m-flash --language de b.wav
cargo run --release --example refine_check            # installs the cleanup model, then measures it
cargo run --example mute_check 3                      # mutes the real output device for 3 s, then restores
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
`dictionary.json`, `history.json` when the user has asked for one, `models/`) — `%APPDATA%\com.bartek.whisperfree\` on Windows. Paths
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

**Onboarding** is a takeover of the settings window, not a window of its own:
`App.svelte` renders `components/Onboarding/` instead of the settings shell while
`settings.onboarding_completed` is false. That flag **defaults to `true`**, which is
load-bearing: `Settings` is `#[serde(default)]`, so a file written before onboarding
existed has no such key and a `false` default would re-onboard every existing user. The
one place it becomes false is a first run in `setup` (`lib.rs`), detected as "no settings
file at all" and written to disk immediately so an abandoned run resumes. The window is
opened on that flag rather than on `first_run`. Permissions come from
`commands::get_permissions`, which returns a `platform::PermissionState` per capability;
`Unasked` is the only value with a system prompt behind it, `Denied` only earns a link to
a settings pane, and `NotRequired` is how Windows says it does not gate synthetic input,
which is what removes the Accessibility step there. Nothing is granted in this process, so
`stores/permissions.ts` polls; it is a `readable`, so it only polls while the settings
window is open. No step blocks the next one: the primary button changes its label to
`Skip for now` instead of being disabled, and the last panel lists what was skipped. See
`docs/decisions/0007-first-run-onboarding.md`.

**History (`history/`)** is the opt-in local record of what was dictated (decision 0011),
and the one place the app writes transcription text to disk. `settings.history_enabled`
defaults to **`false` and must stay that way** — the opposite of `mute_while_recording`,
because a feature that writes down what someone said has to be chosen rather than
inherited from `Default`. `HistoryRetention` carries two separate pure rules that are easy
to confuse: `cutoff` is how old is too old, and `persists` is whether entries reach the
disk at all. `Session` is the retention where `persists` is false, which is why it is a
retention rather than a second checkbox. Choosing it, or switching the feature off,
**deletes `history.json`** rather than merely ceasing to write it; `history::open` does
that at launch and `update_settings` does it on the spot. The list is bounded by age *and*
by `MAX_ENTRIES` (500), so `Forever` still has a ceiling. `remember` in `dictation.rs`
runs after the insertion succeeded and is advisory exactly as refinement and muting are: a
poisoned lock or a full disk is a log line, never a failed dictation, because the words are
already in the user's document by then. Nothing here is ever logged. `keep_on_clipboard`
is the other half of the same decision and lives in `TextInserter::insert` as a parameter:
when set, no snapshot is taken at all and the outcome is `ClipboardOutcome::Kept`.

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

**Muting (`mute/`)** hushes the rest of the machine while the microphone is open
(decision 0009). Structurally it is the overlay again: `mute::wants_silence` is a pure
rule over the snapshot exactly as `overlay::wants_overlay` is, and `mute::apply` is called
from `publish_state`. **That placement is load-bearing.** The obvious hook, beside
`ctx.audio.stop()`, misses a path where that call never happens at all: an Escape arriving
between `claim_finish` and `finish`'s `cancelled` check is caught by neither, so the
stream is left open. Every path out of a recording does publish *something*, which is what
makes `publish_state` the one place the restore cannot be missed. The mute lasts exactly
`AppState::Recording`; transcription and the paste run with sound back on. `MuteEngine`
owns the device on its own thread for the reason `AudioEngine` does (a Windows COM
interface belongs to its apartment), and its sends carry no reply channel because
`publish_state` may be running on the hotkey handler's thread; `restore_blocking` is the
one exception and only the `RunEvent::Exit` arm uses it. Muting is advisory like
refinement: no `AppState`, no error enum, and a device that will not go quiet is a debug
line. `settings.mute_while_recording` defaults to **`true`**, so existing installs pick it
up, which is the opposite of what `onboarding_completed` does; the reasoning for both is
in decision 0009. The microphone test deliberately does not mute, which falls out of it
never entering `AppState::Recording`.

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
  `SpeechRecognizer` trait. `asr/onnx.rs` is the only file that may name `transcribe-rs`,
  ONNX, quantisation, or chunk sizes; `OnnxRecognizer` holds a `Box<dyn SpeechModel>` and
  dispatches on `OnnxEngine`, so another model in a family already supported is a
  `ModelDescriptor` and nothing else. A new *family* is one `OnnxEngine` variant, one
  `EngineKind` variant, and one arm in `build_recognizer` (`lib.rs`). Chunk sizes belong
  to the engine (`Chunking`), not the module: decision 0001's numbers were measured for
  Parakeet and Canary only inherits them. See
  `docs/decisions/0008-a-second-speech-model-and-language-choice.md`.
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
  backend missing any item of the contract fails to compile. `OutputMute` is the shape to
  copy when a backend has to remember something: a public newtype over a private
  `backend::` type, holdable from outside and inspectable only within. Adding Linux is one
  directory plus one `cfg_attr` line. See `docs/decisions/0002-cross-platform-platform-layer.md`.

**Models (`models/`)** are static `ModelDescriptor`s with per-file pinned SHA-256; nothing
is bundled and nothing downloads without the user asking. Digests can be read from
HuggingFace's tree API without downloading — the LFS `oid` is the SHA-256 — but a file not
stored in LFS (`vocab.txt`) has to be fetched and hashed. `ModelFile.base_url` overrides
the descriptor's for one file: Canary needs Parakeet's `nemo128.onnx` preprocessor and its
own repository does not ship one. `download_model` returns
immediately and the worker thread emits `model_download_progress` /
`model_download_completed` / `model_download_failed`, then reloads the recogniser.
`load_installed_model` is that reload and is also what a change of `settings.model_id`
calls, from `update_settings`: nothing else swaps the recogniser, and one left over from
the previous choice answers under the new settings, which is how a language pinned for
Canary reached Parakeet and was refused. It drops the loaded model before loading the
next, so the peak stays at one model and a model that is chosen but not installed leaves
the app `Uninitialized` rather than quietly dictating with the old one.

**Text insertion (`text_insertion/`, `platform/*/text.rs`)** is clipboard + a synthetic
paste — no per-app integrations, by design. The previous clipboard is restored after a
150 ms settle, and **every flavour of it is, not just the text**: `platform/*/clipboard.rs`
snapshots the whole pasteboard and writes it all back, so a rich clipboard survives a
dictation. That module is backend-private (`platform::backend::clipboard`), and the
clipboard plugin is still what *writes* the transcription and only that, since it cannot
express what these read. Reading every flavour is also load-bearing rather than thorough:
both systems advertise flavours they have not produced (plain text derived from RTF, a
delay-rendered Windows format) and serve them by asking the app that did the copying, so a
read is what turns a promise into bytes while the owner is still there to serve it. The old
code read only text, and inferred `NonTextReplaced` from that read failing, which is how a
clipboard holding unfulfilled-promise text got reported to users as an image.
`ClipboardOutcome::after_restore` and `lost_the_clipboard` are pure, shared by both
backends and carry the tests; `PartlyRestored` is deliberately **never surfaced**, because
an unreadable flavour is nearly always a derived one that returns by itself. Only
`RestoreFailed` reaches the user. See
`docs/decisions/0010-preserving-the-clipboard.md`. The two backends are not symmetric in
one way that matters:
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
`stores/models.ts` exists for the same keepalive reason as `update.ts`, and its listeners
are attached at module load rather than on mount, so a 671 MB download survives switching
section, finishing onboarding, or closing the window; onboarding and Settings › Models are
two views of one download. A component may not name a variable `state`: `$state` is a
rune, and Svelte then reads it as a store subscription.
There are two entry points, `index.html` and `overlay.html`, listed in
`vite.config.ts` — a new window means a new entry there, a `label` in
`tauri.conf.json`, and a capability file naming that label, or `listen`/`invoke` are
denied in it. `src/overlay/` must not import `app.css`: it paints `body` opaque.

## Invariants worth not breaking

- **Privacy is architectural.** Audio stays in memory and never touches disk — that part
  has no exception. **Never log** audio samples, transcription text, clipboard contents, or
  dictionary entries; log shapes only (durations, sample counts, char counts, event names).
  Never *persist* any of them either, with one exception: **transcription text, when
  `settings.history_enabled` says so** — off by default, deleted when switched off, and
  written only by `history/`, which itself logs counts and never text. No telemetry, and
  no network call other than an explicitly requested model download **or an update check
  the user has switched on** — `settings.check_for_updates` is off by default and
  `update::watch` re-reads it every tick, so nothing reaches the network until someone
  presses the button or ticks the box. Two decisions amended this paragraph and both did it
  the same way, by rewriting the sentence rather than quietly breaking it:
  `docs/decisions/0006-in-app-updates.md` for the network clause, and
  `docs/decisions/0011-keeping-what-was-dictated.md` for the persistence one.
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
- **Parakeet detects a language and cannot be pinned; Canary is pinned and cannot
  detect.** Hence two separate `Capability` values, and `check_language_request` refuses a
  mismatch up front rather than silently ignoring it. Because one `settings.language`
  serves both, `models::normalise_language` maps a selection onto the nearest thing the
  chosen model can honour, and `update_settings` runs it on every save — it is pure, a
  fixed point, and its output is tested never to be something `check_language_request`
  would refuse. Getting this wrong is not a visible error: Canary given the wrong source
  language *translates* into it, fluently. A model's declared languages are the ones
  measured to work, not the ones its model card lists — Canary 180M Flash claims Spanish
  and returns an empty string for it. See decision 0008.
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
