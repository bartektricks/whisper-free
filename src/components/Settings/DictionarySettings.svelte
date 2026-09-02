<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import type { DictionaryEntry } from "../../types";

  let entries = $state<DictionaryEntry[]>([]);
  let error = $state<string | null>(null);

  let newInput = $state("");
  let newAliases = $state("");
  let newReplacement = $state("");
  let editingId = $state<number | null>(null);
  let editInput = $state("");
  let editAliases = $state("");
  let editReplacement = $state("");

  /** The "also heard as" field is comma-separated; the backend drops blanks. */
  const parseAliases = (raw: string) =>
    raw.split(",").map((a) => a.trim()).filter((a) => a.length > 0);

  const formatAliases = (aliases: string[]) => aliases.join(", ");

  async function run(action: () => Promise<DictionaryEntry[]>) {
    try {
      entries = await action();
      error = null;
    } catch (e) {
      error = String(e);
    }
  }

  const load = () =>
    run(() => invoke<DictionaryEntry[]>("get_dictionary"));

  async function add(event: Event) {
    event.preventDefault();
    if (!newInput.trim()) return;
    await run(() =>
      invoke<DictionaryEntry[]>("add_dictionary_entry", {
        input: newInput,
        aliases: parseAliases(newAliases),
        replacement: newReplacement,
      }),
    );
    if (!error) {
      newInput = "";
      newAliases = "";
      newReplacement = "";
    }
  }

  function startEdit(entry: DictionaryEntry) {
    editingId = entry.id;
    editInput = entry.input;
    editAliases = formatAliases(entry.aliases);
    editReplacement = entry.replacement;
  }

  async function saveEdit(entry: DictionaryEntry) {
    await run(() =>
      invoke<DictionaryEntry[]>("update_dictionary_entry", {
        id: entry.id,
        input: editInput,
        aliases: parseAliases(editAliases),
        replacement: editReplacement,
        enabled: entry.enabled,
      }),
    );
    if (!error) editingId = null;
  }

  const toggle = (entry: DictionaryEntry) =>
    run(() =>
      invoke<DictionaryEntry[]>("update_dictionary_entry", {
        id: entry.id,
        input: entry.input,
        aliases: entry.aliases,
        replacement: entry.replacement,
        enabled: !entry.enabled,
      }),
    );

  const remove = (entry: DictionaryEntry) =>
    run(() => invoke<DictionaryEntry[]>("delete_dictionary_entry", { id: entry.id }));

  onMount(load);
</script>

<section>
  <h2>Dictionary</h2>
  <p class="intro">
    Replacements applied to every transcription before it is inserted. Matching
    ignores case and only fires on whole words, so a rule for "go" will not touch
    "google". If the model mishears a word more than one way, list the other
    forms under "also heard as", separated by commas, and they all become the
    same replacement.
  </p>

  <form class="add" onsubmit={add}>
    <input
      type="text"
      bind:value={newInput}
      placeholder="Recognised text"
      aria-label="Recognised text"
    />
    <span class="arrow" aria-hidden="true">→</span>
    <input
      type="text"
      bind:value={newReplacement}
      placeholder="Replacement"
      aria-label="Replacement"
    />
    <button type="submit" class="primary" disabled={!newInput.trim()}>Add entry</button>
    <input
      class="aliases"
      type="text"
      bind:value={newAliases}
      placeholder="Also heard as (optional, comma separated)"
      aria-label="Also heard as"
    />
  </form>

  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}

  {#if entries.length === 0}
    <p class="empty">
      No entries yet. Add one when the model consistently mishears a word — a name,
      a library, a piece of jargon.
    </p>
  {:else}
    <table>
      <thead>
        <tr>
          <th class="toggle-col"><span class="sr-only">Enabled</span></th>
          <th>Recognised text</th>
          <th>Also heard as</th>
          <th>Replacement</th>
          <th class="actions-col"><span class="sr-only">Actions</span></th>
        </tr>
      </thead>
      <tbody>
        {#each entries as entry (entry.id)}
          <tr class:disabled={!entry.enabled}>
            <td>
              <input
                type="checkbox"
                checked={entry.enabled}
                onchange={() => toggle(entry)}
                aria-label="Enable {entry.input}"
              />
            </td>
            {#if editingId === entry.id}
              <td><input type="text" bind:value={editInput} /></td>
              <td>
                <input
                  type="text"
                  bind:value={editAliases}
                  placeholder="Comma separated"
                  aria-label="Also heard as"
                />
              </td>
              <td><input type="text" bind:value={editReplacement} /></td>
              <td class="actions-col">
                <button type="button" onclick={() => saveEdit(entry)}>Save</button>
                <button type="button" onclick={() => (editingId = null)}>Cancel</button>
              </td>
            {:else}
              <td>{entry.input}</td>
              <td class="aliases-col">
                {#if entry.aliases.length > 0}{formatAliases(entry.aliases)}{:else}<span
                    class="none">—</span
                  >{/if}
              </td>
              <td>{entry.replacement}</td>
              <td class="actions-col">
                <button type="button" onclick={() => startEdit(entry)}>Edit</button>
                <button type="button" onclick={() => remove(entry)}>Delete</button>
              </td>
            {/if}
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</section>

<style>
  h2 {
    font-size: 15px;
    margin-bottom: 4px;
  }

  .intro,
  .empty {
    color: var(--text-dim);
    font-size: 12px;
    margin: 0 0 14px;
    max-width: 56ch;
  }

  .add {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-bottom: 14px;
  }

  .add input {
    flex: 1;
    min-width: 0;
  }

  /* Its own line: four controls abreast leaves none of them readable. */
  .add .aliases {
    flex-basis: 100%;
  }

  .aliases-col {
    color: var(--text-dim);
  }

  .none {
    opacity: 0.6;
  }

  .arrow {
    color: var(--text-dim);
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
  }

  th {
    text-align: left;
    font-weight: 500;
    color: var(--text-dim);
    border-bottom: 1px solid var(--border);
    padding: 6px 8px;
  }

  td {
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
  }

  td input[type="text"] {
    width: 100%;
  }

  tr.disabled td:not(.actions-col) {
    opacity: 0.45;
    text-decoration: line-through;
  }

  .toggle-col {
    width: 28px;
  }

  .actions-col {
    width: 1%;
    white-space: nowrap;
    text-align: right;
  }

  .actions-col button + button {
    margin-left: 6px;
  }

  .error {
    color: var(--err);
    font-size: 11px;
    margin: 0 0 10px;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
