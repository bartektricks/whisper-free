<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { settings } from "../../stores/settings";
  import { formatBytes } from "../../lib/format";
  import Row from "../common/Row.svelte";
  import type { ModelInfo } from "../../types";

  let models = $state<ModelInfo[]>([]);

  const refiner = $derived(
    models.find((m) => m.id === $settings.refine_model_id) ??
      models.find((m) => m.kind === "refiner"),
  );
  const installed = $derived(refiner?.installed ?? false);

  onMount(async () => {
    models = await invoke<ModelInfo[]>("get_models");
  });
</script>

<section>
  <h2>Cleanup</h2>
  <p class="intro">
    A small language model reads each transcription and fixes what the speech model
    misheard — run-together names, wrong homophones, missing punctuation. It runs on this
    machine like everything else, and it only ever sees text that never left it.
  </p>

  <Row
    label="Clean up transcriptions"
    hint={installed
      ? "Adds about a second before the text is pasted."
      : "Download the cleanup model in Settings › Models first."}
  >
    <input
      type="checkbox"
      checked={$settings.refine_enabled}
      disabled={!installed}
      onchange={(e) => settings.update({ refine_enabled: e.currentTarget.checked })}
    />
  </Row>

  {#if refiner}
    <Row label="Model" hint="{refiner.name} · {formatBytes(refiner.size_bytes)}">
      <span class="status" class:ok={installed}>
        {installed ? "Downloaded" : "Not installed"}
      </span>
    </Row>
  {/if}

  <p class="note">
    A cleanup is only ever a suggestion. If the model rewrites, translates, shortens or
    answers your words instead of correcting them, the change is thrown away and what you
    actually said is pasted. Your dictionary is applied afterwards either way, so the
    replacements you have written by hand always win.
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

  .note {
    color: var(--text-dim);
    font-size: 12px;
    margin: 16px 0 0;
    max-width: 56ch;
    border-left: 2px solid var(--border);
    padding-left: 10px;
  }

  .status {
    font-size: 12px;
    color: var(--text-dim);
  }

  .status.ok {
    color: var(--text);
  }
</style>
