<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    label,
    hint,
    children,
  }: { label: string; hint?: string; children: Snippet } = $props();
</script>

<div class="row">
  <div class="label">{label}</div>
  <div class="control">
    {@render children()}
    {#if hint}
      <p class="hint">{hint}</p>
    {/if}
  </div>
</div>

<style>
  .row {
    display: grid;
    grid-template-columns: 150px 1fr;
    gap: 16px;
    align-items: baseline;
    padding: 10px 0;
  }

  .row + :global(.row) {
    border-top: 1px solid var(--border);
  }

  .label {
    color: var(--text-dim);
    text-align: right;
  }

  .hint {
    margin: 6px 0 0;
    font-size: 11px;
    color: var(--text-dim);
    max-width: 42ch;
  }
</style>
