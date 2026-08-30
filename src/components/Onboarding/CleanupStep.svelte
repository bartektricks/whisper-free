<script lang="ts">
  import { settings } from "../../stores/settings";
  import { models } from "../../stores/models";
  import ModelDownload from "./ModelDownload.svelte";

  const model = $derived($models.models.find((m) => m.id === $settings.refine_model_id));
  const installed = $derived(model?.installed ?? false);
  const downloading = $derived(model ? Boolean($models.progress[model.id]) : false);

  /**
   * Whether the download now running was started from this step.
   *
   * The button here says "download and turn on", so finishing the download is
   * the moment to honour the second half of that. Someone who downloaded the
   * model from Settings › Models earlier said nothing about switching it on,
   * and the switch stays theirs to make.
   */
  let requested = $state(false);

  $effect(() => {
    if (requested && installed && !$settings.refine_enabled) {
      requested = false;
      void settings.update({ refine_enabled: true });
    }
  });
</script>

<h2>Tidy up what you dictate?</h2>
<p class="lead">
  Optional, and off unless you say otherwise. A small language model reads each
  transcription and writes it out the way you would have typed it: fillers dropped, false
  starts resolved to whatever you settled on, and spoken numbers and dates written
  properly. English only. It runs on this machine like everything else, and only ever
  sees text that never left it.
</p>

<ModelDownload
  kind="refiner"
  id={$settings.refine_model_id}
  downloadLabel="Download and turn on"
  onDownload={() => (requested = true)}
/>

{#if $settings.refine_enabled}
  <p class="ok">Cleanup is on. Turn it off any time in Settings › Cleanup.</p>
{:else if installed}
  <label class="checkbox">
    <input
      type="checkbox"
      checked={$settings.refine_enabled}
      onchange={(e) => settings.update({ refine_enabled: e.currentTarget.checked })}
    />
    Clean up transcriptions before pasting them
  </label>
{:else if !downloading}
  <p class="hint">
    Skip this if you would rather not spend the disk or the extra half second. Settings ›
    Cleanup has it waiting whenever you change your mind.
  </p>
{/if}

<p class="note">
  A cleanup is only ever a suggestion. The result is measured against what you actually
  said, and if a word appears that you never spoke, the whole thing is thrown away and
  your own words are pasted instead. Your dictionary runs afterwards either way, so
  replacements you wrote by hand always win.
</p>

<style>
  h2 {
    font-size: 19px;
  }

  .lead {
    margin: 10px 0 0;
    max-width: 58ch;
    font-size: 13px;
    color: var(--text-dim);
  }

  .hint {
    margin: 12px 0 0;
    font-size: 12px;
    color: var(--text-dim);
    max-width: 56ch;
  }

  .ok {
    margin: 12px 0 0;
    font-size: 12px;
    color: var(--ok);
  }

  .checkbox {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    margin-top: 14px;
    font-size: 13px;
  }

  .note {
    margin: 20px 0 0;
    font-size: 12px;
    color: var(--text-dim);
    max-width: 58ch;
    border-left: 2px solid var(--border);
    padding-left: 12px;
  }
</style>
