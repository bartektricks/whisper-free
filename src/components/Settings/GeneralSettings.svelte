<script lang="ts">
  import Row from "../common/Row.svelte";
  import { settings } from "../../stores/settings";
  import type { RecordingMode } from "../../types";

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
</style>
