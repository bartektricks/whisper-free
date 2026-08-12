<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import Row from "../common/Row.svelte";
  import { settings } from "../../stores/settings";
  import { toAccelerator, formatAccelerator } from "../../lib/hotkey";
  import type { RecordingMode } from "../../types";

  let capturing = $state(false);
  let captureError = $state<string | null>(null);
  let canInsert = $state(true);

  async function checkPermission() {
    canInsert = await invoke<boolean>("can_insert_text");
  }

  async function grantPermission() {
    await invoke("request_insert_permission");
    // The user grants it in System Settings, so re-check when they come back.
    setTimeout(checkPermission, 3000);
  }

  onMount(checkPermission);

  function startCapture() {
    captureError = null;
    capturing = true;
  }

  async function onCaptureKey(event: KeyboardEvent) {
    event.preventDefault();
    event.stopPropagation();

    if (event.key === "Escape") {
      capturing = false;
      return;
    }

    const accelerator = toAccelerator(event);
    // Modifier-only presses land here while the user is still reaching for the
    // final key; keep listening rather than rejecting them.
    if (!accelerator) return;

    capturing = false;
    const previous = $settings.hotkey;
    await settings.update({ hotkey: accelerator });
    // The backend refuses shortcuts another app owns, so a value that did not
    // change means it was rejected.
    if ($settings.hotkey === previous && previous !== accelerator) {
      captureError = `${formatAccelerator(accelerator)} is already in use by another app.`;
    }
  }

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
</script>

<section>
  <h2>General</h2>

  <Row
    label="Hotkey"
    hint="Works in any app. Needs at least one modifier, or a function key."
  >
    <button
      type="button"
      class="capture"
      class:capturing
      onclick={startCapture}
      onkeydown={capturing ? onCaptureKey : undefined}
    >
      {capturing ? "Press a shortcut…" : formatAccelerator($settings.hotkey)}
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
        <button type="button" onclick={grantPermission}>Open System Settings</button>
      </div>
    </Row>
  {/if}

  <Row label="Startup" hint="Launches LocalDictation into the menu bar when you log in.">
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
</style>
