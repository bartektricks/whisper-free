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

  // The two halves of choosing a language, and no model has both: Parakeet
  // works the language out and refuses to be told, Canary is told and cannot
  // work it out. The backend normalises the saved selection when the model
  // changes, so this only has to render what the chosen model can honour.
  const canDetect = $derived(
    active?.capabilities.includes("language_detection") ?? false,
  );
  const canPin = $derived(
    active?.capabilities.includes("language_selection") ?? false,
  );

  const sorted = $derived(
    [...(active?.languages ?? [])].sort((a, b) => a.name.localeCompare(b.name)),
  );
  const selected = $derived(
    $settings.language.kind === "fixed" ? $settings.language.code : "auto",
  );

  onMount(async () => {
    models = await invoke<ModelInfo[]>("get_models");
  });

  function selectModel(event: Event) {
    settings.update({ model_id: (event.currentTarget as HTMLSelectElement).value });
  }

  function selectLanguage(event: Event) {
    const choice = (event.currentTarget as HTMLSelectElement).value;
    settings.update({
      language:
        choice === "auto" ? { kind: "auto" } : { kind: "fixed", code: choice },
    });
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
      {#if active}
        <p class="languages">{active.description}</p>
      {/if}
    {/if}
  </Row>

  {#if canPin}
    <Row
      label="Language"
      hint={canDetect
        ? "Leave it automatic, or pick one if you only ever dictate in a single language."
        : "This model has to be told which language it is listening to, so pick the one you speak."}
    >
      <select value={selected} onchange={selectLanguage}>
        {#if canDetect}
          <option value="auto">Automatic</option>
        {/if}
        {#each sorted as language (language.code)}
          <option value={language.code}>{language.name}</option>
        {/each}
      </select>
      {#if !canDetect}
        <p class="languages">
          Speaking anything else produces the wrong words rather than an error.
        </p>
      {/if}
    </Row>
  {:else}
    <Row
      label="Language"
      hint="This model detects the spoken language on its own and cannot be pinned to one, so there is nothing to choose."
    >
      <p class="value">Detected automatically</p>
      {#if active}
        <p class="languages">Recognises {summariseLanguages(active.languages)}.</p>
      {/if}
    </Row>
  {/if}
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
