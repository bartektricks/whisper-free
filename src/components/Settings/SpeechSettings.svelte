<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import Row from "../common/Row.svelte";
  import { settings } from "../../stores/settings";
  import { summariseLanguages } from "../../lib/format";
  import type { ModelInfo } from "../../types";

  let models = $state<ModelInfo[]>([]);

  // Speech models only: a cleanup model cannot transcribe, and offering it
  // here would let a user pick one and break dictation.
  const installed = $derived(
    models.filter((m) => m.installed && m.kind === "speech"),
  );
  const active = $derived(models.find((m) => m.id === $settings.model_id));

  onMount(async () => {
    models = await invoke<ModelInfo[]>("get_models");
  });

  function selectModel(event: Event) {
    settings.update({ model_id: (event.currentTarget as HTMLSelectElement).value });
  }
</script>

<section>
  <h2>Speech</h2>

  <Row label="Model" hint="Install models in Settings › Models.">
    {#if installed.length === 0}
      <p class="none">No model installed yet.</p>
    {:else}
      <select value={$settings.model_id} onchange={selectModel}>
        {#each installed as model (model.id)}
          <option value={model.id}>{model.name}</option>
        {/each}
      </select>
    {/if}
  </Row>

  <Row
    label="Language"
    hint="This model detects the spoken language on its own and cannot be pinned to one, so there is nothing to choose."
  >
    <p class="value">Detected automatically</p>
    {#if active}
      <p class="languages">Recognises {summariseLanguages(active.languages)}.</p>
    {/if}
  </Row>
</section>

<style>
  h2 {
    font-size: 15px;
    margin-bottom: 4px;
  }

  .value {
    margin: 0;
  }

  .none,
  .languages {
    margin: 4px 0 0;
    font-size: 11px;
    color: var(--text-dim);
    max-width: 46ch;
  }

  .none {
    font-size: 13px;
    margin: 0;
  }
</style>
