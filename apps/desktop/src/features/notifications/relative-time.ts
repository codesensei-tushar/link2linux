const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** "Just now", "5 min ago", "3 h ago", "Yesterday", or a short date. */
export function relativeTime(whenUnixMs: number, nowUnixMs: number): string {
  const age = nowUnixMs - whenUnixMs;
  if (age < MINUTE) return "Just now";
  if (age < HOUR) return `${Math.floor(age / MINUTE)} min ago`;
  if (age < DAY) return `${Math.floor(age / HOUR)} h ago`;
  if (age < 2 * DAY) return "Yesterday";
  return new Date(whenUnixMs).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}
