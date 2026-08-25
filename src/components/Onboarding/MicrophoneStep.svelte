<script lang="ts">
  import { onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { permissions, requestMicrophone } from "../../stores/permissions";
  import { microphoneSettings, systemName } from "../../lib/platform";
  import PermissionVerdict from "./PermissionVerdict.svelte";
  import type { MicrophoneTest } from "../../types";

  /** How long the take runs for. Long enough to say a sentence into. */
  const TEST_MS = 3000;

  let testing = $state(false);
  let result = $state<MicrophoneTest | null>(null);
  let testError = $state<string | null>(null);
  let timer: ReturnType<typeof setTimeout> | null = null;

  const status = $derived($permissions.microphone);

  /**
   * The backend calls anything above digital silence "heard", because that is
   * what separates a working microphone from a refused one. It is not the same
   * question as whether the level is any good: a quiet room with no speech in
   * it clears that bar and still rounds to nothing.
   */
  const FAINT_BELOW = 0.05;
  const peakLabel = $derived(
    result && result.peak_level >= 0.01
      ? `${Math.round(result.peak_level * 100)}%`
      : "under 1%",
  );

  async function runTest() {
    result = null;
    testError = null;
    try {
      await invoke("start_microphone_test");
    } catch (e) {
      testError = String(e);
      return;
    }
    testing = true;
    // Self-timing rather than a second button: the one thing being asked here
    // is "does anything arrive", and three seconds answers it.
    timer = setTimeout(async () => {
      timer = null;
      testing = false;
      try {
        result = await invoke<MicrophoneTest>("stop_microphone_test");
      } catch (e) {
        testError = String(e);
      }
    }, TEST_MS);
  }

  // Never leave the microphone open because the step was left, or finished.
  onDestroy(() => {
    if (timer !== null) clearTimeout(timer);
    if (testing) invoke("cancel_microphone_test").catch(() => {});
  });
</script>

<h2>Let WhisperFree hear you</h2>
<p class="lead">
  Dictation needs the microphone. What it captures is held in memory for the second or two
  it takes to transcribe, and is never written to disk or sent anywhere.
</p>

<PermissionVerdict
  {status}
  grantedLabel="Microphone access granted"
  deniedLabel={status === "unasked"
    ? "Microphone access not granted yet"
    : "Microphone access was refused"}
  actionLabel={status === "unasked" ? "Allow microphone" : "Open microphone settings"}
  unknownLabel="{systemName()} does not say in advance whether an app may use the microphone. The test below is how to find out."
  onAction={requestMicrophone}
/>

{#if status === "denied"}
  <p class="hint">
    Find WhisperFree in {microphoneSettings()} and switch it on. This page notices by
    itself when you do.
  </p>
{/if}

<div class="test">
  <button type="button" onclick={runTest} disabled={testing}>
    {testing ? "Listening…" : "Test microphone"}
  </button>
  <span class="test-hint">
    {#if testing}
      Say something. This is listening for {TEST_MS / 1000} seconds.
    {:else}
      Records {TEST_MS / 1000} seconds and reports the level it heard. Nothing is saved.
    {/if}
  </span>
</div>

{#if testError}
  <p class="error" role="alert">{testError}</p>
{:else if result?.heard_audio && result.peak_level >= FAINT_BELOW}
  <p class="ok">Heard you clearly, at a peak level of {peakLabel}.</p>
{:else if result?.heard_audio}
  <p class="faint">
    Heard something, but only just: peak level {peakLabel}. Speak up, or check that the
    right microphone is selected in Settings › Audio.
  </p>
{:else if result}
  <p class="error" role="alert">
    Recorded {(result.duration_ms / 1000).toFixed(1)}s of silence. Check that the right
    microphone is selected in Settings › Audio, and that WhisperFree is allowed in
    {microphoneSettings()}.
  </p>
{/if}

<style>
  h2 {
    font-size: 19px;
  }

  .lead {
    margin: 10px 0 0;
    max-width: 56ch;
    font-size: 13px;
    color: var(--text-dim);
  }

  .hint {
    margin: 10px 0 0;
    font-size: 12px;
    color: var(--text-dim);
    max-width: 56ch;
  }

  .test {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    margin-top: 24px;
    padding-top: 18px;
    border-top: 1px solid var(--border);
  }

  .test-hint {
    font-size: 11px;
    color: var(--text-dim);
    max-width: 40ch;
  }

  .ok {
    margin: 12px 0 0;
    font-size: 12px;
    color: var(--ok);
  }

  .faint {
    margin: 12px 0 0;
    font-size: 12px;
    color: var(--warn);
    max-width: 56ch;
  }

  .error {
    margin: 12px 0 0;
    font-size: 12px;
    color: var(--err);
    max-width: 56ch;
  }
</style>
