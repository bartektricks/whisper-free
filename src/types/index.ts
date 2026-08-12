/** Mirrors `state::AppState` in Rust. */
export type AppState =
  | "uninitialized"
  | "ready"
  | "recording"
  | "transcribing"
  | "inserting"
  | "error";

/** Mirrors `state::StateSnapshot`. */
export interface StateSnapshot {
  state: AppState;
  /** User-facing failure text; only set in the `error` state. */
  message: string | null;
}

/** Mirrors `settings::RecordingMode`. */
export type RecordingMode = "hold_to_talk" | "toggle";

/** Mirrors `asr::types::LanguageSelection` (serde tag/content). */
export type LanguageSelection = { kind: "auto" } | { kind: "fixed"; code: string };

/** Mirrors `settings::Settings`. */
export interface Settings {
  hotkey: string;
  recording_mode: RecordingMode;
  input_device: string | null;
  model_id: string;
  language: LanguageSelection;
  start_at_login: boolean;
}

/** Mirrors `audio::AudioDevice`. */
export interface AudioDevice {
  /** Stable system identifier; this is what gets persisted. */
  id: string;
  name: string;
  is_default: boolean;
}

/** Mirrors `commands::MicrophoneTest`. */
export interface MicrophoneTest {
  duration_ms: number;
  peak_level: number;
  heard_audio: boolean;
}

/** Mirrors `asr::types::Language`. */
export interface Language {
  code: string;
  name: string;
}
