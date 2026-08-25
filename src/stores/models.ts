import { writable, get } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { DownloadFailure, DownloadProgress, ModelInfo } from "../types";

export interface ModelsState {
  models: ModelInfo[];
  /** Downloads in flight, keyed by model id. */
  progress: Record<string, DownloadProgress>;
  /** The last failure per model id, cleared when a download is retried. */
  errors: Record<string, string>;
}

const EMPTY: ModelsState = { models: [], progress: {}, errors: {} };

/**
 * Installed models and any download in flight.
 *
 * A store rather than component state for the same reason `update.ts` is one:
 * the settings window renders one section at a time with no keepalive, so a
 * user who starts the 671 MB download and then goes to look at the hotkey
 * would otherwise come back to a progress bar that had been destroyed and
 * recreated empty. The listeners below are attached once for the lifetime of
 * the webview, so nothing is missed while no panel is mounted. Onboarding and
 * Settings › Models are then two views of the same download rather than two
 * copies of the same wiring.
 */
function createModelsStore() {
  const { subscribe, update } = writable<ModelsState>(EMPTY);

  async function refresh() {
    const models = await invoke<ModelInfo[]>("get_models");
    update((state) => ({ ...state, models }));
  }

  /** Drop a model's entry from `progress`, whatever ended the download. */
  function forget(id: string) {
    update((state) => {
      const { [id]: _dropped, ...progress } = state.progress;
      return { ...state, progress };
    });
  }

  async function download(id: string) {
    // Shown before the first progress event arrives, so the button responds
    // immediately to a click that starts a several-minute download.
    update((state) => ({
      ...state,
      errors: { ...state.errors, [id]: "" },
      progress: {
        ...state.progress,
        [id]: { model_id: id, file: "", downloaded_bytes: 0, total_bytes: 0 },
      },
    }));

    try {
      await invoke("download_model", { modelId: id });
    } catch (e) {
      // Only the failure to *start*; a download that dies later arrives as a
      // `model_download_failed` event.
      update((state) => {
        const { [id]: _dropped, ...progress } = state.progress;
        return { ...state, progress, errors: { ...state.errors, [id]: String(e) } };
      });
    }
  }

  async function cancel(id: string) {
    await invoke("cancel_model_download", { modelId: id });
  }

  async function remove(id: string) {
    await invoke("delete_model", { modelId: id });
    await refresh();
  }

  /** Whether `id` is installed, for callers acting on a completed download. */
  function isInstalled(id: string): boolean {
    return get({ subscribe }).models.some((m) => m.id === id && m.installed);
  }

  listen<DownloadProgress>("model_download_progress", (event) => {
    update((state) => ({
      ...state,
      progress: { ...state.progress, [event.payload.model_id]: event.payload },
    }));
  }).catch((e) => console.error("could not follow model downloads", e));

  listen<string>("model_download_completed", (event) => {
    forget(event.payload);
    void refresh();
  }).catch((e) => console.error("could not follow model downloads", e));

  listen<DownloadFailure>("model_download_failed", (event) => {
    update((state) => {
      const { [event.payload.model_id]: _dropped, ...progress } = state.progress;
      return {
        ...state,
        progress,
        errors: { ...state.errors, [event.payload.model_id]: event.payload.message },
      };
    });
    void refresh();
  }).catch((e) => console.error("could not follow model downloads", e));

  return { subscribe, refresh, download, cancel, remove, isInstalled };
}

export const models = createModelsStore();
