import { describe, expect, it } from "vitest";

import { testTransfer } from "@/test/fake-api";

import {
  applyTransferEvent,
  formatBytes,
  nextRate,
  percentDone,
  shownRate,
  statusText,
  transferTitle,
} from "./list";

describe("applyTransferEvent", () => {
  const a = testTransfer({ id: "a", startedUnixMs: 1 });
  const b = testTransfer({ id: "b", startedUnixMs: 2 });

  it("adds new transfers in start order and replaces changed ones in place", () => {
    expect(applyTransferEvent([b], { type: "changed", transfer: a })).toEqual([a, b]);
    const done = { ...a, status: { kind: "completed" as const } };
    expect(applyTransferEvent([a, b], { type: "changed", transfer: done })).toEqual([done, b]);
  });

  it("moves progress and removes transfers by id", () => {
    expect(applyTransferEvent([a, b], { type: "progress", id: "b", done: 7 })[1]?.done).toBe(7);
    expect(applyTransferEvent([a, b], { type: "removed", id: "a" })).toEqual([b]);
    expect(applyTransferEvent([a], { type: "removed", id: "zzz" })).toEqual([a]);
  });
});

describe("wording", () => {
  it("names a transfer by its first file", () => {
    const file = (name: string) => ({ name, size: 1, mimeType: "", saved: false });
    expect(transferTitle(testTransfer({ files: [file("a.jpg")] }))).toBe("a.jpg");
    expect(transferTitle(testTransfer({ files: [file("a.jpg"), file("b"), file("c")] }))).toBe(
      "a.jpg and 2 more",
    );
  });

  it("formats sizes in decimal units", () => {
    expect(formatBytes(999)).toBe("999 B");
    expect(formatBytes(1_500)).toBe("1.5 KB");
    expect(formatBytes(740_000)).toBe("740 KB");
    expect(formatBytes(3_210_000)).toBe("3.2 MB");
    expect(formatBytes(3_900_000_000)).toBe("3.9 GB");
  });

  it("says where a transfer stands, from this computer's side", () => {
    const sending = testTransfer({ direction: "outgoing", size: 200, done: 99 });
    expect(statusText(sending, "Pixel")).toBe("Sending… 49%");
    expect(percentDone(testTransfer({ size: 0, done: 0, status: { kind: "completed" } }))).toBe(
      100,
    );
    expect(statusText({ ...sending, status: { kind: "waiting" } }, "Pixel")).toBe(
      "Waiting for Pixel",
    );
    expect(statusText({ ...sending, status: { kind: "interrupted" } }, "Pixel")).toBe(
      "Paused until Pixel reconnects",
    );
    expect(statusText(testTransfer({ status: { kind: "completed" } }), "Pixel")).toBe(
      "Saved in Downloads",
    );
    expect(
      statusText(testTransfer({ status: { kind: "cancelled", byPhone: true } }), "Pixel"),
    ).toBe("Cancelled on Pixel");
    const full = { kind: "failed", reason: "noSpace" } as const;
    expect(statusText({ ...sending, status: full }, "Pixel")).toBe("Not enough space on Pixel");
    expect(statusText(testTransfer({ status: full }), "Pixel")).toBe(
      "Not enough space on this computer",
    );
    expect(
      statusText(testTransfer({ status: { kind: "failed", reason: "damaged" } }), "Pixel"),
    ).toBe("A file arrived damaged, so it was deleted");
    const failed = (reason: "disabledHere" | "tooManyUnfinished") =>
      statusText(testTransfer({ status: { kind: "failed", reason } }), "Pixel");
    expect(failed("disabledHere")).toBe("Files are turned off on this computer");
    expect(failed("tooManyUnfinished")).toContain("Too many unfinished transfers from Pixel");
  });
});

describe("nextRate", () => {
  it("measures over half a second at least and smooths", () => {
    const first = nextRate(undefined, 0, 1_000);
    expect(first.perSecond).toBeNull();
    expect(nextRate(first, 100_000, 1_100)).toBe(first);
    const second = nextRate(first, 1_000_000, 2_000);
    expect(second.perSecond).toBe(1_000_000);
    expect(nextRate(second, 1_500_000, 3_000).perSecond).toBe(750_000);
    expect(nextRate(second, 10, 3_000).perSecond).toBeNull();
  });

  it("starts over when the samples are far apart", () => {
    const before = { at: 1_000, done: 1_000, perSecond: 2_000 };
    expect(nextRate(before, 3_000, 3_000).perSecond).toBe(1_500);
    expect(nextRate(before, 2_000, 21_000)).toEqual({ at: 21_000, done: 2_000, perSecond: null });
  });
});

describe("shownRate", () => {
  it("shows a measured rate until bytes stop coming for a few seconds", () => {
    expect(shownRate(undefined, 0)).toBeNull();
    expect(shownRate({ at: 1_000, done: 5, perSecond: null }, 1_000)).toBeNull();
    const rate = { at: 1_000, done: 5, perSecond: 2_000 };
    expect(shownRate(rate, 3_000)).toBe(2_000);
    expect(shownRate(rate, 4_001)).toBeNull();
  });
});
