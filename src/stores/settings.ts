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
  show_overlay: true,
  overlay_anchor: "bottom_centre",
  refine_enabled: false,
  refine_model_id: "qwen2.5-0.5b-instruct",
  check_for_updates: false,
  // True, like the Rust default, and for the same reason: the onboarding
  // takeover is keyed off this, and a `false` here would flash setup over the
  // settings window for the moment before `load()` returns.
  onboarding_completed: true,
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
   *
   * Returns the backend's refusal message, or `null` when the change stuck, so
   * a caller can also show it beside the control that caused it — the backend
   * distinguishes an unparseable shortcut from one another app already owns,
   * and that distinction is worth keeping.
   */
  async function update(patch: Partial<Settings>): Promise<string | null> {
    const previous = get({ subscribe });
    const next = { ...previous, ...patch };
    set(next); // optimistic, so controls feel immediate
    try {
      set(await invoke<Settings>("update_settings", { settings: next }));
      error.set(null);
      return null;
    } catch (e) {
      set(previous); // roll back to what is actually persisted
      const message = String(e);
      error.set(message);
      return message;
    }
  }

  return { subscribe, load, update, error, loaded };
}

export const settings = createSettingsStore();
