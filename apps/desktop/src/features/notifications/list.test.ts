import { describe, expect, it } from "vitest";

import { testNotification } from "@/test/fake-api";

import { applyEvent, groupNotifications } from "./list";

describe("applyEvent", () => {
  const a = testNotification({ key: "a", title: "A" });
  const b = testNotification({ key: "b", title: "B" });

  it("adds a new notification", () => {
    expect(applyEvent([a], { type: "posted", notification: b, alert: false })).toEqual([a, b]);
  });

  it("replaces an updated notification in place", () => {
    const updated = { ...a, title: "A2" };
    expect(applyEvent([a, b], { type: "posted", notification: updated, alert: true })).toEqual([
      updated,
      b,
    ]);
  });

  it("removes by key and ignores unknown keys", () => {
    expect(applyEvent([a, b], { type: "removed", key: "a" })).toEqual([b]);
    expect(applyEvent([a], { type: "removed", key: "zzz" })).toEqual([a]);
  });

  it("replaces everything on reset", () => {
    expect(applyEvent([a], { type: "reset", notifications: [b] })).toEqual([b]);
  });
});

describe("groupNotifications", () => {
  it("orders groups and their items newest first", () => {
    const groups = groupNotifications([
      testNotification({ key: "1", group: "g", whenUnixMs: 10 }),
      testNotification({ key: "2", whenUnixMs: 30 }),
      testNotification({ key: "3", group: "g", whenUnixMs: 20 }),
    ]);

    expect(groups.map((g) => g.items.map((n) => n.key))).toEqual([["2"], ["3", "1"]]);
    expect(groups[1]?.first.key).toBe("3");
  });

  it("never merges notifications without a group", () => {
    const groups = groupNotifications([
      testNotification({ key: "1" }),
      testNotification({ key: "2" }),
    ]);
    expect(groups).toHaveLength(2);
  });
});
