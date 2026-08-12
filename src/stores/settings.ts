import { writable, get } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import type { Settings } from "../types";

const DEFAULTS: Settings = {
  hotkey: "Alt+Space",
  recording_mode: "hold_to_talk",
  input_device: null,
  model_id: "parakeet-tdt-0.6b-v3",
  language: { kind: "auto" },
  start_at_login: false,
};

function createSettingsStore() {
  const { subscribe, set } = writable<Settings>(DEFAULTS);
  const error = writable<string | null>(null);
  const loaded = writable(false);

  async function load() {
    try {
      set(await invoke<Settings>("get_settings"));
      error.set(null);
    } catch (e) {
      error.set(String(e));
    } finally {
      loaded.set(true);
    }
  }

  /**
   * Apply a partial change and persist it.
   *
   * The backend is the source of truth: we write what it returns, so a
   * rejected or normalised value shows up in the UI rather than being
   * silently assumed.
   */
  async function update(patch: Partial<Settings>) {
    const previous = get({ subscribe });
    const next = { ...previous, ...patch };
    set(next); // optimistic, so controls feel immediate
    try {
      set(await invoke<Settings>("update_settings", { settings: next }));
      error.set(null);
    } catch (e) {
      set(previous); // roll back to what is actually persisted
      error.set(String(e));
    }
  }

  return { subscribe, load, update, error, loaded };
}

export const settings = createSettingsStore();
