<script lang="ts">
  import { settings } from "../../stores/settings";
  import { permissions } from "../../stores/permissions";
  import { models } from "../../stores/models";
  import { formatAccelerator } from "../../lib/hotkey";
  import { trayName } from "../../lib/platform";

  const hotkey = $derived(formatAccelerator($settings.hotkey));
  const held = $derived($settings.recording_mode === "hold_to_talk");

  /**
   * Everything that was skipped, said once, here, rather than discovered later
   * as a dictation that quietly does nothing.
   */
  const outstanding = $derived(
    [
      // The *chosen* model, not any speech model: someone who downloaded one
      // and then picked another during setup has nothing loadable, and "some
      // speech model is installed" would call that finished.
      $models.models.find((m) => m.id === $settings.model_id)?.installed
        ? null
        : "The speech model you picked is not downloaded, so dictation cannot run yet. Settings › Models.",
      $permissions.microphone === "denied"
        ? "Microphone access is refused, so recordings come out silent. Settings › Audio has the test."
        : null,
      $permissions.accessibility === "denied"
        ? "Accessibility access is not granted, so text lands on the clipboard for you to paste yourself. Settings › General."
        : null,
    ].filter((line): line is string => line !== null),
  );
</script>

<h2>{outstanding.length > 0 ? "Nearly there" : "You're set"}</h2>

<p class="lead">
  {#if held}
    Hold <kbd>{hotkey}</kbd> anywhere, speak, and let go. The text appears where your
    cursor is.
  {:else}
    Press <kbd>{hotkey}</kbd> to start, speak, and press it again to stop. The text
    appears where your cursor is.
  {/if}
</p>

<ul class="points">
  <li><kbd>Esc</kbd> while dictating throws the run away.</li>
  <li>
    A small indicator floats over your work while it runs, so you can see it is listening.
  </li>
  <li>
    WhisperFree sits in the {trayName()}, and everything here is a click away from that
    icon.
  </li>
</ul>

{#if outstanding.length > 0}
  <div class="outstanding">
    <h3>Left for later</h3>
    <ul>
      {#each outstanding as line (line)}
        <li>{line}</li>
      {/each}
    </ul>
  </div>
{/if}

<style>
  h2 {
    font-size: 19px;
  }

  .lead {
    margin: 10px 0 0;
    max-width: 56ch;
    font-size: 13px;
  }

  kbd {
    font: inherit;
    font-weight: 600;
    border: 1px solid var(--border);
    border-bottom-width: 2px;
    border-radius: 5px;
    padding: 1px 6px;
    background: var(--panel);
    white-space: nowrap;
  }

  .points {
    margin: 20px 0 0;
    padding-left: 18px;
    max-width: 56ch;
    font-size: 12px;
    color: var(--text-dim);
    display: flex;
    flex-direction: column;
    gap: 7px;
  }

  .outstanding {
    margin-top: 22px;
    border: 1px solid var(--border);
    border-left: 3px solid var(--warn);
    border-radius: 8px;
    padding: 12px 16px;
    background: var(--panel);
    max-width: 58ch;
  }

  .outstanding h3 {
    font-size: 11px;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--warn);
  }

  .outstanding ul {
    margin: 8px 0 0;
    padding-left: 16px;
    font-size: 12px;
    color: var(--text-dim);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
</style>
