<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import Row from "../common/Row.svelte";
  import { settings } from "../../stores/settings";
  import { microphoneSettings, systemName } from "../../lib/platform";
  import type { AudioDevice, MicrophoneTest } from "../../types";

  let devices = $state<AudioDevice[]>([]);
  let loadError = $state<string | null>(null);
  let testing = $state(false);
  let result = $state<MicrophoneTest | null>(null);
  let testError = $state<string | null>(null);

  async function loadDevices() {
    try {
      devices = await invoke<AudioDevice[]>("list_audio_devices");
      loadError = null;
    } catch (e) {
      loadError = String(e);
    }
  }

  async function startTest() {
    result = null;
    testError = null;
    try {
      await invoke("start_microphone_test");
      testing = true;
    } catch (e) {
      testError = String(e);
    }
  }

  async function stopTest() {
    testing = false;
    try {
      result = await invoke<MicrophoneTest>("stop_microphone_test");
    } catch (e) {
      testError = String(e);
    }
  }

  function selectDevice(event: Event) {
    const value = (event.currentTarget as HTMLSelectElement).value;
    settings.update({ input_device: value === "" ? null : value });
  }

  onMount(loadDevices);

  onDestroy(() => {
    // Never leave the microphone open because the user navigated away.
    if (testing) invoke("cancel_microphone_test").catch(() => {});
  });
</script>

<section>
  <h2>Audio</h2>

  <Row
    label="Microphone"
    hint="System default follows whatever {systemName()} is using, so it keeps working when you plug in headphones."
  >
    <select value={$settings.input_device ?? ""} onchange={selectDevice}>
      <option value="">System default</option>
      {#each devices as device (device.id)}
        <option value={device.id}>{device.name}</option>
      {/each}
    </select>
    {#if loadError}
      <p class="error">{loadError}</p>
    {/if}
  </Row>

  <Row
    label="Test"
    hint="Records a few seconds and reports the level it heard. Nothing is saved."
  >
    <div class="test">
      {#if testing}
        <button type="button" class="primary" onclick={stopTest}>Stop</button>
        <span class="live">Listening…</span>
      {:else}
        <button type="button" onclick={startTest}>Start test</button>
      {/if}
    </div>

    {#if testError}
      <p class="error">{testError}</p>
    {:else if result}
      {#if result.heard_audio}
        <p class="ok">
          Heard {(result.duration_ms / 1000).toFixed(1)}s of audio, peak level
          {Math.round(result.peak_level * 100)}%.
        </p>
      {:else}
        <p class="error">
          Recorded {(result.duration_ms / 1000).toFixed(1)}s but heard only silence. Check
          that the right microphone is selected and that LocalDictation is allowed in
          {microphoneSettings()}.
        </p>
      {/if}
    {/if}
  </Row>
</section>

<style>
  h2 {
    font-size: 15px;
    margin-bottom: 4px;
  }

  .test {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .live {
    font-size: 12px;
    color: var(--accent);
  }

  .ok,
  .error {
    margin: 6px 0 0;
    font-size: 11px;
    max-width: 46ch;
  }

  .ok {
    color: var(--ok);
  }

  .error {
    color: var(--err);
  }
</style>
