<script lang="ts">
  import { settings } from "../../stores/settings";
  import { models } from "../../stores/models";
  import ModelDownload from "./ModelDownload.svelte";

  const model = $derived($models.models.find((m) => m.id === $settings.model_id));
  const installed = $derived(model?.installed ?? false);
</script>

<h2>Download the speech model</h2>
<p class="lead">
  This is the part that does the listening. It is not bundled with the app, because
  nothing is downloaded until you ask, and once it is here dictation works with the
  network switched off entirely.
</p>

<ModelDownload kind="speech" id={$settings.model_id} />

{#if installed}
  <p class="hint">
    Ready. The model loads in the background whenever WhisperFree starts, which takes a
    second or so.
  </p>
{:else}
  <p class="hint">
    It is a large file, so this takes a few minutes on most connections. Every file is
    checked against a pinned checksum before it is used, and an interrupted download
    resumes from what is already on disk.
  </p>
  <p class="hint">
    You can skip this and do it later in Settings › Models, but dictation cannot run
    until it is downloaded.
  </p>
{/if}

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
</style>
