/**
 * Translating browser keyboard events to and from Tauri accelerator strings
 * such as `"Alt+Shift+D"`.
 *
 * The accelerator syntax is the same everywhere; only the way it is *shown* and
 * the name of the Meta key differ, so those two are the only platform-aware
 * parts here.
 */

import { isMac } from "./platform";

const MODIFIER_CODES = new Set([
  "ShiftLeft",
  "ShiftRight",
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "MetaLeft",
  "MetaRight",
]);

/** Symbols macOS users expect to see, in the order macOS shows them. */
const SYMBOLS: Record<string, string> = {
  Ctrl: "⌃",
  Alt: "⌥",
  Shift: "⇧",
  Cmd: "⌘",
};

/**
 * The key part of an accelerator, derived from `KeyboardEvent.code`.
 *
 * `code` rather than `key` because it is layout-independent: Option+S must
 * register as "S" and not as the "ß" the OS would produce.
 */
function keyName(code: string): string | null {
  if (MODIFIER_CODES.has(code)) return null;
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  if (/^F\d{1,2}$/.test(code)) return code;
  const NAMED = new Set([
    "Space",
    "Enter",
    "Tab",
    "Escape",
    "Backspace",
    "Delete",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Minus",
    "Equal",
    "Backquote",
    "BracketLeft",
    "BracketRight",
    "Backslash",
    "Semicolon",
    "Quote",
    "Comma",
    "Period",
    "Slash",
  ]);
  return NAMED.has(code) ? code : null;
}

/**
 * Build an accelerator from a key event, or `null` when the combination is not
 * usable as a global shortcut yet.
 *
 * A bare key with no modifier is rejected, because registering plain "D"
 * globally would swallow the letter everywhere on the system — unless this is
 * the second step of a chord (`bare`), which is claimed only for the moment
 * the chord window is open and is the whole point of writing `⌘K K`.
 */
export function toAccelerator(
  event: KeyboardEvent,
  { bare = false }: { bare?: boolean } = {},
): string | null {
  const key = keyName(event.code);
  if (!key) return null;

  // This order is both the macOS glyph order (⌃⌥⇧⌘) and the Windows written
  // convention (Ctrl+Alt+Shift+Win), so it needs no platform branch.
  const modifiers: string[] = [];
  if (event.ctrlKey) modifiers.push("Ctrl");
  if (event.altKey) modifiers.push("Alt");
  if (event.shiftKey) modifiers.push("Shift");
  // Tauri parses both "Cmd" and "Super" as Meta; the difference is what the
  // key is called to the person pressing it.
  if (event.metaKey) modifiers.push(isMac() ? "Cmd" : "Super");

  // Function keys are already global-safe on their own.
  if (!bare && modifiers.length === 0 && !/^F\d{1,2}$/.test(key)) return null;

  return [...modifiers, key].join("+");
}

/**
 * Render a hotkey the way this platform writes it.
 *
 * macOS collapses modifiers into glyphs (`⌥Space`); everywhere else they are
 * spelled out and joined with `+` (`Ctrl+Alt+Space`).
 *
 * A two-step chord is two accelerators separated by a space, and stays that way
 * on both platforms — `⌘K K` and `Ctrl+K K` — which is how VS Code writes them
 * and what the Rust side parses back.
 */
export function formatAccelerator(accelerator: string): string {
  return accelerator.split(" ").filter(Boolean).map(formatStep).join(" ");
}

function formatStep(step: string): string {
  if (!isMac()) return step;

  const parts = step.split("+");
  const key = parts.pop() ?? "";
  const modifiers = parts.map((m) => SYMBOLS[m] ?? `${m}+`).join("");
  return `${modifiers}${key}`;
}

/**
 * Join two captured combinations into the chord syntax the backend parses.
 *
 * A space, because it is what VS Code uses and what no single accelerator
 * contains.
 */
export function toChord(first: string, second: string): string {
  return `${first} ${second}`;
}
