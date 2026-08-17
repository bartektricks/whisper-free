<script lang="ts">
  import { appState, stateLabel } from "../stores/appState";

  /**
   * The window is shown and hidden by Rust (`overlay::apply`), so this
   * component never decides whether it is on screen — only what is drawn when
   * it is. It renders whatever the last `state_changed` said, which is why the
   * backend emits before it shows the window.
   */
  const isError = $derived($appState.state === "error");

  /** Bars bounce while the microphone is open, and settle once it is not. */
  const isListening = $derived($appState.state === "recording");

  const BARS = [0, 1, 2, 3];
</script>

{#key $appState.state}
  <div class="pill" class:error={isError} class:listening={isListening}>
    {#if isError}
      <span class="glyph" aria-hidden="true">⚠</span>
    {:else}
      <span class="bars" aria-hidden="true">
        {#each BARS as bar (bar)}
          <span class="bar" style="--bar: {bar}"></span>
        {/each}
      </span>
    {/if}
    <span class="label">{stateLabel($appState)}</span>
  </div>
{/key}

<style>
  .pill {
    display: flex;
    align-items: center;
    gap: 10px;
    /* Fills the padding box `#overlay` leaves for it — see overlay.css. */
    flex: 1;
    min-width: 0;
    padding: 0 14px;
    border: 1px solid var(--pill-border);
    border-radius: 999px;
    background: var(--pill);
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.35);
    /* Covers the few milliseconds between the window being ordered front and
       the webview painting the state it was just handed. */
    animation: appear 140ms ease-out;
  }

  /* An error is a sentence, not a word, so it needs a box rather than a pill. */
  .pill.error {
    border-radius: 14px;
    padding: 0 16px;
    align-items: center;
  }

  .label {
    flex: 1;
    min-width: 0;
    font-size: 13px;
    color: var(--pill-text);
    /* Three lines is all the error window has room for; the tray menu keeps
       the full message. */
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .pill.error .label {
    font-size: 12px;
    color: var(--err);
  }

  .glyph {
    font-size: 15px;
    color: var(--err);
    flex: none;
  }

  .bars {
    display: flex;
    align-items: center;
    gap: 3px;
    height: 18px;
    flex: none;
  }

  .bar {
    width: 3px;
    height: 6px;
    border-radius: 2px;
    background: var(--accent);
    /* Not real audio: a fixed loop, staggered per bar so it reads as movement
       rather than as a level meter that happens to be wrong. */
    animation: breathe 1.6s ease-in-out infinite;
    animation-delay: calc(var(--bar) * 120ms);
  }

  .listening .bar {
    animation: bounce 900ms ease-in-out infinite;
    animation-delay: calc(var(--bar) * 110ms);
  }

  @keyframes appear {
    from {
      opacity: 0;
      transform: translateY(4px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }

  @keyframes bounce {
    0%,
    100% {
      height: 5px;
      opacity: 0.55;
    }
    50% {
      height: 17px;
      opacity: 1;
    }
  }

  @keyframes breathe {
    0%,
    100% {
      opacity: 0.3;
    }
    50% {
      opacity: 1;
    }
  }

  /* The overlay animates far more than the settings badge does, so honouring
     this matters more here than it does there. */
  @media (prefers-reduced-motion: reduce) {
    .pill,
    .bar,
    .listening .bar {
      animation: none;
    }

    .bar {
      height: 11px;
    }
  }
</style>
