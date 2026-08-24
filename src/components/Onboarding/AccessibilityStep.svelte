<script lang="ts">
  import { permissions, requestAccessibility } from "../../stores/permissions";
  import { accessibilitySettings } from "../../lib/platform";
  import PermissionVerdict from "./PermissionVerdict.svelte";

  const status = $derived($permissions.accessibility);
</script>

<h2>Let WhisperFree paste</h2>
<p class="lead">
  There are no per-app integrations here, and there never will be: WhisperFree puts the
  finished text on the clipboard and presses <kbd>⌘V</kbd> for you, in whatever app you
  were already typing in. macOS keeps that behind Accessibility, so it has to be granted
  once.
</p>

<PermissionVerdict
  {status}
  grantedLabel="Accessibility access granted"
  deniedLabel="Accessibility access not granted"
  actionLabel="Open System Settings"
  onAction={requestAccessibility}
/>

{#if status !== "granted"}
  <p class="hint">
    Switch WhisperFree on in {accessibilitySettings()}. macOS has no prompt for this, so
    the button opens the pane, and this page notices by itself once you flick the switch.
  </p>
  <p class="hint">
    Without it, dictation still transcribes: the text lands on your clipboard and you
    paste it yourself.
  </p>
{/if}

<style>
  h2 {
    font-size: 19px;
  }

  .lead {
    margin: 10px 0 0;
    max-width: 58ch;
    font-size: 13px;
    color: var(--text-dim);
  }

  kbd {
    font: inherit;
    font-weight: 600;
    border: 1px solid var(--border);
    border-bottom-width: 2px;
    border-radius: 5px;
    padding: 0 5px;
    background: var(--panel);
    color: var(--text);
  }

  .hint {
    margin: 12px 0 0;
    font-size: 12px;
    color: var(--text-dim);
    max-width: 56ch;
  }
</style>
