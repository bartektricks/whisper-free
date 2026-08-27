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

/** The paste shortcut, mirroring `platform::strings::PASTE_SHORTCUT`. */
export function pasteShortcut(): string {
  return isMac() ? "Cmd+V" : "Ctrl+V";
}

/** Where microphone access is granted, in words. */
export function microphoneSettings(): string {
  return isMac()
    ? "System Settings › Privacy & Security › Microphone"
    : "Settings › Privacy & security › Microphone";
}

/** Where permission to paste into other apps is granted, in words. */
export function accessibilitySettings(): string {
  return "System Settings › Privacy & Security › Accessibility";
}

/**
 * What happens to the running app while an update installs.
 *
 * The platforms genuinely differ, so this is a whole sentence rather than a
 * noun — the same reasoning as `INSERT_PERMISSION_DENIED` in
 * `platform::strings`. macOS swaps the bundle underneath a process that keeps
 * running, so the app can offer to restart itself; the Windows installer exits
 * the app as part of installing and there is nothing left to press a button.
 */
export function updateInstallNote(): string {
  return isMac()
    ? "WhisperFree keeps running while the update downloads, and restarts when you say so."
    : "WhisperFree will close while the update installs. Start it again from the Start menu when it finishes.";
}
