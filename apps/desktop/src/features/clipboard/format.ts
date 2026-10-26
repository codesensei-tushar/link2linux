const KB = 1024;
const MB = 1024 * KB;

/** "1 byte", "512 bytes", "1.2 KB" or "3.4 MB". */
export function formatBytes(bytes: number): string {
  if (bytes === 1) return "1 byte";
  if (bytes < KB) return `${String(bytes)} bytes`;
  if (bytes < MB) return `${trim(bytes / KB)} KB`;
  return `${trim(bytes / MB)} MB`;
}

/** One decimal, dropped when it is zero. */
function trim(value: number): string {
  return value.toFixed(1).replace(/\.0$/, "");
}
