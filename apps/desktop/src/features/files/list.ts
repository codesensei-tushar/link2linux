import type { FileTransfer, FileTransferEvent, TransferFailure } from "@/lib/api";

/** Applies one backend event to the list, which stays oldest first. */
export function applyTransferEvent(
  transfers: FileTransfer[],
  event: FileTransferEvent,
): FileTransfer[] {
  switch (event.type) {
    case "changed": {
      const index = transfers.findIndex((t) => t.id === event.transfer.id);
      if (index === -1) {
        return [...transfers, event.transfer].sort((a, b) => a.startedUnixMs - b.startedUnixMs);
      }
      return transfers.map((t, i) => (i === index ? event.transfer : t));
    }
    case "progress":
      return transfers.map((t) => (t.id === event.id ? { ...t, done: event.done } : t));
    case "removed":
      return transfers.filter((t) => t.id !== event.id);
  }
}

/** Whether the transfer is over, for good or not. */
export function isFinished(transfer: FileTransfer): boolean {
  const { kind } = transfer.status;
  return kind === "completed" || kind === "cancelled" || kind === "failed";
}

/** "photo.jpg", or "photo.jpg and 2 more". */
export function transferTitle(transfer: FileTransfer): string {
  const [first, ...rest] = transfer.files;
  if (!first) return "No files";
  return rest.length === 0 ? first.name : `${first.name} and ${String(rest.length)} more`;
}

/** Bytes in decimal units, as file managers show them: "740 KB", "3.2 MB". */
export function formatBytes(bytes: number): string {
  if (bytes < 1000) return `${String(bytes)} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1000;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  const shown = value >= 100 ? Math.round(value).toString() : value.toFixed(1);
  return `${shown} ${units[unit] ?? "TB"}`;
}

/** Whole percent done, rounded down so 100% means done. */
export function percentDone(transfer: FileTransfer): number {
  if (transfer.size === 0) return isFinished(transfer) ? 100 : 0;
  return Math.floor((transfer.done / transfer.size) * 100);
}

/** Why a transfer failed, in plain words. */
export function failureText(
  reason: TransferFailure,
  transfer: FileTransfer,
  phoneName: string,
): string {
  switch (reason) {
    case "disabled":
      return `Files are turned off on ${phoneName}`;
    case "disabledHere":
      return "Files are turned off on this computer";
    case "permissionRequired":
      return `${phoneName} needs permission to save files`;
    case "noSpace":
      return transfer.direction === "outgoing"
        ? `Not enough space on ${phoneName}`
        : "Not enough space on this computer";
    case "refused":
      return `${phoneName} didn't accept the files`;
    case "unreadable":
      return "A file couldn't be read";
    case "changed":
      return "A file changed while it was being sent";
    case "damaged":
      return "A file arrived damaged, so it was deleted";
    case "invalid":
      return `${phoneName} sent the files incorrectly`;
    case "writeFailed":
      return "A file couldn't be saved";
    case "expired":
      return `${phoneName} didn't reconnect within an hour`;
    case "tooManyUnfinished":
      return `Too many unfinished transfers from ${phoneName}, so this one was dropped`;
  }
}

/** Where the transfer stands, in plain words. */
export function statusText(transfer: FileTransfer, phoneName: string): string {
  const outgoing = transfer.direction === "outgoing";
  const { status } = transfer;
  switch (status.kind) {
    case "waiting":
      return outgoing ? `Waiting for ${phoneName}` : "Waiting";
    case "running":
      return `${outgoing ? "Sending" : "Receiving"}… ${String(percentDone(transfer))}%`;
    case "interrupted":
      return `Paused until ${phoneName} reconnects`;
    case "completed":
      return outgoing ? `Sent to ${phoneName}` : "Saved in Downloads";
    case "cancelled":
      return status.byPhone ? `Cancelled on ${phoneName}` : "Cancelled";
    case "failed":
      return failureText(status.reason, transfer, phoneName);
  }
}

/** How long a rate stays on screen without new bytes. */
const STALE_MS = 3_000;

/** The rate to show at `now`, or null when none was measured or bytes stopped coming. */
export function shownRate(rate: Rate | undefined, now: number): number | null {
  if (rate?.perSecond == null || now - rate.at > STALE_MS) return null;
  return rate.perSecond;
}

/** A transfer rate measured over at least `MIN_SPAN_MS`, smoothed. */
export interface Rate {
  /** When `done` was seen, in milliseconds. */
  at: number;
  done: number;
  /** Bytes per second, or null until measured. */
  perSecond: number | null;
}

const MIN_SPAN_MS = 500;

/** The rate after seeing `done` bytes at `now`. */
export function nextRate(previous: Rate | undefined, done: number, now: number): Rate {
  if (!previous || done < previous.done) return { at: now, done, perSecond: null };
  const span = now - previous.at;
  // After a stall the bytes of the gap are not a speed: measure afresh.
  if (span > STALE_MS) return { at: now, done, perSecond: null };
  if (span < MIN_SPAN_MS) return previous;
  const measured = ((done - previous.done) / span) * 1000;
  const perSecond =
    previous.perSecond === null ? measured : previous.perSecond * 0.5 + measured * 0.5;
  return { at: now, done, perSecond };
}
