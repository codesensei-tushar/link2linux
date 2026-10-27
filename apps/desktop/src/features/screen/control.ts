import type { ScreenInput, ScreenPoint } from "@/lib/api";

/** The most points in one gesture. */
export const MAX_POINTS = 64;
/** The longest gesture, in milliseconds. */
export const MAX_GESTURE_MS = 10_000;
/** How often a moving finger is sampled, in milliseconds. */
export const SAMPLE_MS = 16;
/** A finger that stays at one point this long, in milliseconds, is a long press. */
export const LONG_PRESS_MS = 500;
/** How long a swipe made from the mouse wheel takes, in milliseconds. */
export const SWIPE_MS = 300;
/** How long a wheel swipe rests at its end, so the phone scrolls without flinging. */
export const SWIPE_REST_MS = 60;

/** A point in the frames' pixels. */
export interface FramePoint {
  x: number;
  y: number;
}

/** The size of the frames. */
export interface FrameSize {
  width: number;
  height: number;
}

/** Where `(clientX, clientY)` falls on frames of `size` shown in `rect`, kept inside them. */
export function toFramePoint(
  clientX: number,
  clientY: number,
  rect: { left: number; top: number; width: number; height: number },
  size: FrameSize,
): FramePoint {
  const scale = (value: number, start: number, shown: number, side: number) =>
    shown > 0 ? clamp(Math.floor(((value - start) / shown) * side), 0, side - 1) : 0;
  return {
    x: scale(clientX, rect.left, rect.width, size.width),
    y: scale(clientY, rect.top, rect.height, size.height),
  };
}

/**
 * Follows one finger, from the press through its moves to its release, as one gesture: at most
 * one point every {@link SAMPLE_MS}, for at most {@link MAX_GESTURE_MS}. A press that does not
 * move is a tap, or a long press when held for {@link LONG_PRESS_MS} or more.
 */
export class GestureRecorder {
  private readonly points: ScreenPoint[] = [];
  private readonly start: number;

  constructor(
    private readonly formatId: number,
    at: FramePoint,
    time: number,
  ) {
    this.start = time;
    this.points.push({ x: at.x, y: at.y, timeMs: 0 });
  }

  /** The finger moved to `at` at `time` (milliseconds, as event time stamps are). */
  move(at: FramePoint, time: number): void {
    const elapsed = this.elapsed(time);
    const last = this.last();
    if (elapsed - last.timeMs < SAMPLE_MS || elapsed >= MAX_GESTURE_MS) return;
    if (at.x === last.x && at.y === last.y) return;
    this.points.push({ x: at.x, y: at.y, timeMs: elapsed });
  }

  /** The finger lifted at `at`: the gesture to send. */
  up(at: FramePoint, time: number): ScreenInput {
    const elapsed = this.elapsed(time);
    const last = this.last();
    if (at.x !== last.x || at.y !== last.y || elapsed > last.timeMs) {
      this.points.push({ x: at.x, y: at.y, timeMs: Math.max(elapsed, last.timeMs) });
    }
    const first = this.points[0] ?? { x: at.x, y: at.y, timeMs: 0 };
    const still = this.points.every((point) => point.x === first.x && point.y === first.y);
    let points = this.points;
    if (still) {
      // A tap is one point; a long press stays at its point until it lifts.
      points = elapsed < LONG_PRESS_MS ? [first] : [first, { ...first, timeMs: elapsed }];
    }
    return { kind: "gesture", formatId: this.formatId, points: thin(points) };
  }

  private elapsed(time: number): number {
    return clamp(Math.round(time - this.start), 0, MAX_GESTURE_MS);
  }

  private last(): ScreenPoint {
    return this.points[this.points.length - 1] ?? { x: 0, y: 0, timeMs: 0 };
  }
}

/** At most {@link MAX_POINTS} of `points`: the first, the last and evenly spread ones between. */
export function thin(points: ScreenPoint[]): ScreenPoint[] {
  if (points.length <= MAX_POINTS) return points;
  const last = points.length - 1;
  return Array.from(
    { length: MAX_POINTS },
    (_, i) => points[Math.round((i * last) / (MAX_POINTS - 1))] ?? points[last],
  ).filter((point): point is ScreenPoint => point !== undefined);
}

/**
 * The swipe that scrolls like a turn of the mouse wheel by `delta` (in the frames' pixels) over
 * `at`: the finger moves against the wheel, along the axis that moved most, and rests at its end
 * so the phone scrolls without flinging. It stays inside the frame. Null for no movement.
 */
export function wheelSwipe(
  formatId: number,
  size: FrameSize,
  at: FramePoint,
  delta: FramePoint,
): ScreenInput | null {
  const vertical = Math.abs(delta.y) >= Math.abs(delta.x);
  const amount = vertical ? delta.y : delta.x;
  if (amount === 0) return null;
  const side = vertical ? size.height : size.width;
  const distance = Math.round(clamp(Math.abs(amount), side * 0.05, side * 0.3));
  // Scrolling down moves the content up: the finger goes up.
  const direction = amount > 0 ? -1 : 1;
  const along = vertical ? at.y : at.x;
  const from =
    direction < 0 ? clamp(along, distance, side - 1) : clamp(along, 0, side - 1 - distance);
  const steps = 6;
  const points: ScreenPoint[] = [];
  for (let i = 0; i < steps; i++) {
    const position = Math.round(from + (direction * distance * i) / (steps - 1));
    const timeMs = Math.round((SWIPE_MS * i) / (steps - 1));
    points.push(vertical ? { x: at.x, y: position, timeMs } : { x: position, y: at.y, timeMs });
  }
  const end = points[points.length - 1];
  if (end) points.push({ ...end, timeMs: SWIPE_MS + SWIPE_REST_MS });
  return { kind: "gesture", formatId, points };
}

/** The phone key for a desktop key the text field does not type, or null. */
export function keyInput(key: string): ScreenInput | null {
  switch (key) {
    case "Backspace":
      return { kind: "key", key: "backspace" };
    case "Enter":
      return { kind: "key", key: "enter" };
    default:
      return null;
  }
}

function clamp(value: number, low: number, high: number): number {
  return Math.min(Math.max(value, low), Math.max(low, high));
}
