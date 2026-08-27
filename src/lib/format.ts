/** Human-readable byte size, e.g. `640 MB`. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

/**
 * A short summary of a language list, e.g. `Polish, English and 23 more`.
 *
 * Polish and English lead because those are the languages this app is built
 * for; the rest are a count rather than a wall of names.
 */
export function summariseLanguages(
  languages: { code: string; name: string }[],
): string {
  const preferred = ["pl", "en"];
  const lead = preferred
    .map((code) => languages.find((l) => l.code === code)?.name)
    .filter((name): name is string => Boolean(name));

  const rest = languages.length - lead.length;
  if (lead.length === 0) return `${languages.length} languages`;
  if (rest <= 0) return lead.join(" and ");
  return `${lead.join(", ")} and ${rest} more`;
}

/**
 * When something was dictated, relative to now, e.g. `12 minutes ago`.
 *
 * Relative rather than a timestamp because that is the question the history
 * list answers: entries are found by "the one from just before lunch", not by
 * a clock reading. Anything past a week gets a date, where relative stops
 * helping.
 */
export function formatWhen(unixSeconds: number, now = Date.now()): string {
  const seconds = Math.max(0, Math.round(now / 1000 - unixSeconds));
  if (seconds < 60) return "just now";

  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;

  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} hour${hours === 1 ? "" : "s"} ago`;

  const days = Math.floor(hours / 24);
  if (days === 1) return "yesterday";
  if (days < 7) return `${days} days ago`;

  return new Date(unixSeconds * 1000).toLocaleDateString();
}
