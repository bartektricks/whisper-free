<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { appState } from "./stores/appState";
  import { settings } from "./stores/settings";
  import StatusBadge from "./components/common/StatusBadge.svelte";
  import GeneralSettings from "./components/Settings/GeneralSettings.svelte";
  import AudioSettings from "./components/Settings/AudioSettings.svelte";
  import SpeechSettings from "./components/Settings/SpeechSettings.svelte";
  import DictionarySettings from "./components/Settings/DictionarySettings.svelte";
  import ModelSettings from "./components/Settings/ModelSettings.svelte";
  import CleanupSettings from "./components/Settings/CleanupSettings.svelte";
  import UpdateSettings from "./components/Settings/UpdateSettings.svelte";
  import Onboarding from "./components/Onboarding/Onboarding.svelte";

  const settingsError = settings.error;
  const settingsLoaded = settings.loaded;

  const SECTIONS = [
    { id: "general", label: "General" },
    { id: "audio", label: "Audio" },
    { id: "speech", label: "Speech" },
    { id: "cleanup", label: "Cleanup" },
    { id: "dictionary", label: "Dictionary" },
    { id: "models", label: "Models" },
    { id: "updates", label: "Updates" },
  ] as const;

  type SectionId = (typeof SECTIONS)[number]["id"];
  let active = $state<SectionId>("general");

  function isSection(id: string): id is SectionId {
    return SECTIONS.some((section) => section.id === id);
  }

  onMount(() => {
    settings.load();

    // The tray can point at a section — it is how a menu-bar app says "the
    // thing I just told you about is over here". The window may have been
    // created moments ago, so the listener is attached before it is needed and
    // an unknown id is ignored rather than blanking the panel.
    const listener = listen<string>("show_section", (event) => {
      if (isSection(event.payload)) active = event.payload;
    });
    return () => {
      listener.then((unlisten) => unlisten()).catch(() => {});
    };
  });
</script>

<!--
  First-run setup takes the whole window (decision 0007). It waits for the
  settings to have actually loaded: the store starts on defaults, and the
  default is "already onboarded", so this neither flashes setup over an
  established install nor flashes settings over a fresh one.
-->
{#if $settingsLoaded && !$settings.onboarding_completed}
  <Onboarding />
{:else}
<div class="shell">
  <nav>
    <div class="brand">WhisperFree</div>
    <ul>
      {#each SECTIONS as section (section.id)}
        <li>
          <button
            type="button"
            class:selected={active === section.id}
            onclick={() => (active = section.id)}
          >
            {section.label}
          </button>
        </li>
      {/each}
    </ul>
  </nav>

  <main>
    <header>
      <StatusBadge snapshot={$appState} />
    </header>

    <div class="panel">
      {#if active === "general"}
        <GeneralSettings />
      {:else if active === "audio"}
        <AudioSettings />
      {:else if active === "speech"}
        <SpeechSettings />
      {:else if active === "cleanup"}
        <CleanupSettings />
      {:else if active === "dictionary"}
        <DictionarySettings />
      {:else if active === "models"}
        <ModelSettings />
      {:else if active === "updates"}
        <UpdateSettings />
      {/if}
    </div>

    {#if $settingsError}
      <p class="error" role="alert">{$settingsError}</p>
    {/if}
  </main>
</div>
{/if}

<style>
  .shell {
    display: grid;
    grid-template-columns: 180px 1fr;
    height: 100%;
  }

  nav {
    background: var(--sidebar);
    border-right: 1px solid var(--border);
    padding: 14px 10px;
  }

  .brand {
    font-weight: 600;
    padding: 0 8px 12px;
  }

  nav ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  nav button {
    width: 100%;
    text-align: left;
    border: none;
    background: none;
    border-radius: 6px;
    padding: 6px 8px;
  }

  nav button.selected {
    background: var(--accent);
    color: var(--accent-text);
  }

  main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    overflow-y: auto;
  }

  header {
    padding: 12px 22px;
    border-bottom: 1px solid var(--border);
  }

  .panel {
    padding: 18px 22px;
  }

  .error {
    margin: 0 22px 18px;
    color: var(--err);
    font-size: 12px;
  }
</style>
