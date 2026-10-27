import { describe, expect, it } from "vitest";

import type { ScreenInput, ScreenPoint } from "@/lib/api";

import {
  GestureRecorder,
  MAX_GESTURE_MS,
  MAX_POINTS,
  SWIPE_MS,
  SWIPE_REST_MS,
  keyInput,
  thin,
  toFramePoint,
  wheelSwipe,
} from "./control";

const size = { width: 360, height: 780 };

function pointsOf(input: ScreenInput | null): ScreenPoint[] {
  if (input?.kind !== "gesture") throw new Error(`expected a gesture, got ${JSON.stringify(input)}`);
  return input.points;
}

describe("toFramePoint", () => {
  it("maps the shown screen onto the frames and keeps points inside them", () => {
    const rect = { left: 100, top: 50, width: 180, height: 390 };
    expect(toFramePoint(190, 245, rect, size)).toEqual({ x: 180, y: 390 });
    expect(toFramePoint(0, 0, rect, size)).toEqual({ x: 0, y: 0 });
    expect(toFramePoint(1000, 1000, rect, size)).toEqual({ x: 359, y: 779 });
    expect(toFramePoint(10, 10, { left: 0, top: 0, width: 0, height: 0 }, size)).toEqual({
      x: 0,
      y: 0,
    });
  });
});

describe("GestureRecorder", () => {
  it("makes a quick press in one place a tap of one point", () => {
    const gesture = new GestureRecorder(4, { x: 10, y: 20 }, 1000);
    gesture.move({ x: 10, y: 20 }, 1030);
    expect(gesture.up({ x: 10, y: 20 }, 1080)).toEqual({
      kind: "gesture",
      formatId: 4,
      points: [{ x: 10, y: 20, timeMs: 0 }],
    });
  });

  it("makes a held press a long press that stays at its point", () => {
    const gesture = new GestureRecorder(4, { x: 10, y: 20 }, 1000);
    expect(pointsOf(gesture.up({ x: 10, y: 20 }, 1700))).toEqual([
      { x: 10, y: 20, timeMs: 0 },
      { x: 10, y: 20, timeMs: 700 },
    ]);
  });

  it("samples a drag and ends where the finger lifted", () => {
    const gesture = new GestureRecorder(1, { x: 0, y: 0 }, 0);
    gesture.move({ x: 5, y: 5 }, 5); // too soon after the press
    gesture.move({ x: 10, y: 10 }, 20);
    gesture.move({ x: 20, y: 20 }, 40);
    expect(pointsOf(gesture.up({ x: 30, y: 30 }, 50))).toEqual([
      { x: 0, y: 0, timeMs: 0 },
      { x: 10, y: 10, timeMs: 20 },
      { x: 20, y: 20, timeMs: 40 },
      { x: 30, y: 30, timeMs: 50 },
    ]);
  });

  it("keeps a long drag within the limits", () => {
    const gesture = new GestureRecorder(1, { x: 0, y: 0 }, 0);
    for (let time = 16; time < 20_000; time += 16) gesture.move({ x: time % 300, y: 1 }, time);
    const points = pointsOf(gesture.up({ x: 7, y: 7 }, 20_000));
    expect(points.length).toBeLessThanOrEqual(MAX_POINTS);
    expect(points[0]).toEqual({ x: 0, y: 0, timeMs: 0 });
    expect(points.at(-1)).toEqual({ x: 7, y: 7, timeMs: MAX_GESTURE_MS });
    for (let i = 1; i < points.length; i++) {
      expect(points[i]?.timeMs).toBeGreaterThanOrEqual(points[i - 1]?.timeMs ?? 0);
    }
  });
});

describe("thin", () => {
  it("keeps the first and last of many points", () => {
    const points = Array.from({ length: 200 }, (_, i) => ({ x: i, y: i, timeMs: i }));
    const thinned = thin(points);
    expect(thinned).toHaveLength(MAX_POINTS);
    expect(thinned[0]?.x).toBe(0);
    expect(thinned.at(-1)?.x).toBe(199);
    expect(thin(points.slice(0, 3))).toHaveLength(3);
  });
});

describe("wheelSwipe", () => {
  it("moves the finger up for a turn down, and rests at the end", () => {
    const points = pointsOf(wheelSwipe(2, size, { x: 100, y: 400 }, { x: 0, y: 120 }));
    expect(points[0]).toEqual({ x: 100, y: 400, timeMs: 0 });
    const end = points.at(-2);
    expect(end).toEqual({ x: 100, y: 280, timeMs: SWIPE_MS });
    expect(points.at(-1)).toEqual({ x: 100, y: 280, timeMs: SWIPE_MS + SWIPE_REST_MS });
  });

  it("stays inside the frame and within a third of it", () => {
    const up = pointsOf(wheelSwipe(2, size, { x: 100, y: 10 }, { x: 0, y: 5000 }));
    expect(up.every((p) => p.y >= 0 && p.y < size.height)).toBe(true);
    expect((up[0]?.y ?? 0) - (up.at(-1)?.y ?? 0)).toBe(Math.round(size.height * 0.3));
    const down = pointsOf(wheelSwipe(2, size, { x: 100, y: 770 }, { x: 0, y: -50 }));
    expect(down.every((p) => p.y >= 0 && p.y < size.height)).toBe(true);
    expect(down.at(-1)?.y).toBeGreaterThan(down[0]?.y ?? 0);
  });

  it("swipes sideways for a sideways turn, and not at all for none", () => {
    const side = pointsOf(wheelSwipe(2, size, { x: 200, y: 400 }, { x: 60, y: 10 }));
    expect(side.every((p) => p.y === 400)).toBe(true);
    expect(side.at(-1)?.x).toBe(140);
    expect(wheelSwipe(2, size, { x: 1, y: 1 }, { x: 0, y: 0 })).toBeNull();
  });
});

describe("keyInput", () => {
  it("sends Backspace and Enter as the phone's keys", () => {
    expect(keyInput("Backspace")).toEqual({ kind: "key", key: "backspace" });
    expect(keyInput("Enter")).toEqual({ kind: "key", key: "enter" });
    expect(keyInput("a")).toBeNull();
  });
});
