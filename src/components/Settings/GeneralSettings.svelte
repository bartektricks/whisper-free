<script lang="ts">
  import { onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import Row from "../common/Row.svelte";
  import { settings } from "../../stores/settings";
  import { toAccelerator, formatAccelerator, toChord } from "../../lib/hotkey";
  import { trayName } from "../../lib/platform";
  import { permissions, requestAccessibility } from "../../stores/permissions";
  import type { OverlayAnchor, RecordingMode } from "../../types";

  /**
   * How long to wait after the first combination before deciding it was the
   * whole hotkey.
   *
   * A chord is typed as one gesture, so this only has to outlast the gap
   * between two deliberate presses. The captured combination is on screen
   * immediately either way — nothing visible is waiting on this.
   */
  const CHORD_WINDOW_MS = 900;

  /** `off` · `first` waiting for a combination · `second` waiting to see whether a chord follows. */
  let capturing = $state<"off" | "first" | "second">("off");
  let firstStep = $state<string | null>(null);
  let captureError = $state<string | null>(null);
  let captureButton = $state<HTMLButtonElement | null>(null);
  let chordTimer: ReturnType<typeof setTimeout> | null = null;

  // Polled rather than checked once: it is granted in System Settings, so the
  // app is not the thing that finds out, and the row has to stop nagging by
  // itself when the user comes back.
  const canInsert = $derived($permissions.accessibility !== "denied");

  /**
   * Capture listens on the window rather than on the button.
   *
   * WKWebView follows the macOS convention of *not* focusing a button when it
   * is clicked, so a `keydown` bound to the button never fires there and the
   * shortcut could never be changed. Focus is still requested explicitly, for
   * the focus ring and for keyboard users.
   */
  function startCapture() {
    if (capturing !== "off") {
      cancelCapture(); // a second click is how you back out with the mouse
      return;
    }
    captureError = null;
    capturing = "first";
    captureButton?.focus();
    // The live shortcut is claimed system-wide, so without this the one
    // combination the user most likely wants to re-record — the current one —
    // would start a dictation instead of reaching this recorder.
    void invoke("suspend_hotkey");
  }

  /** Leave capture without choosing anything, and give the hotkey back. */
  function cancelCapture() {
    resetCapture();
    void resumeHotkey();
  }

  function resetCapture() {
    clearChordTimer();
    capturing = "off";
    firstStep = null;
  }

  async function resumeHotkey() {
    try {
      await invoke("resume_hotkey");
    } catch (e) {
      // The app is now without a working shortcut, which the user cannot
      // discover any other way.
      captureError = String(e);
    }
  }

  /**
   * Leaving the window ends capture rather than leaving a global key trap armed
   * out of sight — but a combination already pressed is the user's answer, so
   * it is saved rather than dropped.
   */
  function onWindowBlur() {
    const pending = capturing === "second" ? firstStep : null;
    if (!pending) {
      cancelCapture();
      return;
    }
    resetCapture();
    void commit(pending);
  }

  function clearChordTimer() {
    if (chordTimer !== null) clearTimeout(chordTimer);
    chordTimer = null;
  }

  // Switching settings sections destroys this component. A timer outliving it
  // would save a hotkey for a pane the user has left, and a capture left open
  // would strand the app with no shortcut registered at all.
  onDestroy(() => {
    clearChordTimer();
    if (capturing !== "off") void resumeHotkey();
  });

  async function onCaptureKey(event: KeyboardEvent) {
    event.preventDefault();
    event.stopPropagation();

    // A held key repeats, and the listener is only detached on the next render.
    if (event.repeat) return;

    if (event.key === "Escape") {
      cancelCapture();
      return;
    }

    // A chord's second step may be a bare key — `⌘K K` is the shape people
    // expect — because it is only claimed while the chord window is open.
    const accelerator = toAccelerator(event, { bare: capturing === "second" });
    // Modifier-only presses land here while the user is still reaching for the
    // final key; keep listening rather than rejecting them.
    if (!accelerator) return;

    // The second combination of a chord.
    if (capturing === "second" && firstStep) {
      const chord = toChord(firstStep, accelerator);
      resetCapture();
      await commit(chord);
      return;
    }

    // The first, which may turn out to be the whole hotkey. It shows on the
    // button straight away; only the save waits to see whether a second
    // follows, so there is nothing to watch while the window runs.
    firstStep = accelerator;
    capturing = "second";
    chordTimer = setTimeout(() => {
      chordTimer = null;
      const single = firstStep;
      resetCapture();
      if (single) void commit(single);
    }, CHORD_WINDOW_MS);
  }

  async function commit(hotkey: string) {
    captureError = await settings.update({ hotkey });
    // Shown beside the button that caused it, so the window-wide banner would
    // only be the same sentence a second time.
    if (captureError) settings.error.set(null);

    // Last, and awaited, so this cannot race the registration `update` just
    // did. On a rejection it puts the previous shortcut back, since the
    // suspend that opened this capture released it.
    await resumeHotkey();
  }

  const captureLabel = $derived.by(() => {
    if (capturing === "first") return "Press a shortcut…";
    if (capturing === "second" && firstStep) {
      return `${formatAccelerator(firstStep)} …`;
    }
    return formatAccelerator($settings.hotkey);
  });

  const hotkeyHint = $derived.by(() => {
    if (capturing === "first") {
      return "Press the combination you want, or Esc to cancel.";
    }
    if (capturing === "second") {
      return "Press a second key now for a two-step chord, or pause to keep just this one.";
    }
    return "Works in any app. Needs at least one modifier, or a function key.";
  });

  const MODES: { value: RecordingMode; label: string; hint: string }[] = [
    {
      value: "hold_to_talk",
      label: "Hold to talk",
      hint: "Recording runs while the hotkey is held, and transcribes when you let go.",
    },
    {
      value: "toggle",
      label: "Toggle",
      hint: "Press once to start recording, press again to stop and transcribe.",
    },
  ];

  const activeHint = $derived(
    MODES.find((m) => m.value === $settings.recording_mode)?.hint ?? "",
  );

  /**
   * Laid out as they sit on screen: three rows of three, read left to right.
   * The overlay ignores the mouse entirely, so it cannot be dragged into place
   * — this grid is how it gets moved.
   */
  const ANCHORS: { value: OverlayAnchor; label: string }[] = [
    { value: "top_left", label: "Top left" },
    { value: "top_centre", label: "Top centre" },
    { value: "top_right", label: "Top right" },
    { value: "centre_left", label: "Centre left" },
    { value: "centre", label: "Centre" },
    { value: "centre_right", label: "Centre right" },
    { value: "bottom_left", label: "Bottom left" },
    { value: "bottom_centre", label: "Bottom centre" },
    { value: "bottom_right", label: "Bottom right" },
  ];
</script>

<!--
  While capturing, every key in the window belongs to the recorder; leaving the
  window ends it rather than leaving that trap armed out of sight.
-->
<svelte:window
  onkeydown={capturing === "off" ? undefined : onCaptureKey}
  onblur={capturing === "off" ? undefined : onWindowBlur}
/>

<section>
  <h2>General</h2>

  <Row label="Hotkey" hint={hotkeyHint}>
    <button
      type="button"
      class="capture"
      class:capturing={capturing !== "off"}
      bind:this={captureButton}
      onclick={startCapture}
    >
      {captureLabel}
    </button>
    {#if captureError}
      <p class="error">{captureError}</p>
    {/if}
  </Row>

  {#if !canInsert}
    <Row
      label="Permission"
      hint="Without it, transcriptions can be produced but not pasted into other apps."
    >
      <div class="permission">
        <span class="warn">Accessibility access not granted</span>
        <button type="button" onclick={requestAccessibility}>Open System Settings</button>
      </div>
    </Row>
  {/if}

  <Row
    label="Setup"
    hint="Walks through the permissions and the model download again. Nothing is reset; it only shows you where everything is."
  >
    <button type="button" onclick={() => settings.update({ onboarding_completed: false })}>
      Run setup again
    </button>
  </Row>

  <Row
    label="Startup"
    hint="Launches WhisperFree into the {trayName()} when you log in."
  >
    <label class="checkbox">
      <input
        type="checkbox"
        checked={$settings.start_at_login}
        onchange={(e) =>
          settings.update({ start_at_login: e.currentTarget.checked })}
      />
      Start at login
    </label>
  </Row>

  <Row label="Recording mode" hint={activeHint}>
    <div class="segmented" role="radiogroup" aria-label="Recording mode">
      {#each MODES as mode (mode.value)}
        <button
          type="button"
          role="radio"
          aria-checked={$settings.recording_mode === mode.value}
          class:selected={$settings.recording_mode === mode.value}
          onclick={() => settings.update({ recording_mode: mode.value })}
        >
          {mode.label}
        </button>
      {/each}
    </div>
  </Row>

  <Row
    label="Overlay"
    hint="A small indicator floats above whatever you are working in while dictation runs. It never takes focus, and clicks pass straight through it."
  >
    <label class="checkbox">
      <input
        type="checkbox"
        checked={$settings.show_overlay}
        onchange={(e) => settings.update({ show_overlay: e.currentTarget.checked })}
      />
      Show while dictating
    </label>

    <div
      class="anchors"
      role="radiogroup"
      aria-label="Overlay position"
      aria-disabled={!$settings.show_overlay}
    >
      {#each ANCHORS as anchor (anchor.value)}
        <button
          type="button"
          role="radio"
          title={anchor.label}
          aria-label={anchor.label}
          aria-checked={$settings.overlay_anchor === anchor.value}
          class:selected={$settings.overlay_anchor === anchor.value}
          disabled={!$settings.show_overlay}
          onclick={() => settings.update({ overlay_anchor: anchor.value })}
        >
          <span class="spot"></span>
        </button>
      {/each}
    </div>
  </Row>
</section>

<style>
  h2 {
    font-size: 15px;
    margin-bottom: 4px;
  }

  .segmented {
    display: inline-flex;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
  }

  .segmented button {
    border: none;
    border-radius: 0;
    background: var(--panel);
  }

  .segmented button + button {
    border-left: 1px solid var(--border);
  }

  .segmented button.selected {
    background: var(--accent);
    color: var(--accent-text);
  }

  .capture {
    min-width: 130px;
    font-variant-numeric: tabular-nums;
  }

  .capture.capturing {
    border-color: var(--accent);
    color: var(--accent);
  }

  .error {
    margin: 6px 0 0;
    font-size: 11px;
    color: var(--err);
    max-width: 42ch;
  }

  .permission {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  .warn {
    color: var(--warn);
    font-size: 12px;
  }

  .checkbox {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }

  /* A miniature of the screen: nine cells, positioned as the overlay will be. */
  .anchors {
    display: grid;
    width: fit-content;
    grid-template-columns: repeat(3, 26px);
    grid-template-rows: repeat(3, 20px);
    margin-top: 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
    background: var(--panel);
  }

  .anchors button {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    border: none;
    border-radius: 0;
    background: transparent;
  }

  .anchors button + button {
    border-left: 1px solid var(--border);
  }

  /* The fourth cell onwards starts a new row, which needs a top edge instead. */
  .anchors button:nth-child(3n + 1) {
    border-left: none;
  }

  .anchors button:nth-child(n + 4) {
    border-top: 1px solid var(--border);
  }

  .anchors button.selected {
    background: var(--accent);
  }

  .anchors button:disabled {
    opacity: 0.45;
  }

  .spot {
    width: 8px;
    height: 4px;
    border-radius: 2px;
    background: var(--text-dim);
  }

  .anchors button.selected .spot {
    background: var(--accent-text);
  }
</style>
