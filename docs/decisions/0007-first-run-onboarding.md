# 0007: First-run onboarding

**Status:** accepted
**Date:** 2026-08-24
**Applies to:** first launch, the permission contract in `platform/`, and `Settings::onboarding_completed`

A fresh install of WhisperFree could do nothing at all. It opened its settings window,
which was the right instinct, and then said nothing about the three things standing
between the user and a working dictation: microphone access, Accessibility access, and a
671 MB model that is deliberately not bundled. Every one of them fails quietly. A denied
microphone hands macOS's digital silence to the recogniser, which produces an empty
transcription. A missing Accessibility grant transcribes perfectly and pastes nothing.
A missing model leaves the hotkey doing nothing at all, with "No model installed" in a
menu the user has no reason to open. Three different silences, all of which read as "this
app is broken".

This decision covers the guided setup that replaces that: what it asks for, how it knows
whether an answer arrived, and where it lives.

## Where it lives: the settings window, not a window of its own

Tauri makes a second window cheap in effort and expensive in surface: a Vite entry in
`vite.config.ts`, a `label` in `tauri.conf.json`, and a capability file naming that label,
or `listen`/`invoke` are denied inside it. That is three files' worth of permanent
configuration for six panels the user sees once.

The settings window is already the thing the backend opens on a first run, it already
holds every capability onboarding needs, and its stores already mirror the state the steps
render. So onboarding is a **takeover** of that window: `App.svelte` renders `Onboarding`
instead of the settings shell while setup is pending. The cost is one `{#if}` at the top
of `App.svelte`; the benefit is that every store, every command and every style token is
the one already in use.

## Knowing when setup is pending, without dragging existing users through it

The flag is `Settings::onboarding_completed`, and its default is **`true`**, which reads
backwards until you look at how the file is loaded. `Settings` is `#[serde(default)]`, so a
`settings.json` written by any earlier version simply has no such key and takes the field
from `Default`. A `false` default would therefore march every established user through a
tour of permissions they granted months ago, on the first launch after they updated.

The only reliable evidence of a genuinely fresh install is that **there is no settings file
at all**, which `setup` in `lib.rs` already computes as `first_run`. So that is the one
place the flag is ever set to `false`, and it is written to disk immediately, before
anything else happens, so that quitting halfway through setup resumes it on the next
launch instead of skipping it forever. The window is opened on `onboarding_pending` rather
than on `first_run` for the same reason.

Two tests hold the pair honest: `a_file_from_before_onboarding_is_treated_as_already_onboarded`
and `a_file_that_says_onboarding_is_pending_is_believed`.

## Permissions: three answers, not two

The obvious shape for "may we use the microphone" is a `bool`, and it is wrong. The
difference between *refused* and *never asked* is the difference between the two things
the UI can offer to do, and macOS makes that distinction load-bearing:

| Answer | What the app can offer |
|---|---|
| `Unasked` | `requestAccessForMediaType:` puts a system prompt on screen |
| `Denied` | nothing but a link to the settings pane, since the prompt is spent |
| `Granted` | nothing |
| `Unknown` | the platform will not say; try it and see |
| `NotRequired` | the platform does not gate this at all |

macOS shows its microphone prompt **exactly once per app**. After that,
`requestAccessForMediaType:` calls its completion handler straight back without putting
anything on screen, so a button wired to it unconditionally does nothing, visibly, for
precisely the user who most needs it to work. `platform::request_microphone_permission`
therefore raises the prompt only from `Unasked` and opens the settings pane otherwise, and
that fallback order lives in `platform/mod.rs` beside `resolve_active_monitor` rather than
in either backend.

Accessibility has no `Unasked`: `AXIsProcessTrusted` answers yes or no, and there is no
prompt to raise, which is why `request_insert_permission` has always opened the pane.

### Why macOS gets a real answer and Windows gets `Unknown`

macOS answers through `AVCaptureDevice.authorizationStatusForMediaType:`, reached with a
`#[link]` on AVFoundation and two `msg_send!`s in `platform/macos/permissions.rs`. cpal
already links AVFAudio out of the same umbrella, so no framework is added to the process
that was not already there, and `block2`, the one new dependency, is already in the tree
via cpal, pinned to match rather than to add a second copy.

Windows has no equivalent. A desktop app is covered by the single global "let desktop apps
access your microphone" switch rather than by an entry of its own, and there is no
supported API that answers for the running process. The consent store *is* readable out of
`HKCU\…\CapabilityAccessManager\ConsentStore\microphone\NonPackaged`, and reading it was
rejected: it would present a guess about a global switch as a fact about this app, on the
one platform where the guess cannot be checked from a development machine. `Unknown` is
the honest answer, and the microphone step falls back to the thing that is authoritative
everywhere: open the microphone and see whether anything arrives.

Windows also has no Accessibility permission (`INPUT_PERMISSION_REQUIRED = false`), so the
step is **removed** from the list rather than shown as satisfied. The list is filtered on
`accessibility === "not_required"`, the backend saying so, rather than on the platform
name, so the frontend keeps knowing nothing about which OS it is on.

### Polling, because the answer arrives in another application

Both permissions are granted in System Settings or in a system prompt. Nothing in this
process is told when that happens, so there is no event to subscribe to and
`stores/permissions.ts` polls every 1.5 s. It is a Svelte `readable`, so it polls only
while something is subscribed, which for a menu-bar app means only while its settings
window is open, and each tick is two cheap system calls. That is also what lets the
Accessibility step say "this page notices by itself when you flick the switch", and it
replaced a `setTimeout(check, 3000)` in `GeneralSettings` that guessed how long a trip to
System Settings takes.

## Nothing is ever mandatory

No step blocks the one after it, and the primary button is never disabled. Instead it
**changes what it says**: `Continue` when the step is satisfied, `Skip for now` when it is
not. A button reading "Continue" over a permission that was refused would quietly imply
the step succeeded; a disabled button would strand a user whose only other option is to
quit the app. Whatever was skipped is then listed once on the final panel, so it is stated
rather than discovered later as a hotkey that does nothing.

The download is the reason this matters most. It is several minutes on most connections,
and `stores/models.ts`, a store for exactly the reason `stores/update.ts` is one, keeps it
running across step changes, across a switch to Settings › Models, and across finishing
setup entirely.

## The cleanup model is offered, not enabled

Decision 0005 made refinement opt-in because it costs a second per dictation, half a
gigabyte resident, and can only ever be advisory. Onboarding does not weaken that: the
model is one step, clearly marked optional, and the *default* is still off.

What it does do is honour its own button. The button reads "Download and turn on", so a
download started from that step sets `refine_enabled` when it completes. A download
started from Settings › Models does not, because nobody said anything about switching it
on there. The step tracks whether the request came from itself rather than watching
installation state alone.

## Consequences

- `Settings` gains a field whose default is `true` and whose meaning depends on a comment.
  That is the price of not re-onboarding existing installs, and the two tests are what
  keep it from rotting.
- The `platform/` contract gains four items: `PermissionState`, `microphone_permission`,
  `request_microphone_permission`, and `input_permission_required`. Adding Linux now means
  answering four more questions in that directory, and none anywhere else.
- Windows onboarding is one step shorter and one step vaguer than macOS onboarding. Both
  differences are the platform's, and both are stated in the UI rather than papered over.
- `ModelSettings` moved onto the shared store, which fixed a bug nobody had reported:
  switching sections mid-download destroyed the progress bar and recreated it empty.
