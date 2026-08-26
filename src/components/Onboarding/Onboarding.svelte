<script lang="ts">
  /**
   * First-run setup (decision 0007).
   *
   * A takeover of the settings window rather than a window of its own: the
   * backend already opens this window on a first run, and a second window
   * would need its own Vite entry, its own label and its own capability file
   * to render six panels that the user sees once.
   *
   * The steps are ordered by what blocks what. Permissions come first because
   * they are granted in another application and can be left running while the
   * model downloads; the model comes last of the real work because it is the
   * long one.
   */
  import { onMount } from "svelte";
  import { settings } from "../../stores/settings";
  import { permissions } from "../../stores/permissions";
  import { models } from "../../stores/models";
  import WelcomeStep from "./WelcomeStep.svelte";
  import MicrophoneStep from "./MicrophoneStep.svelte";
  import MutingStep from "./MutingStep.svelte";
  import AccessibilityStep from "./AccessibilityStep.svelte";
  import SpeechStep from "./SpeechStep.svelte";
  import CleanupStep from "./CleanupStep.svelte";
  import DoneStep from "./DoneStep.svelte";

  type StepId =
    | "welcome"
    | "microphone"
    | "muting"
    | "accessibility"
    | "speech"
    | "cleanup"
    | "done";

  /**
   * The accessibility step is dropped where the platform does not gate
   * synthetic input, because a step that asks for nothing is worse than no step.
   * `not_required` is the backend saying exactly that, so the list is derived
   * rather than switched on the platform name here.
   */
  const steps = $derived(
    (
      [
        { id: "welcome", label: "Welcome" },
        { id: "microphone", label: "Microphone" },
        { id: "muting", label: "Other audio" },
        { id: "accessibility", label: "Pasting" },
        { id: "speech", label: "Speech model" },
        { id: "cleanup", label: "Cleanup" },
        { id: "done", label: "Done" },
      ] as const
    ).filter(
      (step) =>
        step.id !== "accessibility" || $permissions.accessibility !== "not_required",
    ),
  );

  let index = $state(0);
  let finishing = $state(false);

  // The list shortens on Windows once the first permission poll lands, so the
  // cursor is clamped rather than trusted.
  const position = $derived(Math.min(index, steps.length - 1));
  const step = $derived<StepId>(steps[position]?.id ?? "welcome");
  const last = $derived(position === steps.length - 1);

  const speechModel = $derived($models.models.find((m) => m.id === $settings.model_id));
  const refineModel = $derived(
    $models.models.find((m) => m.id === $settings.refine_model_id),
  );
  const downloading = $derived(
    Boolean(speechModel && $models.progress[speechModel.id]) ||
      Boolean(refineModel && $models.progress[refineModel.id]),
  );

  /**
   * What the primary button offers.
   *
   * Nothing here is ever disabled: every step can be left unfinished, and the
   * last panel says what was left. A button that reads "Skip for now" is how
   * the user is told that, rather than one that reads "Continue" and quietly
   * moves on from something that did not happen.
   */
  const nextLabel = $derived.by(() => {
    if (last) return finishing ? "Finishing…" : "Finish setup";
    switch (step) {
      case "welcome":
        return "Get started";
      case "microphone":
        return $permissions.microphone === "denied" ? "Skip for now" : "Continue";
      // A preference rather than a task: it is answered whichever way the box
      // is left, so there is nothing here to skip.
      case "muting":
        return "Continue";
      case "accessibility":
        return $permissions.accessibility === "granted" ? "Continue" : "Skip for now";
      case "speech":
        if (speechModel && $models.progress[speechModel.id]) return "Continue";
        return speechModel?.installed ? "Continue" : "Skip for now";
      case "cleanup":
        return refineModel?.installed ? "Continue" : "Not now";
      default:
        return "Continue";
    }
  });

  async function next() {
    if (!last) {
      index = position + 1;
      return;
    }
    finishing = true;
    // The takeover is keyed off this flag, so a failure to persist it has to
    // leave the user somewhere rather than in a loop: the message lands in the
    // window-wide banner and the button becomes pressable again.
    const failure = await settings.update({ onboarding_completed: true });
    if (failure) finishing = false;
  }

  function back() {
    index = Math.max(0, position - 1);
  }

  onMount(() => {
    models.refresh().catch((e) => console.error("could not list models", e));
  });
</script>

<div class="onboarding">
  <header>
    <div class="brand">WhisperFree</div>
    <ol class="progress">
      {#each steps as item, i (item.id)}
        <li class:done={i < position} class:current={i === position}>
          <span class="dot"></span>
          <span class="label">{item.label}</span>
        </li>
      {/each}
    </ol>
  </header>

  <main>
    {#if step === "welcome"}
      <WelcomeStep />
    {:else if step === "microphone"}
      <MicrophoneStep />
    {:else if step === "muting"}
      <MutingStep />
    {:else if step === "accessibility"}
      <AccessibilityStep />
    {:else if step === "speech"}
      <SpeechStep />
    {:else if step === "cleanup"}
      <CleanupStep />
    {:else}
      <DoneStep />
    {/if}
  </main>

  <footer>
    <button type="button" onclick={back} disabled={position === 0}>Back</button>
    <span class="spacer">
      {#if last && downloading}
        A download is still running. It carries on after you finish here.
      {/if}
    </span>
    <button type="button" class="primary" onclick={next} disabled={finishing}>
      {nextLabel}
    </button>
  </footer>
</div>

<style>
  .onboarding {
    display: grid;
    grid-template-rows: auto 1fr auto;
    height: 100%;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 20px;
    padding: 14px 26px;
    border-bottom: 1px solid var(--border);
    background: var(--sidebar);
  }

  .brand {
    font-weight: 600;
    flex: none;
  }

  .progress {
    display: flex;
    align-items: center;
    gap: 14px;
    margin: 0;
    padding: 0;
    list-style: none;
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .progress li {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--text-dim);
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--border);
    flex: none;
  }

  .progress li.done .dot,
  .progress li.current .dot {
    background: var(--accent);
  }

  .progress li.current {
    color: var(--text);
    font-weight: 600;
  }

  /* Only the current step is named on a narrow window; the dots still track. */
  @media (max-width: 700px) {
    .progress li:not(.current) .label {
      display: none;
    }
  }

  main {
    padding: 26px;
    overflow-y: auto;
    min-height: 0;
  }

  footer {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 14px 26px;
    border-top: 1px solid var(--border);
    background: var(--sidebar);
  }

  .spacer {
    flex: 1;
    font-size: 11px;
    color: var(--text-dim);
    text-align: center;
  }
</style>
