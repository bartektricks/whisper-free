import { readable } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UpdateStatus } from "../types";

const INITIAL: UpdateStatus = {
  phase: "idle",
  version: null,
  release_url: null,
  downloaded_bytes: 0,
  total_bytes: 0,
  message: null,
};

/**
 * The backend owns the update, the same way it owns application state
 * (decision 0006); this store only mirrors it.
 *
 * It has to live here rather than in the panel: `App.svelte` renders one
 * section at a time with no keepalive, so switching to Models mid-download
 * would destroy a component holding the progress and the panel would come back
 * blank. A store outside the component tree survives that, and the backend is
 * the source of truth either way.
 */
export const updateStatus = readable<UpdateStatus>(INITIAL, (set) => {
  let stop: (() => void) | undefined;
  let cancelled = false;

  invoke<UpdateStatus>("get_update_status")
    .then((status) => {
      if (!cancelled) set(status);
    })
    .catch((e) => console.error("could not read update status", e));

  listen<UpdateStatus>("update_status_changed", (event) => set(event.payload))
    .then((unlisten) => {
      if (cancelled) unlisten();
      else stop = unlisten;
    })
    .catch((e) => console.error("could not subscribe to update status", e));

  return () => {
    cancelled = true;
    stop?.();
  };
});

/**
 * Ask whether a newer version is published.
 *
 * The commands below never reject in practice — the backend reports refusals
 * and failures through the status, so they arrive beside the button that
 * caused them rather than as a thrown string the panel would have to render a
 * second way.
 */
export async function checkForUpdates(): Promise<void> {
  await invoke("check_for_updates");
}

/** Download the offered update and put it in place. */
export async function installUpdate(): Promise<void> {
  await invoke("install_update");
}

/** Restart into the version just installed. */
export async function restartForUpdate(): Promise<void> {
  await invoke("restart_for_update");
}

/**
 * Open the release notes in the browser.
 *
 * Goes through the backend rather than `@tauri-apps/plugin-opener`: the
 * settings window is granted no `opener` permission, and every other link the
 * app follows is opened from Rust.
 */
export async function openReleaseNotes(): Promise<void> {
  await invoke("open_release_notes");
}
