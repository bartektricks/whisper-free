import { readable } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { StateSnapshot } from "../types";

const INITIAL: StateSnapshot = { state: "uninitialized", message: null };

/**
 * The backend owns application state (plan §15); this store only mirrors it.
 * Nothing in the UI writes to it.
 */
export const appState = readable<StateSnapshot>(INITIAL, (set) => {
  let stop: (() => void) | undefined;
  let cancelled = false;

  invoke<StateSnapshot>("get_recording_state")
    .then((snapshot) => {
      if (!cancelled) set(snapshot);
    })
    .catch((e) => console.error("could not read initial state", e));

  listen<StateSnapshot>("state_changed", (event) => set(event.payload))
    .then((unlisten) => {
      if (cancelled) unlisten();
      else stop = unlisten;
    })
    .catch((e) => console.error("could not subscribe to state changes", e));

  return () => {
    cancelled = true;
    stop?.();
  };
});

/** Short label for the current state, for the status line. */
export function stateLabel(snapshot: StateSnapshot): string {
  switch (snapshot.state) {
    case "uninitialized":
      return "No model installed";
    case "ready":
      return "Ready";
    case "recording":
      return "Recording";
    case "transcribing":
      return "Transcribing";
    case "inserting":
      return "Inserting";
    case "error":
      return snapshot.message ?? "Something went wrong";
  }
}
