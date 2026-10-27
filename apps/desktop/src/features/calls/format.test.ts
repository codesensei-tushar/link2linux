import { describe, expect, it } from "vitest";

import { formatDuration, groupCode } from "./format";

describe("formatDuration", () => {
  it("shows minutes and seconds, and hours once there are any", () => {
    expect(formatDuration(0)).toBe("0:00");
    expect(formatDuration(65_400)).toBe("1:05");
    expect(formatDuration(3_725_000)).toBe("1:02:05");
    expect(formatDuration(-5_000)).toBe("0:00");
  });
});

describe("groupCode", () => {
  it("splits six digits in two", () => {
    expect(groupCode("482913")).toBe("482 913");
    expect(groupCode("12345")).toBe("12345");
  });
});
