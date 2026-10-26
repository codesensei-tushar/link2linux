import { describe, expect, it } from "vitest";

import { formatBytes } from "./format";

describe("formatBytes", () => {
  it.each([
    [1, "1 byte"],
    [42, "42 bytes"],
    [1023, "1023 bytes"],
    [1024, "1 KB"],
    [1_234, "1.2 KB"],
    [262_144, "256 KB"],
    [3_565_158, "3.4 MB"],
  ])("formats %i bytes", (bytes, expected) => {
    expect(formatBytes(bytes)).toBe(expected);
  });
});
