import type { FileTransfer } from "@/lib/api";

const MINUTE_MS = 60_000;

/** Transfers for the browser preview: one of each kind the page shows. */
export function demoTransfers(now: number): FileTransfer[] {
  const file = (name: string, size: number, saved = false) => ({
    name,
    size,
    mimeType: "",
    saved,
  });
  return [
    {
      id: "00000000000000000000000000000001",
      direction: "incoming",
      status: { kind: "completed" },
      files: [file("IMG_2041.jpg", 2_412_118, true), file("IMG_2042.jpg", 2_198_540, true)],
      size: 4_610_658,
      done: 4_610_658,
      startedUnixMs: now - 42 * MINUTE_MS,
    },
    {
      id: "00000000000000000000000000000002",
      direction: "outgoing",
      status: { kind: "failed", reason: "noSpace" },
      files: [file("backup.zip", 3_900_000_000)],
      size: 3_900_000_000,
      done: 0,
      startedUnixMs: now - 20 * MINUTE_MS,
    },
    {
      id: "00000000000000000000000000000003",
      direction: "incoming",
      status: { kind: "completed" },
      files: [file("Boarding pass.pdf", 182_330, true)],
      size: 182_330,
      done: 182_330,
      startedUnixMs: now - 9 * MINUTE_MS,
    },
    {
      id: "00000000000000000000000000000004",
      direction: "outgoing",
      status: { kind: "running" },
      files: [file("Holiday.mp4", 184_000_000)],
      size: 184_000_000,
      done: 110_400_000,
      startedUnixMs: now - MINUTE_MS,
    },
  ];
}
