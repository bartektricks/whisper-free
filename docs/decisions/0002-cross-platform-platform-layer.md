# 0002 — The cross-platform layer, and adding Windows

**Status:** accepted
**Date:** 2026-08-13
**Applies to:** plan §22, §23.6; supersedes nothing

WhisperFree shipped macOS-only, but with `platform/` carved out in advance and a
`compile_error!` where the second backend would go. This decision records what that
seam became when a real Windows backend was put behind it, and what the deferred
Linux backend will have to supply.

## What turned out to be portable

Two things that looked like the hardest parts of the port are not problems at all.

**ONNX Runtime is statically linked.** With the default `download-binaries` feature,
`ort-sys` emits `cargo:rustc-link-lib=static=onnxruntime`; `otool -L` on the release
binary shows no onnxruntime dependency, and no dylib appears in `target/`. There is
therefore **no runtime library to ship** in a CPU-only build, on any platform — the
NSIS installer carries nothing extra. This stops being true the moment a GPU
execution provider is enabled, since DirectML and CUDA ship provider DLLs.

**Global hotkeys need no per-platform code.** `tauri-plugin-global-shortcut` covers
all three desktop platforms, and — critically for hold-to-talk — reports key
*release* as well as press. Windows' `RegisterHotKey` never sends a release, so
`global-hotkey` synthesises one by polling `GetAsyncKeyState` on a thread it spawns
per `WM_HOTKEY`. That has a consequence of its own; see "The race Windows exposed".

`cpal` (WASAPI), `rubato`, `ureq`, `sha2` and Tauri's path resolver were portable
as-is. `asr/` needed no source change whatsoever, which is the boundary from
decision 0001 paying for itself.

## The backend contract

`platform/mod.rs` selects one private module at compile time:

```rust
#[cfg_attr(target_os = "macos",   path = "macos/mod.rs")]
#[cfg_attr(target_os = "windows", path = "windows/mod.rs")]
mod backend;
```

Two properties are the point of this shape:

- **`mod backend` is private.** `platform::macos::…` is unnameable from application
  code, so the rule that used to live in a doc comment is now enforced by the
  compiler.
- **Every backend must supply the whole contract** — `text::Inserter`,
  `replay::send`, the four `focus` items (`SCREEN_UNIT`,
  `focused_window_centre`, `pointer_position`, `window_position`),
  `become_menu_bar_app`, `float_above_other_windows`, `DEFAULT_HOTKEY`, the
  three tray constants, and `strings` — or it does not compile. A missing item
  is an error, not a silent divergence.

The cost is that neither developer machine ever compiles the other's backend. That
is why `.github/workflows/ci.yml` runs the matrix; it is not optional infrastructure.

`strings` exists because of the "errors are written for a person" invariant. An error
has to name *where* to fix the problem, and that place has a different name on every
platform. `INSERT_PERMISSION_DENIED` is a whole sentence rather than a place name,
because the two platforms do not fail for the same reason: macOS withholds a
permission before anything happens, while Windows refuses input to a more privileged
window once the text is already on the clipboard.

## Text insertion: the only genuinely per-platform code

There is no official plugin for synthetic keyboard input, and there is unlikely to
be one — it is the capability every OS guards most carefully. Both backends follow
the same sequence (read clipboard → write text → paste → settle → restore) and
return the same `ClipboardOutcome`, but the paste itself differs.

**A macOS `CGEvent` states its modifiers absolutely.** `set_flags(CGEventFlagCommand)`
means the target sees exactly Cmd+V no matter what the user is physically holding.

**`SendInput` has no equivalent.** The receiving application reads the real keyboard
state, so an Alt still held from `Ctrl+Alt+Space` turns our Ctrl+V into **Ctrl+Alt+V**
— which in most editors does something entirely different. The Windows backend
therefore waits a bounded 200 ms for the hotkey modifiers to clear and then forces a
synthetic key-up for whatever is left. Usually the wait exits immediately: in
hold-to-talk the release *is* what triggered transcription. `VK_CONTROL` is
deliberately excluded from that set, since a held Ctrl is the Ctrl we were about to
press. The rule is a pure function, `modifiers_to_clear`, testable without a keyboard.

The second Windows-only behaviour is **UIPI**: a normal-integrity process may not
send input to an elevated window. `SendInput` reports it by accepting fewer events
than it was given, with `ERROR_ACCESS_DENIED`. That is mapped to
`InsertError::PermissionDenied`, which is what gives that variant meaning on a
platform with no permission to grant.

## The race Windows exposed

Windows repeats `WM_HOTKEY` while a key is held, and `global-hotkey` spawns a release
watcher per repeat. So a single hold produces several `Released` events, delivered
concurrently from different threads. `dictation::on_hotkey` read `is_recording()` and
then acted on it, with no lock across the gap — two threads could both decide to
stop, and the loser's `audio.stop()` would surface as a spurious error banner.

Fixed with an `AtomicBool` claimed by `compare_exchange`. It is not `#[cfg]`-gated:
the check-then-act gap was always wrong, macOS just never delivered the events that
made it visible.

## Plugins adopted, and rejected

Adopted: **single-instance** (two instances would fight over the shortcut and load
two ~1.4 GB models; far easier to trigger on Windows than on macOS, and it must be
registered first), **os** (the frontend needs the platform to render hotkey glyphs),
**opener** (free `open_url`, replacing a `std::process::Command` shell-out),
**clipboard-manager** (one managed clipboard rather than one per insert; note it
wraps `arboard` internally, so that crate stays in the tree transitively).

Rejected, so the reasoning is not re-litigated: **log** — `tracing-appender` is
already cross-platform and the current setup is shaped by the privacy invariant;
**store** — settings are Rust-owned with fault-tolerant load and write-tmp-rename,
which a frontend key-value store would lose; **http** — model download needs
streaming, SHA-256 and cancellation on a worker thread, which `ureq` already does;
**notification** and **updater** — real future value, but product changes rather
than porting work.

Capabilities gate the **JavaScript** API only, so only `os` needed an entry. This is
also why global-shortcut has always worked with just `core:default` despite its
commands being "disabled by default" in the docs.

## Execution provider on Windows

Decision 0001's conclusion — CPU beats CoreML by 2.9× on this int8 graph — is scoped
to Apple Silicon and **does not transfer**. Windows inherits the CPU provider by
default *by inheritance, not by measurement*. `OrtAccelerator` already has a
`DirectMl` variant and the provider is a runtime setting, so benchmarking it later
costs a feature flag and a config line, not a redesign. No claim should be made about
Windows inference performance until someone measures it on Windows hardware.

## Linux, deferred

Not built, and deliberately so rather than by omission. A `platform/linux/` backend
would supply the same contract, and on X11 the insertion path is a straightforward
XTEST synthetic Ctrl+V.

**Wayland is the reason this is separate work.** It forbids synthetic input outright.
Doing it properly needs the XDG `RemoteDesktop` portal — which shows a consent dialog
once per session, changing the app's interaction model — plus the `GlobalShortcuts`
portal for the hotkey, with uneven support across GNOME, KDE and Sway. That is a
product decision as much as an engineering one, so it does not belong bolted onto a
Windows port.

Three knock-on notes for whoever picks it up:

- `ort`'s default `tls-native` feature needs OpenSSL headers on Linux CI; switching
  `ort` to `tls-rustls` avoids the system dependency.
- ALSA device ids are positional (`hw:N,M`) and do not survive a replug, unlike the
  CoreAudio and WASAPI ids that `audio/capture.rs` assumes when it persists a chosen
  microphone. It fails loudly rather than silently recording from the wrong device,
  but the experience will be worse.
- `arboard`'s `wayland-data-control` feature is what makes the Wayland clipboard
  work, and clipboard-manager already enables it.
