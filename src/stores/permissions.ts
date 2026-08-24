import { readable } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import type { Permissions } from "../types";

/**
 * How often to ask the backend again.
 *
 * These permissions are granted in *another* application, either System
 * Settings or a system prompt, so there is no event to subscribe to and
 * polling is the only way to notice. Both checks are a single cheap system call each, and the
 * store only polls while something is subscribed to it, which for a menu-bar
 * app means only while its settings window is open.
 */
const POLL_MS = 1500;

const INITIAL: Permissions = { microphone: "unknown", accessibility: "unknown" };

export const permissions = readable<Permissions>(INITIAL, (set) => {
  let stopped = false;

  async function refresh() {
    try {
      const next = await invoke<Permissions>("get_permissions");
      if (!stopped) set(next);
    } catch (e) {
      // A permission we cannot read is not an error worth a banner: the step
      // that shows it falls back to "try it and see".
      console.error("could not read permissions", e);
    }
  }

  void refresh();
  const timer = setInterval(refresh, POLL_MS);

  return () => {
    stopped = true;
    clearInterval(timer);
  };
});

/** Raise the system prompt, or open the settings pane where there is none. */
export function requestMicrophone(): void {
  invoke("request_microphone_permission").catch((e) =>
    console.error("could not ask for microphone access", e),
  );
}

/** Open the pane where synthetic input is allowed. There is no prompt to raise. */
export function requestAccessibility(): void {
  invoke("request_insert_permission").catch((e) =>
    console.error("could not open the accessibility pane", e),
  );
}
