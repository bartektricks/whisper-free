<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { settings } from "../../stores/settings";
  import { formatBytes } from "../../lib/format";
  import Row from "../common/Row.svelte";
  import type { ModelInfo, RefineStrength, RefineStyling } from "../../types";

  let models = $state<ModelInfo[]>([]);

  const refiner = $derived(
    models.find((m) => m.id === $settings.refine_model_id) ??
      models.find((m) => m.kind === "refiner"),
  );
  const installed = $derived(refiner?.installed ?? false);
  const on = $derived($settings.refine_enabled && installed);

  onMount(async () => {
    models = await invoke<ModelInfo[]>("get_models");
  });

  function selectStrength(e: Event & { currentTarget: HTMLSelectElement }) {
    settings.update({ refine_strength: e.currentTarget.value as RefineStrength });
  }

  function selectStyling(e: Event & { currentTarget: HTMLSelectElement }) {
    settings.update({ refine_styling: e.currentTarget.value as RefineStyling });
  }
</script>

<section>
  <h2>Cleanup</h2>
  <p class="intro">
    A small language model reads each transcription and writes it out the way you would
    have typed it: fillers dropped, false starts resolved to whatever you settled on, and
    spoken numbers, dates and email addresses written properly. It runs on this machine
    like everything else, and it only ever sees text that never left it.
  </p>

  <Row
    label="Clean up transcriptions"
    hint={installed
      ? "Adds about half a second before the text is pasted. English only."
      : "Download the cleanup model in Settings › Models first."}
  >
    <input
      type="checkbox"
      checked={$settings.refine_enabled}
      disabled={!installed}
      onchange={(e) => settings.update({ refine_enabled: e.currentTarget.checked })}
    />
  </Row>

  <Row
    label="How much to change"
    hint={$settings.refine_strength === "light_touch"
      ? "Punctuation, capitalisation and misheard words only. Your fillers stay."
      : "Everything above. Anything you did not say is still thrown away."}
  >
    <select value={$settings.refine_strength} onchange={selectStrength} disabled={!on}>
      <option value="full_cleanup">Full cleanup</option>
      <option value="light_touch">Light touch</option>
    </select>
  </Row>

  <Row label="Style" hint="How the cleaned-up text is written.">
    <select value={$settings.refine_styling} onchange={selectStyling} disabled={!on}>
      <option value="casual">Casual (lower case, relaxed)</option>
      <option value="semi_casual">Semi-casual (your phrasing, tidied)</option>
      <option value="semi_formal">Semi-formal (standard written English)</option>
      <option value="formal">Formal (contractions expanded)</option>
    </select>
  </Row>

  {#if refiner}
    <Row label="Model" hint="{refiner.name} · {formatBytes(refiner.size_bytes)}">
      <span class="status" class:ok={installed}>
        {installed ? "Downloaded" : "Not installed"}
      </span>
    </Row>
  {/if}

  <p class="note">
    A cleanup is only ever a suggestion. The result is measured against what you actually
    said, and if a word appears that you never spoke, the whole thing is thrown away and
    your own words are pasted instead. That covers the model answering you, translating
    you, or guessing at a name it did not know. Your dictionary is applied afterwards
    either way, so the replacements you have written by hand always win.
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
