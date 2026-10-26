import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Countdown } from "./Countdown";
import { formatRemaining } from "./format-time";

afterEach(() => {
  vi.useRealTimers();
});

describe("formatRemaining", () => {
  it("shows minutes and seconds, rounding up", () => {
    expect(formatRemaining(300_000)).toBe("5:00");
    expect(formatRemaining(272_000)).toBe("4:32");
    expect(formatRemaining(59_001)).toBe("1:00");
    expect(formatRemaining(1)).toBe("0:01");
    expect(formatRemaining(0)).toBe("0:00");
    expect(formatRemaining(-5)).toBe("0:00");
  });
});

describe("Countdown", () => {
  it("runs down to zero and stops there", () => {
    vi.useFakeTimers();
    render(<Countdown expiresInMs={2_000} lifetimeMs={4_000} label="Left" />);
    expect(screen.getByRole("timer")).toHaveTextContent("Left 0:02");

    act(() => {
      vi.advanceTimersByTime(1_000);
    });
    expect(screen.getByRole("timer")).toHaveTextContent("Left 0:01");
    act(() => {
      vi.advanceTimersByTime(5_000);
    });
    expect(screen.getByRole("timer")).toHaveTextContent("Left 0:00");
  });

  it("restarts when remounted with a new key", () => {
    vi.useFakeTimers();
    const { rerender } = render(
      <Countdown key="a" expiresInMs={5_000} lifetimeMs={5_000} label="L" />,
    );
    act(() => {
      vi.advanceTimersByTime(3_000);
    });
    expect(screen.getByRole("timer")).toHaveTextContent("0:02");
    rerender(<Countdown key="b" expiresInMs={5_000} lifetimeMs={5_000} label="L" />);
    expect(screen.getByRole("timer")).toHaveTextContent("0:05");
  });
});
