<script lang="ts">
  import { settings } from "../../stores/settings";
  import { formatAccelerator } from "../../lib/hotkey";
  import { trayName } from "../../lib/platform";

  const hotkey = $derived(formatAccelerator($settings.hotkey));
  const held = $derived($settings.recording_mode === "hold_to_talk");
</script>

<h2>Dictation that never leaves this machine</h2>
<p class="lead">
  {#if held}
    Hold <kbd>{hotkey}</kbd> anywhere, say what you want, and let go.
  {:else}
    Press <kbd>{hotkey}</kbd> anywhere, say what you want, and press it again.
  {/if}
  WhisperFree transcribes it and pastes it where your cursor already is.
</p>

<ul class="points">
  <li>
    <strong>Nothing is uploaded.</strong> Transcription runs on this machine, through a model
    you download in a moment. There is no account, no API and no telemetry.
  </li>
  <li>
    <strong>Nothing is written to disk.</strong> Audio is held in memory only for as long as
    it takes to transcribe it, and the logs record durations and event names, never what
    you said.
  </li>
  <li>
    <strong>It lives in the {trayName()}.</strong> There is no Dock icon and no window in your
    way; settings are always a click away from the icon up there.
  </li>
</ul>

<p class="note">
  Setup takes a minute: two permissions, then the speech model. You can leave and come
  back, and this picks up where you left off.
</p>

<style>
  h2 {
    font-size: 19px;
  }

  .lead {
    margin: 10px 0 0;
    max-width: 56ch;
    font-size: 13px;
  }

  kbd {
    font: inherit;
    font-weight: 600;
    border: 1px solid var(--border);
    border-bottom-width: 2px;
    border-radius: 5px;
    padding: 1px 6px;
    background: var(--panel);
    white-space: nowrap;
  }

  .points {
    margin: 20px 0 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 12px;
    max-width: 58ch;
  }

  .points li {
    font-size: 12px;
    color: var(--text-dim);
    border-left: 2px solid var(--border);
    padding-left: 12px;
  }

  .points strong {
    color: var(--text);
    font-weight: 600;
  }

  .note {
    margin: 22px 0 0;
    font-size: 12px;
    color: var(--text-dim);
    max-width: 56ch;
  }
</style>
