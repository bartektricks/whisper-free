<script lang="ts">
  /**
   * The one-line verdict on a permission, plus whatever action can be taken
   * about it. Shared by the two permission steps so that "granted" looks and
   * reads the same in both.
   *
   * The prop is `status`, not `state`: `$state` is a rune, and a variable
   * called `state` in a Svelte 5 component makes the compiler read every
   * `$state` as a store subscription instead.
   */
  import type { PermissionState } from "../../types";

  let {
    status,
    grantedLabel,
    deniedLabel,
    actionLabel,
    unknownLabel,
    onAction,
  }: {
    status: PermissionState;
    grantedLabel: string;
    deniedLabel: string;
    /** What the button offers to do while the permission is not granted. */
    actionLabel: string;
    /** Shown in place of a verdict where the platform will not say. */
    unknownLabel?: string;
    onAction: () => void;
  } = $props();
</script>

<div class="verdict">
  {#if status === "granted"}
    <span class="line ok"><span class="mark">✓</span>{grantedLabel}</span>
  {:else if status !== "not_required"}
    {#if status === "unknown"}
      <span class="line dim">{unknownLabel ?? deniedLabel}</span>
    {:else}
      <span class="line warn"><span class="mark">!</span>{deniedLabel}</span>
    {/if}
    <!--
      The button is offered for `unknown` too, not only for a refusal: that is
      the state where the user has no way of finding out except by looking, so
      the settings page is the one thing worth putting in front of them. It is
      quieter there, because nothing is known to be wrong yet.
    -->
    <button type="button" class:primary={status !== "unknown"} onclick={onAction}>
      {actionLabel}
    </button>
  {/if}
</div>

<style>
  .verdict {
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
    margin: 18px 0 0;
  }

  .line {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }

  .mark {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    font-size: 11px;
    color: var(--accent-text);
    flex: none;
  }

  .ok {
    color: var(--ok);
  }

  .ok .mark {
    background: var(--ok);
  }

  .warn {
    color: var(--warn);
  }

  .warn .mark {
    background: var(--warn);
  }

  .dim {
    color: var(--text-dim);
    max-width: 52ch;
  }
</style>
