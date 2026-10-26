/** "4:32" for 272 000 ms. Rounds up, so 0:00 only shows once time is up. */
export function formatRemaining(ms: number): string {
  const seconds = Math.ceil(Math.max(0, ms) / 1000);
  const rest = String(seconds % 60).padStart(2, "0");
  return `${String(Math.floor(seconds / 60))}:${rest}`;
}
