import { describe, expect, it } from "vitest";

import { relativeTime } from "./relative-time";

const now = Date.UTC(2026, 5, 15, 12, 0, 0);

describe("relativeTime", () => {
  it.each([
    [30_000, "Just now"],
    [60_000, "1 min ago"],
    [59 * 60_000, "59 min ago"],
    [60 * 60_000, "1 h ago"],
    [23 * 3_600_000, "23 h ago"],
    [30 * 3_600_000, "Yesterday"],
  ])("formats an age of %i ms", (age, expected) => {
    expect(relativeTime(now - age, now)).toBe(expected);
  });

  it("falls back to a short date after two days", () => {
    expect(relativeTime(now - 3 * 86_400_000, now)).toMatch(/\d/);
  });
});
