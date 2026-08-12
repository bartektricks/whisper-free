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
