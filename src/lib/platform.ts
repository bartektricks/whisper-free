/**
 * Platform-dependent wording for the settings UI.
 *
 * Mirrors `platform::strings` in the Rust backend by hand, the same way
 * `src/types/index.ts` mirrors the serde shapes. Change one, change the other.
 *
 * Everything here is a getter rather than a top-level constant: `platform()`
 * reads a value Tauri injects, so evaluating it at module load would throw in
 * a plain `bun run dev` session and take the whole page down with it.
 */

import { platform } from "@tauri-apps/plugin-os";

let cached: string | null = null;

function os(): string {
  cached ??= platform();
  return cached;
}

/** True on macOS, where modifiers are glyphs and Meta is Command. */
export function isMac(): boolean {
  return os() === "macos";
}

/** What to call the operating system in a sentence. */
export function systemName(): string {
  return isMac() ? "macOS" : "Windows";
}

/** What this platform calls the strip the tray icon lives in. */
export function trayName(): string {
  return isMac() ? "menu bar" : "notification area";
}

/** Where microphone access is granted, in words. */
export function microphoneSettings(): string {
  return isMac()
    ? "System Settings › Privacy & Security › Microphone"
    : "Settings › Privacy & security › Microphone";
}
