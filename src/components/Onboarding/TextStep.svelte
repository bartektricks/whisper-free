<script lang="ts">
  /**
   * What happens to your words after they are pasted (decision 0011).
   *
   * One panel for both settings, because they are one question asked over two
   * timescales: whether the text is still to hand a second later, and whether
   * it is still there next week. Like the muting step, it is a preference
   * rather than a task, so there is nothing here to skip.
   */
  import { settings } from "../../stores/settings";
  import { pasteShortcut } from "../../lib/platform";
  import type { HistoryRetention } from "../../types";

  const RETENTIONS: { value: HistoryRetention; label: string }[] = [
    { value: "session", label: "Until I quit WhisperFree" },
    { value: "one_day", label: "24 hours" },
    { value: "seven_days", label: "7 days" },
    { value: "thirty_days", label: "30 days" },
    { value: "forever", label: "Forever" },
  ];

  const keep = $derived($settings.keep_on_clipboard);
  const keeping = $derived($settings.history_enabled);
  const onDisk = $derived(keeping && $settings.history_retention !== "session");
</script>

<h2>What happens to your words?</h2>
<p class="lead">
  Dictated text goes straight into whatever you are typing in. WhisperFree borrows your
  clipboard to get it there and puts back what it found, so nothing you had copied is
  lost. Two things you can change about that.
</p>

<label class="checkbox">
  <input
    type="checkbox"
    checked={keep}
    onchange={(e) => settings.update({ keep_on_clipboard: e.currentTarget.checked })}
  />
  Leave the transcription on the clipboard
</label>

<p class="hint">
  {#if keep}
    On. {pasteShortcut()} pastes what you just said again, and whatever you had copied
    before is given up.
  {:else}
    Off. Your clipboard is put back exactly as you left it.
  {/if}
</p>

<label class="checkbox">
  <input
    type="checkbox"
    checked={keeping}
    onchange={(e) => settings.update({ history_enabled: e.currentTarget.checked })}
  />
  Keep a list of what I have dictated
</label>

{#if keeping}
  <label class="retention">
    Keep them for
    <select
      value={$settings.history_retention}
      onchange={(e) =>
        settings.update({
          history_retention: e.currentTarget.value as HistoryRetention,
        })}
    >
      {#each RETENTIONS as option (option.value)}
        <option value={option.value}>{option.label}</option>
      {/each}
    </select>
  </label>
{/if}

<p class="hint">
  {#if keeping}
    On. Settings › History lists them, newest first, and one click copies any of them
    back.
  {:else}
    Off. Nothing you dictate is recorded anywhere.
  {/if}
  Settings › History has both of these either way.
</p>

<p class="note">
  {#if onDisk}
    This is the only thing WhisperFree writes down that contains what you said. It is a
    plain file in the app's own folder, on this machine and nowhere else, and the 500 most
    recent are kept. Emptying it is one click, and switching this off deletes it.
  {:else}
    Nothing you say is written to disk. Audio never is either way: it is transcribed in
    memory and thrown away, and no part of this app talks to the internet unless you ask
    it to download a model or check for an update.
  {/if}
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

  .retention {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 12px;
    font-size: 12px;
    color: var(--text-dim);
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
