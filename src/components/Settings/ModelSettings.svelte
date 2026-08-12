<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { formatBytes, summariseLanguages } from "../../lib/format";
  import type { ModelInfo, DownloadProgress, DownloadFailure } from "../../types";

  let models = $state<ModelInfo[]>([]);
  let progress = $state<Record<string, DownloadProgress>>({});
  let errors = $state<Record<string, string>>({});
  let unlisteners: UnlistenFn[] = [];

  async function refresh() {
    models = await invoke<ModelInfo[]>("get_models");
  }

  async function download(id: string) {
    errors = { ...errors, [id]: "" };
    progress = {
      ...progress,
      [id]: { model_id: id, file: "", downloaded_bytes: 0, total_bytes: 0 },
    };
    try {
      await invoke("download_model", { modelId: id });
    } catch (e) {
      errors = { ...errors, [id]: String(e) };
      const { [id]: _dropped, ...rest } = progress;
      progress = rest;
    }
  }

  async function cancel(id: string) {
    await invoke("cancel_model_download", { modelId: id });
  }

  async function remove(id: string) {
    await invoke("delete_model", { modelId: id });
    await refresh();
  }

  onMount(async () => {
    await refresh();

    unlisteners.push(
      await listen<DownloadProgress>("model_download_progress", (event) => {
        progress = { ...progress, [event.payload.model_id]: event.payload };
      }),
    );
    unlisteners.push(
      await listen<string>("model_download_completed", async (event) => {
        const { [event.payload]: _done, ...rest } = progress;
        progress = rest;
        await refresh();
      }),
    );
    unlisteners.push(
      await listen<DownloadFailure>("model_download_failed", async (event) => {
        const { [event.payload.model_id]: _failed, ...rest } = progress;
        progress = rest;
        errors = { ...errors, [event.payload.model_id]: event.payload.message };
        await refresh();
      }),
    );
  });

  onDestroy(() => unlisteners.forEach((stop) => stop()));
</script>

<section>
  <h2>Models</h2>
  <p class="intro">
    Speech models are downloaded on request, never automatically. Once installed,
    transcription runs entirely on this Mac.
  </p>

  {#each models as model (model.id)}
    {@const active = progress[model.id]}
    <article class="model">
      <div class="head">
        <div>
          <h3>{model.name}</h3>
          <p class="meta">
            {summariseLanguages(model.languages)} · {formatBytes(model.size_bytes)}
          </p>
        </div>

        <div class="actions">
          {#if active}
            <button type="button" onclick={() => cancel(model.id)}>Cancel</button>
          {:else if model.installed}
            <span class="installed">Downloaded</span>
            <button type="button" onclick={() => remove(model.id)}>Remove</button>
          {:else}
            <button type="button" class="primary" onclick={() => download(model.id)}>
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

      {#if errors[model.id]}
        <p class="error" role="alert">{errors[model.id]}</p>
      {/if}
    </article>
  {/each}
</section>

<style>
  h2 {
    font-size: 15px;
    margin-bottom: 4px;
  }

  h3 {
    font-size: 13px;
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
