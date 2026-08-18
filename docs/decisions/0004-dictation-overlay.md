# 0004 — The dictation overlay

**Status:** accepted
**Date:** 2026-08-13
**Applies to:** plan §14, §15; supersedes nothing

Until now the app gave no sign it was working. The tray status line needs the menu
open and the status badge needs the settings window open, so in the case the app is
actually built for — hotkey pressed while typing in something else — LocalDictation
was invisible. You could not tell recording had started, that the hotkey had been
swallowed by another app, or that transcription had failed.

The overlay is a small pill that floats above whatever the user is working in while
dictation runs, and for four seconds when it fails. It is off-limits to the mouse and
never takes focus, and it can be switched off entirely.

## Focus is the whole problem

Text insertion pastes into **whatever application has focus**. An indicator that
became the key window when it appeared would make itself the paste target and break
dictation outright — not degrade it, break it. Every other decision here follows from
avoiding that.

The obvious answer on macOS is an `NSPanel` with `NSWindowStyleMask::NonactivatingPanel`,
which is what superwhisper uses. That means `objc2` or `tauri-nspanel`, a macOS-only
code path, and a Windows equivalent invented separately — a lot of surface area for
one window.

It turns out not to be necessary. tao already implements exactly this, on both
platforms, behind one window attribute:

- `tao/src/platform_impl/macos/window.rs` overrides `canBecomeKeyWindow` and
  `canBecomeMainWindow` with a `focusable` ivar taken from the window attributes. With
  `focusable: false`, `set_visible(true)` still calls `makeKeyAndOrderFront:`, but the
  window declines to become key — and `makeKeyAndOrderFront:` never activates the
  application, so the frontmost app keeps both focus and its active appearance.
- `tao/src/platform_impl/windows/window_state.rs` turns the absence of the same flag
  into `WS_EX_NOACTIVATE`, which is the same guarantee.

Tauri exposes it as the `focusable` window-config key, which
`tauri-runtime-wry` forwards to `with_focusable`. So `platform/` is untouched by this
feature: there is no OS-specific overlay code to keep in step between two backends.

`overlay::apply` therefore calls `show()` and never `set_focus()`. That is the
opposite of `tray::show_settings_window`, which must focus, and the difference is
worth noticing before editing either.

**Verified by driving the real app**, because no unit test can reach this: with the
overlay on screen, keystrokes sent to TextEdit still landed in TextEdit, and a real
dictation logged `text_inserted chars=13 clipboard=Restored`.

## Window level, and the full-screen Space that is still not solved

`always_on_top` maps to `NSFloatingWindowLevel` (3), which ties with every other
floating window on screen — another app's picture-in-picture will happily cover the
indicator. `platform::float_above_other_windows` raises it to
`NSStatusWindowLevel` (25) and sets the Spaces collection behaviour, which is
`platform/`'s only job in this feature. Two details cost time and are worth keeping:

- **Assign the collection behaviour, do not OR into it.** tao's default includes
  `Managed`, and Apple's rule is that at most one of
  `Managed`/`Transient`/`Stationary` may be set — so OR-ing `CanJoinAllSpaces` on top
  of `Managed` silently does nothing.
- **Raising the level further does not reach a full-screen Space.** Measured, not
  assumed: levels 3, 25, 101 (`NSPopUpMenuWindowLevel`) and 1000
  (`NSScreenSaverWindowLevel`) were each tested against a full-screen `TextEdit`,
  with both `CanJoinAllSpaces` and `MoveToActiveSpace`, and with the overlay already
  visible before the Space switch. None appeared. The obstacle is Space membership,
  not z-order.

So the limitation stands, now with the reason understood: **a plain `NSWindow` is not
admitted to another application's full-screen Space.** Doing it needs the window to
be an `NSPanel` with `NSWindowStyleMask::NonactivatingPanel` — which tao never
creates, and which is exactly why comparable apps carry `tauri-nspanel`. That is a
dependency and a runtime `object_setClass` on a window Tauri owns; worth doing if
dictating inside full-screen apps matters, but not worth doing blind. The level is
deliberately left at 25 rather than parked in the screen-saver band, since the extra
height was measured to buy nothing.

Transparency on macOS is gated behind Tauri's `macos-private-api` feature, which
`tauri-runtime-wry` refuses to run without. A transparent window is what allows a
rounded pill instead of a rectangle, so the flag is unavoidable. It costs Mac App
Store eligibility, which this app has already forfeited by synthesising keyboard
input and requiring Accessibility. The feature is named in `Cargo.toml` as well as in
`tauri.conf.json`: the Tauri CLI infers it from the config, but CI runs `cargo clippy`
directly, and the two builds must not disagree.

## Rust decides whether, the webview decides what

`overlay::apply` is called from `publish_state` — the single funnel every state
transition already passes through — and does nothing but size, position, show and
hide. The pill's appearance comes from the `state_changed` broadcast the settings
window already listens to, which the overlay webview receives unchanged because
`app.emit` goes to every window.

Two consequences worth keeping:

- `publish_state` emits **before** it calls `apply`, so the webview has been handed
  the new snapshot before its window is ordered front. A 140 ms fade-in covers the
  remaining few milliseconds.
- `apply` posts its window work with `run_on_main_thread` and returns. `publish_state`
  runs on whatever thread caused the transition — the `dictation` worker, an audio
  error callback, or the hotkey handler, which on macOS *is* the main thread. Posting
  rather than blocking is the same precaution as never registering a shortcut from
  the hotkey handler (decision 0003).

The overlay is a second Vite entry point rather than a route inside `App.svelte`.
Routing would pull the entire settings bundle into a webview that renders one pill;
as built, the overlay chunk is about 1 KB.

## Placement is an anchor, not coordinates

The overlay ignores the mouse (`set_ignore_cursor_events(true)`, which has no config
key and so is set once at startup), which means it cannot be dragged. Rather than add
a mode that temporarily makes it draggable, position is chosen from a nine-cell grid
in Settings.

Named anchors also survive things pixel coordinates do not: a saved position from a
monitor that has since been unplugged strands the overlay off-screen, while an anchor
is meaningful on whichever display the user is working on. `apply` resolves it against
the active monitor (the next section), falling back to the primary, and against that
monitor's `work_area` so the inset is measured from usable screen rather than from
underneath the menu bar or the Dock.

`place` is a pure function over a rectangle and a size, so every anchor, a second
display at a negative origin, and a work area smaller than the window are all covered
by unit tests — the same separation as `hotkey::decide` and `state::is_valid`.

## Which display it appears on

An anchor still has to be resolved against *a* screen, and the first version used the
screen under the pointer. That is the wrong screen twice over.

The pointer is not where the user is working. It is left wherever it was last put —
frequently on another display — while the typing, and therefore the paste, happens in
the focused window. The indicator for text about to land in window X belongs on X's
display.

And the pointer lookup did not work anyway. `AppHandle::cursor_position` on macOS
returns global points multiplied by the **primary** monitor's scale factor (tao
`util::cursor_position`), while `monitor_from_point` compares that number against
`CGDisplayBounds`, which is in **points** (tao `platform_impl/macos/monitor.rs`). On a
2× display every coordinate handed to the lookup was therefore twice what it should be:
usually nothing matched and the code fell back to the primary display, and on some
arrangements a *different* monitor matched and the overlay appeared on the wrong one.
Windows was never affected, since `GetCursorPos` and `MonitorFromPoint` are both in
physical pixels.

`platform::active_monitor` replaces it, and answers in one step: the focused window's
display, else the pointer's, else `None` — leaving `overlay::apply` to fall back to the
primary rather than being handed a guess. It lives in `platform/` because the coordinate
space is a platform fact, not an application one.

- **macOS** asks the Accessibility API: `AXFocusedApplication` → `AXFocusedWindow` →
  `AXPosition`/`AXSize`. The permission is already required to paste, and the request is
  read-only geometry — no title, no value, no application name — so nothing is learned
  about what the user is doing. Three details worth keeping: the messaging timeout is
  lowered to 250 ms, because accessibility calls are synchronous IPC that default to six
  seconds and this runs on the main thread as dictation starts; the pointer fallback
  reads a synthetic `CGEvent`'s location rather than `NSEvent.mouseLocation`, whose
  bottom-left origin would have to be flipped against the main display's height; and
  **the focused application's pid is compared with our own before anything is asked of
  it.** `AXUIElement.h` warns that an application talking to itself over this API can
  deadlock, and the thread that would have to answer is the same main thread that is
  blocked waiting — which is reachable simply by pressing the hotkey with the settings
  window focused. The timeout would bound that to a stall rather than a hang, but a
  stall on every state transition is a freeze as far as the user is concerned, and when
  our own window has focus the pointer is the better signal anyway.
- **Windows** asks `GetForegroundWindow` and `GetWindowRect`. No permission, and no
  conversion — the virtual desktop is physical pixels throughout, which is the whole of
  what the `ScreenUnit` argument records.

Containment itself is a pure function over `MonitorBounds`, so the Retina case that
caused all this is a unit test rather than something you need two monitors to see. So
is the *order* the two measurements are trusted in: the backends supply
`focused_window_centre`, `pointer_position` and the `SCREEN_UNIT` they are measured in,
and `platform::active_monitor` owns the fallback. Two platforms cannot drift apart on
the decision, and the decision is covered by tests.

### Picking the display is only half of it

Knowing the right monitor does not place the window on it. `place` works in the target
monitor's physical pixels — that is what `work_area` is reported in — but
`set_outer_position` on macOS converts a physical position using the scale factor of
the display the window is **currently** on (tao `platform_impl/macos/window.rs`). While
the overlay is still on a 2× built-in and the pill has been placed on a 1× external at
x = 2680, that number is halved to 1340 points and the pill appears in the middle of
the built-in display. The reverse direction sends it off-screen entirely, and it never
converges, because the window's scale factor only changes once it has actually moved.

Same class of bug as the `cursor_position` one above, and it was hidden by it: while
the display lookup always fell back to the primary, the window was already there, so
the two scale factors always agreed.

So `platform::window_position` converts `place`'s output into whatever the platform
positions windows in — a `Position::Logical` in points on macOS, which tao passes
through untouched, and the unchanged `Position::Physical` on Windows, where
`SetWindowPos` wants virtual-desktop pixels. Which units those are is a platform fact,
so it sits beside `active_monitor` rather than in `overlay.rs`.

The display is resolved on every call to `apply`, which is every state transition, so
the pill follows focus if it moves between recording and insertion.

Two things this does not solve, both deliberate: a window straddling two displays is
placed on the one containing its centre, and a minimised or off-screen focused window
falls through to the pointer rather than being forced onto a display.

## Errors

An error shows the same user-facing sentence the tray does, then hides itself after
four seconds. The dismissal runs on a spare thread against an `AtomicU64` ticket that
every `apply` bumps, so a timer that wakes after the user has started dictating again
finds its ticket stale and does nothing. Without that, the four-second timer from a
failure could hide the overlay in the middle of the next recording.

## Escape abandons a run

An indicator that tells you a mistake is under way is only half of it, so Escape
cancels: the recording is dropped, and a transcription already in flight is thrown
away rather than pasted.

Escape is **claimed from the OS only while a dictation is running** — from
`begin` to the end of the pipeline — and handed straight back. A permanently
registered Escape would swallow the key system-wide, which no dictation app is worth.
`AppContext::cancel_key_held` makes the claim single-entry so two starts cannot leave
two registrations behind, or one release cancel the other's claim. Registration still
obeys decision 0003's rule: never from the hotkey handler, always via
`dictation::off_handler`.

Which of the two cancellations happens is decided by the existing `finishing` gate
rather than by `AppState`, which can be stale. Holding it means we own the audio
engine and can stop it; failing to take it means a pipeline is already past recording,
where inference cannot be interrupted part-way — so `AppContext::cancelled` is set and
`finish` checks it immediately before the paste. The work is wasted either way, but
the thing the user actually called off, text appearing in their document, does not
happen.
