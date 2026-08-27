# 0009 — Muting other audio while recording

**Status:** accepted
**Date:** 2026-08-26
**Applies to:** `platform/`, the new `mute/`, `publish_state` in `lib.rs`, Settings › Audio, onboarding

Dictating over music costs the user twice. The playback leaks into the microphone and
turns up in the transcription, and it is hard to talk over in the first place. The fix
everyone performs by hand is to pause Spotify, dictate, and start it again. This decision
covers doing it for them: system output goes quiet for exactly as long as the microphone
is open, and comes back exactly as it was.

## There is no way to mute other applications, only everything

The obvious shape is "mute the other apps", and neither platform offers it.

macOS has no per-application output volume at all; the concept does not exist outside
third-party kernel extensions and virtual audio drivers, and this app ships neither.
Windows comes closer through `IAudioSessionManager2`, which can enumerate the render
sessions on an endpoint and mute each one that is not ours. That was rejected: it would
make Windows behave differently from macOS for reasons the user cannot see, it misses
anything that starts playing after the recording begins, and it turns a two-call sequence
into session enumeration plus PID matching plus a per-session restore list.

So both backends do the same blunt thing: mute the **default output device**, and put it
back. The cost is stated plainly rather than hidden, in the settings hint and on the
onboarding panel: a call you are dictating into goes quiet too. That is also the whole
reason the feature is a setting instead of just being behaviour.

## Mute, not volume zero

Both are one property write, and both silence the machine. The difference is what happens
when the app dies while recording, because then nothing puts anything back.

A muted device is one tap of the volume-up key away from working again, and the key most
people reach for anyway. A device sitting at volume zero looks identical to a broken
output and takes a deliberate drag of a slider. So `kAudioDevicePropertyMute` and
`IAudioEndpointVolume::SetMute` are the first choice on both platforms, and the macOS
backend falls back to `kAudioDevicePropertyVolumeScalar` only for devices whose master
element has no settable mute. A device offering neither leaves the feature a logged no-op.

## Never unmute somebody who muted themselves

Both backends read the current state before writing, and return `None` when the output is
already silent. `None` means "there is nothing to put back", so the restore does nothing
too. Without that check, muting your own machine and then dictating would unmute it, which
is a worse failure than never having muted at all.

The device is also **remembered**, not looked up again on the way out. Plugging in
headphones mid-dictation changes which device is default, and restoring that one would
leave the muted device muted while moving a setting nobody touched.

## The restore hangs off `publish_state`, not off `audio.stop()`

This is the load-bearing choice. The obvious place to unmute is beside the call that ends
the recording, and it is wrong, because there is a path where that call never happens.

`dictation::cancel` gives up without touching the audio engine when it loses the
`finishing` gate, on the grounds that the pipeline is already past recording. But `finish`
checks `is_cancelled` *before* it calls `ctx.audio.stop()`. An Escape landing between
`claim_finish` and that check therefore reaches neither: `cancel` returns early because
the gate is held, and `finish` returns early because `cancelled` is set. The capture stream
is left open. That is a pre-existing bug, and it is exactly the shape of thing an unmute
must not depend on.

So `mute::apply` is called from `publish_state` in `lib.rs`, the fan-out that already
drives the tray status line and the overlay, and the rule is a pure function over the
snapshot:

```rust
pub const fn wants_silence(state: AppState, enabled: bool) -> bool {
    enabled && matches!(state, AppState::Recording)
}
```

That is deliberately the same shape as `overlay::wants_overlay`, and for the same reason:
the state machine already knows when a recording is happening, and every path out of one
publishes something. The leaky path above publishes `Ready`, so the sound comes back even
though the stream does not close. A stop failure publishes `Error`. A too-short recording
publishes `Ready`. None of them had to be enumerated.

The mute therefore lasts exactly the length of `AppState::Recording`. Transcription,
cleanup and the paste all run with sound back on, which is right: none of them can hear
anything.

## A thread, because COM interfaces belong to their apartment

`MuteEngine` owns the platform state on a thread of its own, in the shape
`audio/capture.rs` already established. The justification is the same one written at the
top of that file: `cpal::Stream` is `!Send` on macOS, and a Windows COM interface is bound
to the apartment that created it. One thread holds it, everything else sends messages.

The difference is that these messages carry **no reply channel**. `publish_state` runs on
whichever thread caused the transition, and that is frequently the hotkey handler, which
on macOS is the main thread. Nothing there may be made to wait on an audio device. The
channel is also what orders a silence against the restore before it, so the two cannot
race without a lock.

The one exception is `restore_blocking`, used only from the `RunEvent::Exit` arm in `run`.
Quitting mid-recording must not leave the machine silent, managed state is not reliably
dropped on the way out, and a fire-and-forget send would lose the race with process
teardown. Waiting there is the point.

That wait is bounded at two seconds, which is the one place this design admits a deadline.
The thread being waited on is mid-conversation with an audio device, and a driver that
never answers would otherwise leave the app unable to quit. Between the two ways it can
go wrong, output left muted is recoverable with the volume key and an app that will not
quit needs Force Quit, so the timeout resolves in favour of the outcome a crash already
produces. A thread that was never started does not reach the deadline at all: the reply
channel travels inside the undelivered command and is dropped with it, so the send fails
and the receive reports a disconnect immediately.

## The platform contract gains an opaque value

```rust
pub struct OutputMute(backend::output::Mute);
pub fn mute_system_output() -> Option<OutputMute>;
pub fn restore_system_output(mute: OutputMute);
```

The field's type lives in the private `backend` module, so application code can hold an
`OutputMute` and hand it back and do nothing else with it. What has to be remembered
differs by platform (a device id and a previous value on macOS, a live interface pointer
on Windows) and none of that has to become a shared vocabulary.

`OutputMute` restores on **drop**, and `restore_system_output` is the name to call that
by. A value dropped down some path nobody thought about still gives the user their sound
back, while call sites still read as intent rather than as a bare `drop`.

Muting is advisory in the sense `refine/` and `update/` are advisory. Nothing here touches
`AppState`, there is no error enum and no `user_message()`, and no failure can fail a
dictation. A device that will not go quiet is a debug line.

## On by default, unlike updates

`mute_while_recording` defaults to `true`, which means every existing install picks it up
at the next launch: `Settings` is `#[serde(default)]`, so a file written by 0.4.0 has no
such key and takes the field from `Default`.

That is the opposite of what `onboarding_completed` does, and deliberately so. Onboarding
defaults to "already done" because re-running it would waste an established user's time
over something they have already answered. Muting has nothing to re-ask: it is behaviour
that improves the transcription, it is visible the first time it happens, and it is one
checkbox in Settings › Audio to switch off. It is also unlike `check_for_updates`, which
is off by default because it is the only thing in the app that reaches the network. This
touches a volume, on this machine, that the user can see and undo.

Two tests hold the pair honest: `a_file_from_before_muting_gets_it_switched_on` and
`a_file_that_opts_out_of_muting_is_believed`.

`update_settings` also restores immediately when the setting is switched **off**, because
nothing else publishes a state until the run ends and the machine would otherwise stay
quiet until then. Switching it *on* mid-recording is not honoured: the run started under
the old answer, and un-muting is the direction that cannot surprise anyone.

## The microphone test does not mute

`commands::start_microphone_test` never enters `AppState::Recording`, so it falls outside
this design without needing an exception. That is the behaviour wanted: the test lives in
the settings window with the user watching it, it is about the input device, and silencing
their music because they pressed "Start test" would be a surprise.

## Consequences

- The `platform/` contract gains three items, so adding Linux now means answering one more
  question in that directory (PulseAudio or PipeWire sink mute) and nothing anywhere else.
- Two dependencies, both already in the tree and pinned to the version already resolved:
  `objc2-core-audio` via cpal, and `windows` via tauri, tao and wry. `windows-sys` cannot
  express a COM interface without hand-written vtable calls, which a two-call sequence
  does not justify, so this is the first place both Windows crates are used at once.
- A hard crash while recording leaves the machine muted. That is what the mute-over-volume
  choice above is for.
- Onboarding is one panel longer on both platforms. Unlike every other step it asks about
  a preference rather than a task, so its button always reads `Continue` and nothing about
  it can appear in `DoneStep`'s "Left for later" list.
- `cargo run --example mute_check` exists because none of this can be tested without
  changing a real device's state, which is not something `cargo test` should do to the
  machine running it. Only `wants_silence` and the settings default are unit-testable.
