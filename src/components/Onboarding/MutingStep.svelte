<script lang="ts">
  import { settings } from "../../stores/settings";
  import { systemName } from "../../lib/platform";

  const on = $derived($settings.mute_while_recording);
</script>

<h2>Hush everything else while you talk?</h2>
<p class="lead">
  Music and video playing while you dictate work against you twice over: they leak into the
  microphone and end up in the transcription, and they are hard to talk over. WhisperFree
  can silence them for the length of each recording and bring them straight back.
</p>

<label class="checkbox">
  <input
    type="checkbox"
    checked={on}
    onchange={(e) => settings.update({ mute_while_recording: e.currentTarget.checked })}
  />
  Mute while recording
</label>

<p class="hint">
  {#if on}
    On. Sound returns the moment you stop talking, before the text is even pasted.
  {:else}
    Off. Nothing is muted, and whatever is playing carries on into the recording.
  {/if}
  Settings › Audio has this either way.
</p>

<p class="note">
  No operating system lets one app mute another, so this mutes {systemName()} itself,
  which means a call you are dictating into goes quiet too. It puts back exactly what it
  found, and if you had already muted the machine yourself it leaves well alone.
</p>

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

  .checkbox {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    margin-top: 18px;
    font-size: 13px;
  }

  .hint {
    margin: 12px 0 0;
    font-size: 12px;
    color: var(--text-dim);
    max-width: 56ch;
  }

  .note {
    margin: 20px 0 0;
    font-size: 12px;
    color: var(--text-dim);
    max-width: 58ch;
    border-left: 2px solid var(--border);
    padding-left: 12px;
  }
</style>
