<script lang="ts">
  import { onMount } from "svelte";
  import { models } from "../../stores/models";
  import { settings } from "../../stores/settings";
  import { formatBytes, summariseLanguages } from "../../lib/format";
  import type { ModelInfo } from "../../types";

  // With one model of each kind this was obvious. With several it is not, and
  // Remove is right next to it.
  function inUse(model: ModelInfo): boolean {
    return model.kind === "speech"
      ? model.id === $settings.model_id
      : model.id === $settings.refine_model_id;
  }

  const speech = $derived($models.models.filter((m) => m.kind === "speech"));
  const refiners = $derived($models.models.filter((m) => m.kind === "refiner"));

  onMount(() => {
    models.refresh().catch((e) => console.error("could not list models", e));
  });
</script>

<section>
  <h2>Models</h2>
  <p class="intro">
    Models are downloaded on request, never automatically. Once installed, everything
    runs entirely on this machine.
  </p>

  {#snippet card(model: ModelInfo)}
    {@const active = $models.progress[model.id]}
    <article class="model">
      <div class="head">
        <div>
          <h3>
            {model.name}
            {#if model.installed && inUse(model)}
              <span class="badge">In use</span>
            {/if}
          </h3>
          <p class="meta">
            {#if model.languages.length > 0}
              {summariseLanguages(model.languages)} ·
            {/if}
            {formatBytes(model.size_bytes)}
          </p>
        </div>

        <div class="actions">
          {#if active}
            <button type="button" onclick={() => models.cancel(model.id)}>Cancel</button>
          {:else if model.installed}
            <span class="installed">Downloaded</span>
            <button type="button" onclick={() => models.remove(model.id)}>Remove</button>
          {:else}
            <button type="button" class="primary" onclick={() => models.download(model.id)}>
              Download
            </button>
          {/if}
        </div>
      </div>

      <p class="description">{model.description}</p>

      {#if active}
        {@const fraction =
          active.total_bytes > 0 ? active.downloaded_bytes / active.total_bytes : 0}
        <div class="progress">
          <div class="bar"><div class="fill" style:width="{fraction * 100}%"></div></div>
          <p class="meta">
            {formatBytes(active.downloaded_bytes)} of {formatBytes(active.total_bytes)}
            {#if active.file}· {active.file}{/if}
          </p>
        </div>
      {:else if !model.installed && model.bytes_on_disk > 0}
        <p class="meta">
          {formatBytes(model.bytes_on_disk)} already downloaded — starting again will keep
          the finished files.
        </p>
      {/if}

      {#if $models.errors[model.id]}
        <p class="error" role="alert">{$models.errors[model.id]}</p>
      {/if}
    </article>
  {/snippet}

  <h3 class="group">Speech</h3>
  {#each speech as model (model.id)}
    {@render card(model)}
  {/each}

  <h3 class="group">Cleanup</h3>
  <p class="intro">
    Optional. A small language model that checks a transcription over and fixes words the
    speech model misheard. Switch it on in Settings › Cleanup once it is downloaded.
  </p>
  {#each refiners as model (model.id)}
    {@render card(model)}
  {/each}
</section>

<style>
  h2 {
    font-size: 15px;
    margin-bottom: 4px;
  }

  .badge {
    margin-left: 6px;
    padding: 1px 6px;
    border-radius: 999px;
    font-size: 10px;
    font-weight: 500;
    vertical-align: middle;
    color: var(--text-dim);
    border: 1px solid var(--border);
  }

  h3 {
    font-size: 13px;
  }

  /* The section headings, distinguished from a model's own name. */
  h3.group {
    color: var(--text-dim);
    font-size: 11px;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    margin: 22px 0 8px;
  }

  h3.group:first-of-type {
    margin-top: 0;
  }

  .intro {
    color: var(--text-dim);
    font-size: 12px;
    margin: 0 0 14px;
    max-width: 56ch;
  }

  .model {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 12px 14px;
    background: var(--panel);
  }

  .model + .model {
    margin-top: 10px;
  }

  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }

  .installed {
    font-size: 12px;
    color: var(--ok);
  }

  .meta,
  .description {
    color: var(--text-dim);
    font-size: 11px;
    margin: 4px 0 0;
  }

  .description {
    font-size: 12px;
  }

  .progress {
    margin-top: 10px;
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
