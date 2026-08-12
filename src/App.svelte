<script lang="ts">
  import { onMount } from "svelte";
  import { appState } from "./stores/appState";
  import { settings } from "./stores/settings";
  import StatusBadge from "./components/common/StatusBadge.svelte";
  import GeneralSettings from "./components/Settings/GeneralSettings.svelte";
  import AudioSettings from "./components/Settings/AudioSettings.svelte";

  const settingsError = settings.error;

  // Sections gain entries as their milestones land, so the UI never offers a
  // panel that does nothing.
  const SECTIONS = [
    { id: "general", label: "General" },
    { id: "audio", label: "Audio" },
  ] as const;

  type SectionId = (typeof SECTIONS)[number]["id"];
  let active = $state<SectionId>("general");

  onMount(() => {
    settings.load();
  });
</script>

<div class="shell">
  <nav>
    <div class="brand">LocalDictation</div>
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
      {/if}
    </div>

    {#if $settingsError}
      <p class="error" role="alert">{$settingsError}</p>
    {/if}
  </main>
</div>

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
