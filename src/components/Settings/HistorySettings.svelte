<script lang="ts">
  /**
   * What happens to your words after they are pasted (decision 0011).
   *
   * Both halves of the panel answer the same question, which is why they share
   * a section and a single onboarding step: the clipboard toggle decides
   * whether the text is still to hand a second later, and the history decides
   * whether it is still to hand a week later.
   */
  import { onMount } from "svelte";
  import Row from "../common/Row.svelte";
  import { settings } from "../../stores/settings";
  import { history } from "../../stores/history";
  import { formatWhen } from "../../lib/format";
  import { pasteShortcut } from "../../lib/platform";
  import type { HistoryRetention } from "../../types";

  const RETENTIONS: { value: HistoryRetention; label: string }[] = [
    { value: "session", label: "Until I quit WhisperFree" },
    { value: "one_day", label: "24 hours" },
    { value: "seven_days", label: "7 days" },
    { value: "thirty_days", label: "30 days" },
    { value: "forever", label: "Forever" },
  ];

  const historyError = history.error;
  const enabled = $derived($settings.history_enabled);
  const onDisk = $derived(enabled && $settings.history_retention !== "session");

  /** Which entry was last copied, so the button can say it worked. */
  let copied = $state<number | null>(null);
  let confirmingClear = $state(false);
  let copiedTimer: ReturnType<typeof setTimeout> | null = null;

  onMount(() => {
    void history.load();
    return () => {
      if (copiedTimer) clearTimeout(copiedTimer);
    };
  });

  async function copy(id: number) {
    if (!(await history.copy(id))) return;
    copied = id;
    if (copiedTimer) clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => (copied = null), 1600);
  }

  async function clearAll() {
    if (!confirmingClear) {
      confirmingClear = true;
      return;
    }
    confirmingClear = false;
    await history.clear();
  }
</script>

<section>
  <h2>History</h2>
  <p class="intro">
    What happens to the words after they land in your document. Both of these are off
    until you turn them on, and neither sends anything anywhere.
  </p>

  <Row
    label="Keep on clipboard"
    hint={$settings.keep_on_clipboard
      ? `The transcription stays on the clipboard, so ${pasteShortcut()} pastes it again.`
      : "Whatever you had copied before is put back after the paste."}
  >
    <input
      type="checkbox"
      checked={$settings.keep_on_clipboard}
      onchange={(e) => settings.update({ keep_on_clipboard: e.currentTarget.checked })}
    />
  </Row>

  <Row
    label="Keep a history"
    hint={enabled
      ? "Every dictation is added to the list below."
      : "Nothing is recorded, and the list stays empty."}
  >
    <input
      type="checkbox"
      checked={enabled}
      onchange={(e) => settings.update({ history_enabled: e.currentTarget.checked })}
    />
  </Row>

  {#if enabled}
    <Row
      label="Keep them for"
      hint={onDisk
        ? "Older entries are removed automatically, here and on disk."
        : "Nothing is written to disk. The list is emptied when you quit."}
    >
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
    </Row>
  {/if}

  {#if enabled}
    <div class="list-head">
      <h3>{$history.length === 0 ? "Nothing yet" : `${$history.length} kept`}</h3>
      {#if $history.length > 0}
        <button type="button" class="danger" onclick={clearAll}>
          {confirmingClear ? "Really delete everything?" : "Delete all"}
        </button>
      {/if}
    </div>

    {#if $history.length === 0}
      <p class="empty">Your next dictation will appear here.</p>
    {:else}
      <ul class="entries">
        {#each $history as entry (entry.id)}
          <li>
            <p class="text">{entry.text}</p>
            <div class="meta">
              <span class="when">{formatWhen(entry.at)}</span>
              <button type="button" onclick={() => copy(entry.id)}>
                {copied === entry.id ? "Copied" : "Copy"}
              </button>
              <button type="button" onclick={() => history.remove(entry.id)}>Delete</button>
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}

  {#if $historyError}
    <p class="error" role="alert">{$historyError}</p>
  {/if}

  <p class="note">
    {#if onDisk}
      This is the one thing WhisperFree writes to disk that contains what you said. It
      lives in the app's own folder in plain text, it never leaves this machine, and the
      500 most recent are kept whichever window you choose. Turning this off, or choosing
      "Until I quit", deletes the file.
    {:else}
      Transcriptions are never written to disk unless you pick a window longer than
      "Until I quit". Everything stays on this machine either way.
    {/if}
  </p>
</section>

<style>
  h2 {
    font-size: 15px;
    margin-bottom: 4px;
  }

  .intro {
    color: var(--text-dim);
    font-size: 12px;
    margin: 0 0 14px;
    max-width: 56ch;
  }

  .list-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    margin: 20px 0 8px;
    border-top: 1px solid var(--border);
    padding-top: 14px;
  }

  h3 {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-dim);
    margin: 0;
  }

  .empty {
    color: var(--text-dim);
    font-size: 12px;
    margin: 0;
  }

  .entries {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .entries li {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 10px 12px;
  }

  .text {
    margin: 0;
    font-size: 12px;
    line-height: 1.45;
    /* Long dictations are common; the entry is a handle, not the document. */
    display: -webkit-box;
    -webkit-line-clamp: 4;
    line-clamp: 4;
    -webkit-box-orient: vertical;
    overflow: hidden;
    overflow-wrap: anywhere;
  }

  .meta {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 8px;
  }

  .when {
    font-size: 11px;
    color: var(--text-dim);
    margin-right: auto;
  }

  .meta button,
  .list-head button {
    font-size: 11px;
    padding: 3px 9px;
  }

  .danger {
    color: var(--danger, #c0392b);
  }

  .error {
    color: var(--danger, #c0392b);
    font-size: 12px;
    margin: 12px 0 0;
  }

  .note {
    color: var(--text-dim);
    font-size: 12px;
    margin: 16px 0 0;
    max-width: 56ch;
    border-left: 2px solid var(--border);
    padding-left: 10px;
  }
</style>
