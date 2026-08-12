/**
 * Translating browser keyboard events to and from Tauri accelerator strings
 * such as `"Alt+Shift+D"`.
 */

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
 * A bare key with no modifier is rejected: registering plain "D" globally would
 * swallow the letter everywhere on the system.
 */
export function toAccelerator(event: KeyboardEvent): string | null {
  const key = keyName(event.code);
  if (!key) return null;

  const modifiers: string[] = [];
  if (event.ctrlKey) modifiers.push("Ctrl");
  if (event.altKey) modifiers.push("Alt");
  if (event.shiftKey) modifiers.push("Shift");
  if (event.metaKey) modifiers.push("Cmd");

  // Function keys are already global-safe on their own.
  if (modifiers.length === 0 && !/^F\d{1,2}$/.test(key)) return null;

  return [...modifiers, key].join("+");
}

/** Render an accelerator the way macOS would show it, e.g. `⌥Space`. */
export function formatAccelerator(accelerator: string): string {
  const parts = accelerator.split("+");
  const key = parts.pop() ?? "";
  const modifiers = parts.map((m) => SYMBOLS[m] ?? `${m}+`).join("");
  return `${modifiers}${key}`;
}
