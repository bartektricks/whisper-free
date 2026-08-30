/** Mirrors `state::AppState` in Rust. */
export type AppState =
  | "uninitialized"
  | "ready"
  | "recording"
  | "transcribing"
  | "refining"
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

/** Mirrors `overlay::OverlayAnchor`. */
export type OverlayAnchor =
  | "top_left"
  | "top_centre"
  | "top_right"
  | "centre_left"
  | "centre"
  | "centre_right"
  | "bottom_left"
  | "bottom_centre"
  | "bottom_right";

/** Mirrors `settings::HistoryRetention`. */
export type HistoryRetention =
  /** Kept in memory for this run only, and never written to disk. */
  | "session"
  | "one_day"
  | "seven_days"
  | "thirty_days"
  | "forever";

/** Mirrors `settings::RefineStrength`. */
export type RefineStrength =
  /** Punctuation, capitalisation and a misheard word; anything larger is
   * thrown away, so fillers and false starts survive into the paste. */
  | "light_touch"
  /** Fillers dropped, false starts resolved, numbers and dates written out. */
  | "full_cleanup";

/** Mirrors `refine::prompt::Styling`. */
export type RefineStyling =
  | "casual"
  | "semi_casual"
  | "semi_formal"
  | "formal";

/** Mirrors `history::HistoryEntry`. */
export interface HistoryEntry {
  id: number;
  /** Exactly what was inserted, after refinement and the dictionary. */
  text: string;
  /** Unix seconds. */
  at: number;
}

/** Mirrors `settings::Settings`. */
export interface Settings {
  hotkey: string;
  recording_mode: RecordingMode;
  input_device: string | null;
  model_id: string;
  language: LanguageSelection;
  start_at_login: boolean;
  show_overlay: boolean;
  overlay_anchor: OverlayAnchor;
  mute_while_recording: boolean;
  refine_enabled: boolean;
  refine_model_id: string;
  refine_strength: RefineStrength;
  refine_styling: RefineStyling;
  check_for_updates: boolean;
  keep_on_clipboard: boolean;
  history_enabled: boolean;
  history_retention: HistoryRetention;
  onboarding_completed: boolean;
}

/** Mirrors `platform::PermissionState`. */
export type PermissionState =
  | "granted"
  | "denied"
  /** Never asked, and asking will put a system prompt on screen. */
  | "unasked"
  /** The platform will not say; the capability has to be tried. */
  | "unknown"
  /** This platform does not gate the capability at all. */
  | "not_required";

/** Mirrors `commands::Permissions`. */
export interface Permissions {
  microphone: PermissionState;
  accessibility: PermissionState;
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

/** Mirrors `models::ModelKind`. */
export type ModelKind = "speech" | "refiner";

/** Mirrors `asr::types::Capability`. */
export type Capability =
  /** Detects the spoken language on its own. */
  | "language_detection"
  /** Honours a caller-specified language. */
  | "language_selection"
  | "punctuation"
  | "timestamps"
  | "streaming";

/** Mirrors `models::ModelInfo`. */
export interface ModelInfo {
  id: string;
  name: string;
  description: string;
  kind: ModelKind;
  size_bytes: number;
  languages: Language[];
  capabilities: Capability[];
  installed: boolean;
  bytes_on_disk: number;
}

/** Mirrors `models::download::DownloadProgress`. */
export interface DownloadProgress {
  model_id: string;
  file: string;
  downloaded_bytes: number;
  total_bytes: number;
}

/** Payload of the `model_download_failed` event. */
export interface DownloadFailure {
  model_id: string;
  message: string;
}

/** Mirrors `update::UpdatePhase`. */
export type UpdatePhase =
  | "idle"
  | "checking"
  | "up_to_date"
  | "available"
  | "downloading"
  | "ready_to_restart"
  | "failed";

/** Mirrors `update::UpdateStatus`. */
export interface UpdateStatus {
  phase: UpdatePhase;
  /** The version on offer, once a check has found one. */
  version: string | null;
  /** Where to read what changed, on the releases page. */
  release_url: string | null;
  downloaded_bytes: number;
  /** Zero until the server announces a length, which it need not do. */
  total_bytes: number;
  /** User-facing failure text; only set in the `failed` phase. */
  message: string | null;
}

/** Mirrors `dictionary::DictionaryEntry`. */
export interface DictionaryEntry {
  id: number;
  input: string;
  replacement: string;
  enabled: boolean;
}
