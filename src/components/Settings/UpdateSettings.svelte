<script lang="ts">
  import { onMount } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import { settings } from "../../stores/settings";
  import {
    updateStatus,
    checkForUpdates,
    installUpdate,
    restartForUpdate,
    openReleaseNotes,
  } from "../../stores/update";
  import { formatBytes } from "../../lib/format";
  import { updateInstallNote } from "../../lib/platform";
  import Row from "../common/Row.svelte";

  let current = $state("");

  const status = $derived($updateStatus);
  const busy = $derived(
    status.phase === "checking" || status.phase === "downloading",
  );

  /**
   * The download bar's fill, 0–1.
   *
   * The server need not announce a length. Rather than invent a percentage,
   * the bar is left at zero and the caption says how much has arrived.
   */
  const fraction = $derived(
    status.total_bytes > 0 ? status.downloaded_bytes / status.total_bytes : 0,
  );

  onMount(async () => {
    current = await getVersion();
  });
</script>

<section>
  <h2>Updates</h2>
  <p class="intro">
    WhisperFree checks GitHub for a newer version and installs it in place. It is the only
    thing in the app that goes online apart from downloading a model, which is why the
    automatic check is off until you switch it on.
  </p>

  <Row label="This version" hint={updateInstallNote()}>
    <span class="version">{current || "…"}</span>
  </Row>

  <Row
    label="Check automatically"
    hint="Once a day, and shortly after WhisperFree starts. Nothing is downloaded without
      you asking."
  >
    <input
      type="checkbox"
      checked={$settings.check_for_updates}
      onchange={(e) => settings.update({ check_for_updates: e.currentTarget.checked })}
    />
  </Row>

  <div class="outcome">
    {#if status.phase === "available"}
      <p class="headline">WhisperFree {status.version} is available.</p>
      <div class="actions">
        <button type="button" class="primary" onclick={installUpdate}>
          Download and install
        </button>
        <!-- A link rather than the manifest's own notes field: `tauri-action`
             fills that with the workflow's release body, which for this
             project is install instructions rather than a changelog. -->
        {#if status.release_url}
          <button type="button" onclick={openReleaseNotes}>What's new</button>
        {/if}
      </div>
    {:else if status.phase === "downloading"}
      <p class="headline">Downloading WhisperFree {status.version}…</p>
      <div class="progress">
        <div class="bar"><div class="fill" style:width="{fraction * 100}%"></div></div>
        <p class="meta">
          {#if status.total_bytes > 0}
            {formatBytes(status.downloaded_bytes)} of {formatBytes(status.total_bytes)}
          {:else}
            {formatBytes(status.downloaded_bytes)} so far
          {/if}
        </p>
      </div>
    {:else if status.phase === "ready_to_restart"}
      <p class="headline">WhisperFree {status.version} is installed.</p>
      <div class="actions">
        <button type="button" class="primary" onclick={restartForUpdate}>
          Restart now
        </button>
      </div>
      <p class="meta">
        Nothing changes until you restart. Finish what you are dictating first — this
        closes the app.
      </p>
    {:else if status.phase === "up_to_date"}
      <p class="headline">WhisperFree is up to date.</p>
    {:else if status.phase === "failed"}
      <p class="error" role="alert">{status.message}</p>
    {/if}
  </div>

  <div class="actions">
    <button type="button" onclick={checkForUpdates} disabled={busy}>
      {status.phase === "checking" ? "Checking…" : "Check now"}
    </button>
  </div>
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

  .version {
    font-variant-numeric: tabular-nums;
  }

  .outcome:not(:empty) {
    margin-top: 16px;
  }

  .headline {
    margin: 0 0 8px;
    font-size: 13px;
  }

  .actions {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-top: 12px;
  }

  .progress {
    margin-top: 4px;
    max-width: 42ch;
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

  .meta {
    color: var(--text-dim);
    font-size: 11px;
    margin: 6px 0 0;
    max-width: 42ch;
  }

  .error {
    margin: 0;
    font-size: 12px;
    color: var(--err);
    max-width: 56ch;
  }
</style>
