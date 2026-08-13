# 0003 — Two-step hotkey chords

**Status:** accepted
**Date:** 2026-08-13
**Applies to:** plan §10; extends decision 0002

The hotkey recorder accepted one combination. This decision records how `⌘K K` — a
prefix followed by a second key, the shape VS Code made familiar — was made to work
as a *global* shortcut, and why the obvious implementation was not used.

## Why this is not just a parsing change

The OS APIs underneath take modifiers plus exactly one key.
`tauri-plugin-global-shortcut` hands a `Shortcut` to `global-hotkey`, whose `HotKey`
is `{ mods: Modifiers, key: Code }`; that reaches `RegisterEventHotKey` on macOS and
`RegisterHotKey` on Windows. **There is nowhere to put a second step.** A chord has
to be assembled by the app out of two registrations.

The deeper difference is that a chord in an editor is not the same object as a chord
in a global shortcut. VS Code owns its keyboard focus: while it waits for the second
key, it is only withholding keys from itself. A global prefix is withheld from
*every* application — so `⌘K` stops working in Slack, in the browser, everywhere,
for as long as the app runs. That is a larger imposition than the single-combination
hotkey it replaces, not a smaller one, and it is what most of the design below is
paying for.

## The mechanism

Only the prefix is registered. When it fires:

1. `dictation::arm_chord` registers the **second step**, alongside the prefix, and
   opens an 800 ms window (`chord::CHORD_TIMEOUT`).
2. If the second step fires, that is the dictation trigger — press and release are
   passed to `hotkey::decide` exactly as a plain hotkey's would be.
3. If the window lapses, the second step is unregistered and the prefix is
   **replayed** into whatever has focus, so the app that should have received it
   still does.

The second step is claimed for well under a second, and only after the user has
already pressed the prefix. A bare `K` is therefore an acceptable second step even
though it would be an unacceptable hotkey — which matters, because `⌘K K` is the
notation people expect. `toAccelerator` in `src/lib/hotkey.ts` relaxes its
bare-key rule for exactly that position and no other.

**The second step is released on its key-up, not on the timeout**, and the window
has three states rather than two to make that true:

| `chord::Window` | second step | the timeout |
| --- | --- | --- |
| `Closed` | not registered | — |
| `Open` | registered, waiting | may abandon the chord |
| `Held` | registered, key is down | must not touch it |

`Held` is the one that is easy to leave out, and leaving it out is a bug that only
shows up in hold-to-talk: the window stays abandonable while the key is down, so
800 ms into a recording the timeout unregisters the second step and replays the
prefix — and the key-up that ends the recording is never delivered, because a
release only arrives while the shortcut is still registered. Toggle mode hides it
completely, since toggle only ever needs presses. A test that presses and releases
faster than the timeout hides it too.

The other direction matters just as much: leaving the second step registered after
the key-up would swallow a bare key system-wide for as long as a toggle-mode
recording ran.

Nothing re-opens a `Held` window except the prefix firing again, which is also the
way out if a key-up is ever missed — otherwise a missed release would keep a bare
key claimed indefinitely.

## Replaying the prefix

`platform::replay_keystroke` posts a synthetic press: `CGEvent` at HID level on
macOS, `SendInput` on Windows — the same techniques, and the same asymmetry, as
text insertion in decision 0002. macOS sets the modifier flags on the event;
Windows must release whatever the user is physically holding first.

Two things make this less simple than it sounds.

**The registration must come off first.** A posted HID event is seen by our own
global shortcut, which would arm the chord again, and again. So the prefix is
unregistered, replayed, and re-registered after a 50 ms settle — the event is
delivered asynchronously, and re-registering immediately races the press just sent.
For that 50 ms the hotkey is not live. If the unregister fails the replay is
skipped entirely: costing the user one keystroke is better than a loop that costs
them the hotkey.

**Each backend needs a `Code` → virtual-key table.** `KeyboardEvent.code` names are
positional, and so are `kVK_ANSI_*` and the `VK_OEM_*` block, which is the right
level: a chord prefix is the physical key that was pressed. The tables cover exactly
what the recorder in `src/lib/hotkey.ts` can produce, and a test in each backend
asserts that — a gap there would be a prefix that can be swallowed but never handed
back. An unmapped key is reported, never posted as key code 0, which is `A` on
macOS.

## Nothing may register a shortcut from the hotkey handler

`tauri-plugin-global-shortcut` holds its shortcut map locked for the whole of the
handler, and sends registration to the main thread and blocks waiting on it. On
macOS the handler *is* the main thread. Calling `register` or `unregister` from
inside it therefore deadlocks — against the map on both platforms, and against the
main thread on macOS.

Every registration in the chord path consequently runs on a thread of its own, via
`dictation::off_handler`. The pure decisions (`chord::classify`, `chord::Arming`)
stay where they can be tested without a window server, in the same spirit as
`hotkey::decide`.

Windows repeats `WM_HOTKEY` while a key is held, so a held prefix re-arms rather
than being ignored, and a generation counter on `Arming` stops a timeout that
fires late from abandoning a window some later press has since opened.

## The hotkey is suspended while a new one is recorded

A registered shortcut is claimed from our own windows too, so the recorder in
Settings could never see the combination the user most likely wants to
re-record — the current one. Pressing it started a dictation instead. This was
already true of a plain hotkey and merely unavoidable for a chord, whose prefix
*is* the thing being re-recorded, so `suspend_hotkey` / `resume_hotkey` bracket the
capture. Resume registers whatever the live hotkey is by then: the new one if it
was accepted, the previous one if it was not.

## Storage

A chord is two accelerators separated by a space, in the one `hotkey` string —
`"Cmd+K K"`. No new IPC shape, no migration, and an existing `"Alt+Space"` keeps
parsing as a one-step hotkey. Whitespace is also the one thing an accelerator
cannot contain, and it is how VS Code writes chords.

`Chord::parse` refuses more than two steps, and refuses two identical steps: both
halves would arrive as the same registration, leaving no way to tell which fired.

## Rejected: a system-wide event tap

The alternative is `CGEventTap` on macOS and a `WH_KEYBOARD_LL` hook on Windows,
watching every keystroke and interpreting the sequence in the app. It would remove
the replay problem — the prefix could be passed through or swallowed as the tap
decides, with no synthetic press and no 50 ms gap — and it would allow things
registration cannot express at all, such as double-tapping a modifier.

It was rejected because it is a large amount of hand-written platform code on the
path of *every* keystroke the user types, in an app whose privacy posture
(decision 0001, plan §18) is that nothing about what is typed or said leaves
memory. The registration approach touches the keyboard only for the combinations
the user chose, and stays entirely on the official plugin. If double-tap-a-modifier
is ever wanted, that is the decision to revisit — it cannot be built on
registration.
