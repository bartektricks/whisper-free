<script lang="ts">
  /**
   * Choosing the speech model, and fetching it.
   *
   * Every speech model is listed rather than just the default, because setup is
   * the moment the choice is cheapest: afterwards, switching means a second
   * download of up to a gigabyte. Each row downloads independently and shows
   * its own progress, so nothing is ever fetched out of sight of the person who
   * asked for it, and someone who starts one and then picks another can see
   * both.
   *
   * Settings › Speech lists only *installed* models, which is the opposite rule
   * and the right one there: you cannot dictate with a model you do not have.
   * Here nothing is installed yet, so choosing and downloading are one gesture.
   */
  import { settings } from "../../stores/settings";
  import { models } from "../../stores/models";
  import ModelDownload from "./ModelDownload.svelte";

  const speech = $derived($models.models.filter((m) => m.kind === "speech"));
  const chosen = $derived($models.models.find((m) => m.id === $settings.model_id));
  const installed = $derived(chosen?.installed ?? false);

  /**
   * The two halves of choosing a language, and no model has both: Parakeet
   * works it out and refuses to be told, Canary is told and cannot work it out.
   * Picking a Canary model therefore *requires* an answer here, and getting it
   * wrong is not an error the user would see: Canary handed the wrong source
   * language translates into it, fluently. Hence the warning below rather than
   * a silently sensible default.
   */
  const canDetect = $derived(
    chosen?.capabilities.includes("language_detection") ?? false,
  );
  const canPin = $derived(
    chosen?.capabilities.includes("language_selection") ?? false,
  );
  const sorted = $derived(
    [...(chosen?.languages ?? [])].sort((a, b) => a.name.localeCompare(b.name)),
  );
  const selectedLanguage = $derived(
    $settings.language.kind === "fixed" ? $settings.language.code : "auto",
  );

  /**
   * The backend normalises `language` whenever the model changes, so a
   * selection the new model cannot honour is repaired on the way in rather
   * than left to fail at the first dictation.
   */
  function choose(id: string) {
    void settings.update({ model_id: id });
  }

  function selectLanguage(event: Event) {
    const choice = (event.currentTarget as HTMLSelectElement).value;
    void settings.update({
      language: choice === "auto" ? { kind: "auto" } : { kind: "fixed", code: choice },
    });
  }
</script>

<h2>Choose the speech model</h2>
<p class="lead">
  This is the part that does the listening. None of them are bundled with the app,
  because nothing is downloaded until you ask, and once one is here dictation works with
  the network switched off entirely.
</p>

{#each speech as model (model.id)}
  <ModelDownload
    kind="speech"
    id={model.id}
    group="speech-model"
    selected={model.id === $settings.model_id}
    onSelect={() => choose(model.id)}
  />
{/each}

{#if canPin}
  <div class="language">
    <label for="onboarding-language">
      {canDetect ? "Language" : "Which language will you dictate in?"}
    </label>
    <select id="onboarding-language" value={selectedLanguage} onchange={selectLanguage}>
      {#if canDetect}
        <option value="auto">Automatic</option>
      {/if}
      {#each sorted as language (language.code)}
        <option value={language.code}>{language.name}</option>
      {/each}
    </select>
  </div>
  {#if !canDetect}
    <p class="warn">
      This model has to be told, and it does not check. Dictating in any other language
      produces confident, wrong words rather than an error, so it is worth getting right
      here. Settings › Speech can change it later.
    </p>
  {/if}
{/if}

{#if installed}
  <p class="hint">
    Ready. {chosen?.name} loads in the background whenever WhisperFree starts, which takes
    a second or so. You can download the others later from Settings › Models.
  </p>
{:else}
  <p class="hint">
    These are large files, so this takes a few minutes on most connections. Every file is
    checked against a pinned checksum before it is used, and an interrupted download
    resumes from what is already on disk.
  </p>
  <p class="hint">
    You can skip this and do it later in Settings › Models, but dictation cannot run
    until the model you picked is downloaded.
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

  .language {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 18px;
    font-size: 13px;
  }

  .hint {
    margin: 12px 0 0;
    font-size: 12px;
    color: var(--text-dim);
    max-width: 56ch;
  }

  .warn {
    margin: 10px 0 0;
    font-size: 12px;
    color: var(--text-dim);
    max-width: 58ch;
    border-left: 2px solid var(--border);
    padding-left: 12px;
  }
</style>
