<script lang="ts">
  /**
   * One model, with whatever it is currently doing: a download button, a
   * progress bar, or the fact that it is already here.
   *
   * Everything comes from the shared `models` store, so leaving onboarding, or
   * finishing it, does not interrupt a download that is running, and Settings ›
   * Models shows the same bar moving.
   */
  import { models } from "../../stores/models";
  import { formatBytes } from "../../lib/format";
  import type { ModelKind } from "../../types";

  let {
    kind,
    id,
    downloadLabel = "Download",
    onDownload,
    group,
    selected = false,
    onSelect,
  }: {
    kind: ModelKind;
    id?: string;
    downloadLabel?: string;
    /** Called when the download is started from here, not from anywhere else. */
    onDownload?: () => void;
    /**
     * Radio group name. Passing one turns the row into a choice as well as a
     * download, which is what the speech step needs: at setup nothing is
     * installed yet, so picking a model and fetching it are the same gesture.
     * Left off, the row is exactly what it was.
     */
    group?: string;
    selected?: boolean;
    onSelect?: () => void;
  } = $props();

  function start(modelId: string) {
    onDownload?.();
    void models.download(modelId);
  }

  const model = $derived(
    $models.models.find((m) => m.id === id) ?? $models.models.find((m) => m.kind === kind),
  );
  const active = $derived(model ? $models.progress[model.id] : undefined);
  const failure = $derived(model ? $models.errors[model.id] : undefined);
  const fraction = $derived(
    active && active.total_bytes > 0 ? active.downloaded_bytes / active.total_bytes : 0,
  );
</script>

{#if model}
  <article class="model" class:chosen={group && selected}>
    <div class="head">
      <div class="identity">
        {#if group}
          <!--
            A radio rather than a clickable card: the choice has to be reachable
            by keyboard, and the row also holds a button, so the whole thing
            cannot become one label.
          -->
          <input
            type="radio"
            name={group}
            id="choose-{model.id}"
            checked={selected}
            onchange={() => onSelect?.()}
          />
        {/if}
        <div>
          <h3>
            {#if group}
              <label for="choose-{model.id}">{model.name}</label>
            {:else}
              {model.name}
            {/if}
          </h3>
          <p class="meta">{model.description}</p>
        </div>
      </div>
      <div class="actions">
        {#if active}
          <button type="button" onclick={() => models.cancel(model.id)}>Cancel</button>
        {:else if model.installed}
          <span class="installed">✓ Downloaded</span>
        {:else}
          <button type="button" class="primary" onclick={() => start(model.id)}>
            {downloadLabel} · {formatBytes(model.size_bytes)}
          </button>
        {/if}
      </div>
    </div>

    {#if active}
      <div class="progress">
        <div class="bar"><div class="fill" style:width="{fraction * 100}%"></div></div>
        <p class="meta">
          {formatBytes(active.downloaded_bytes)} of {formatBytes(active.total_bytes)}
          {#if active.file}· {active.file}{/if}
        </p>
      </div>
      <p class="meta">
        You can carry on with setup while this runs. It keeps going in the background.
      </p>
    {:else if !model.installed && model.bytes_on_disk > 0}
      <p class="meta">
        {formatBytes(model.bytes_on_disk)} already downloaded. Starting again keeps the
        finished files.
      </p>
    {/if}

    {#if failure}
      <p class="error" role="alert">{failure}</p>
    {/if}
  </article>
{/if}

<style>
  .model {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 14px 16px;
    background: var(--panel);
    margin-top: 18px;
  }

  h3 {
    font-size: 13px;
  }

  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 14px;
  }

  .identity {
    display: flex;
    align-items: flex-start;
    gap: 10px;
  }

  .identity input {
    margin-top: 2px;
    flex: none;
  }

  .identity label {
    cursor: pointer;
  }

  .model.chosen {
    border-color: var(--accent);
  }

  .actions {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .installed {
    font-size: 12px;
    color: var(--ok);
  }

  .meta {
    color: var(--text-dim);
    font-size: 11px;
    margin: 5px 0 0;
    max-width: 52ch;
  }

  .progress {
    margin-top: 12px;
  }

  .bar {
    height: 5px;
    border-radius: 3px;
    background: var(--hover);
    overflow: hidden;
  }

  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.2s linear;
  }

  .error {
    margin: 8px 0 0;
    font-size: 11px;
    color: var(--err);
  }
</style>
