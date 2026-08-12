<script lang="ts">
  import type { StateSnapshot } from "../../types";
  import { stateLabel } from "../../stores/appState";

  let { snapshot }: { snapshot: StateSnapshot } = $props();

  const tone = $derived(
    snapshot.state === "error"
      ? "err"
      : snapshot.state === "uninitialized"
        ? "warn"
        : snapshot.state === "ready"
          ? "ok"
          : "busy",
  );
</script>

<div class="badge {tone}">
  <span class="dot"></span>
  <span class="label">{stateLabel(snapshot)}</span>
</div>

<style>
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-size: 12px;
    color: var(--text-dim);
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-dim);
    flex: none;
  }

  .ok .dot {
    background: var(--ok);
  }

  .warn .dot {
    background: var(--warn);
  }

  .err .dot {
    background: var(--err);
  }

  .err .label {
    color: var(--err);
  }

  .busy .dot {
    background: var(--accent);
    animation: pulse 1.2s ease-in-out infinite;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.25;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .busy .dot {
      animation: none;
    }
  }
</style>
