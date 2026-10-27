import { describe, expect, it } from "vitest";

import { testPhoneStatus } from "@/test/fake-api";

import {
  ringEndText,
  ringProblemText,
  statusItems,
  statusNote,
  type CurrentStatus,
} from "./format";

function current(overrides: Partial<CurrentStatus> = {}): CurrentStatus {
  return testPhoneStatus(overrides) as CurrentStatus;
}

const labels = (status: CurrentStatus) => statusItems(status).map((item) => item.label);
const texts = (status: CurrentStatus) => statusItems(status).map((item) => item.text);

describe("statusItems", () => {
  it("names each part of a full status in words", () => {
    expect(labels(current())).toEqual([
      "Battery 76%",
      "Mobile signal 3 bars of 4, 5G, Carrier",
      "Wi-Fi 4 bars of 4",
    ]);
    expect(texts(current())).toEqual(["76%", "5G", ""]);
  });

  it("marks power, battery saver, a quiet ringer and Do Not Disturb", () => {
    const status = current({
      battery: { percent: 100, pluggedIn: true, powerSaver: true },
      ringer: "silent",
      doNotDisturb: true,
      wifi: null,
    });
    expect(labels(status)).toEqual([
      "Battery 100%, plugged in",
      "Battery saver on",
      "Mobile signal 3 bars of 4, 5G, Carrier",
      "Ringer silenced",
      "Do Not Disturb on",
    ]);
    expect(labels(current({ ringer: "vibrate" }))).toContain("Ringer set to vibrate");
  });

  it("leaves out what is unknown", () => {
    const status = current({
      battery: { percent: null, pluggedIn: false, powerSaver: false },
      cellular: { level: null, network: null, operatorName: null },
      wifi: { level: null },
      ringer: null,
    });
    expect(labels(status)).toEqual(["Battery level unknown", "Mobile signal", "Wi-Fi"]);
    expect(texts(status)).toEqual(["", "", ""]);
    expect(statusItems(current({ battery: null, cellular: null, wifi: null }))).toEqual([]);
  });

  it("says when the phone has no mobile service, and counts one bar", () => {
    const status = current({
      cellular: { level: 0, network: "noService", operatorName: "Carrier" },
      wifi: { level: 1 },
    });
    expect(labels(status)).toEqual(["Battery 76%", "No mobile service", "Wi-Fi 1 bar of 4"]);
    expect(texts(status)[1]).toBe("No service");
  });
});

describe("notes", () => {
  it("says why a status is not shown, or nothing", () => {
    expect(statusNote({ state: "disconnected" })).toBe("Not connected");
    expect(statusNote({ state: "turnedOff" })).toBe("Status is turned off on the phone");
    expect(statusNote({ state: "unsupported" })).toMatch(/Update link2linux/);
    expect(statusNote({ state: "unavailable" })).toBe("Status unavailable");
    expect(statusNote({ state: "waiting" })).toBeNull();
    expect(statusNote({ state: "hidden" })).toBeNull();
  });

  it("gives each ring problem and ending a sentence", () => {
    expect(ringProblemText("turnedOff")).toBe("Ringing is turned off on your phone.");
    expect(ringProblemText("unavailable")).toMatch(/during a call/);
    expect(ringProblemText("notConnected")).toBe("Your phone isn't connected.");
    expect(ringProblemText("failed")).toMatch(/Try again/);
    expect(ringProblemText("unsupported")).toMatch(/Update link2linux/);
    expect(ringEndText("stoppedOnPhone")).toBe("Stopped on the phone.");
    expect(ringEndText("interrupted")).toMatch(/call started/);
    expect(ringEndText("timedOut")).toMatch(/full minute/);
    expect(ringEndText("stoppedFromDesktop")).toMatch(/another computer/);
  });
});
