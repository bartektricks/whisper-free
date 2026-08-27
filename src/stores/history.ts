import { writable } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { HistoryEntry } from "../types";

/**
 * What has been dictated and kept (decision 0011).
 *
 * A store rather than component state, and its listener attached once at module
 * load rather than on mount, for the reason `models.ts` is shaped that way: the
 * settings window renders one section at a time with no keepalive. Dictation
 * carries on while the window is open, so an entry recorded while the user is
 * looking at the hotkey panel has to be there when they come back, and a list
 * that only listened while mounted would miss it.
 *
 * The backend is the source of truth throughout. Every mutation here returns
 * the new list and that is what gets stored, so a delete the backend refused
 * does not leave the UI showing something that is still on disk.
 */
function createHistoryStore() {
  const { subscribe, set } = writable<HistoryEntry[]>([]);
  const error = writable<string | null>(null);

  async function load() {
    try {
      set(await invoke<HistoryEntry[]>("get_history"));
      error.set(null);
    } catch (e) {
      error.set(String(e));
    }
  }

  /** Run a command that returns the new list, and show its refusal if it fails. */
  async function apply(command: string, args?: Record<string, unknown>) {
    try {
      set(await invoke<HistoryEntry[]>(command, args));
      error.set(null);
    } catch (e) {
      error.set(String(e));
    }
  }

  return {
    subscribe,
    load,
    error,
    remove: (id: number) => apply("delete_history_entry", { id }),
    clear: () => apply("clear_history"),

    /**
     * Put an entry back on the clipboard.
     *
     * Rust does the writing: the settings window holds no clipboard capability,
     * and the backend owns the clipboard everywhere else in the app.
     */
    async copy(id: number): Promise<boolean> {
      try {
        await invoke("copy_history_entry", { id });
        error.set(null);
        return true;
      } catch (e) {
        error.set(String(e));
        return false;
      }
    },
  };
}

export const history = createHistoryStore();

// A dictation that finishes while the window is open should appear in the list
// without the user going away and coming back.
void listen("history_changed", () => {
  void history.load();
});
