import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { Unsubscribe } from "./api";
import { useLive } from "./use-live";

function setup() {
  let listener: ((value: string) => void) | undefined;
  const stop = vi.fn<Unsubscribe>();
  const subscribe = vi.fn((cb: (value: string) => void) => {
    listener = cb;
    return Promise.resolve(stop);
  });
  return {
    subscribe,
    stop,
    emit: (value: string) => {
      act(() => listener?.(value));
    },
  };
}

describe("useLive", () => {
  it("applies the snapshot when no event arrived", async () => {
    const { subscribe } = setup();
    const load = vi.fn().mockResolvedValue("snapshot");
    const { result } = renderHook(() => useLive(subscribe, load));

    expect(result.current[0]).toEqual({ status: "loading" });
    await waitFor(() => {
      expect(result.current[0]).toEqual({ status: "ready", data: "snapshot" });
    });
  });

  it("subscribes before reading the snapshot", async () => {
    const order: string[] = [];
    const subscribe = vi.fn(() => {
      order.push("subscribe");
      return Promise.resolve(() => undefined);
    });
    const load = vi.fn(() => {
      order.push("load");
      return Promise.resolve("snapshot");
    });
    renderHook(() => useLive(subscribe, load));

    await waitFor(() => {
      expect(order).toEqual(["subscribe", "load"]);
    });
  });

  it("ignores a snapshot that arrives after a newer event", async () => {
    const { subscribe, emit } = setup();
    let resolveLoad: (value: string) => void = () => undefined;
    const load = vi.fn(() => new Promise<string>((resolve) => (resolveLoad = resolve)));
    const { result } = renderHook(() => useLive(subscribe, load));
    await waitFor(() => {
      expect(load).toHaveBeenCalled();
    });

    emit("event");
    await act(() => {
      resolveLoad("stale snapshot");
      return Promise.resolve();
    });

    expect(result.current[0]).toEqual({ status: "ready", data: "event" });
  });

  it("reports a failed snapshot and recovers on reload", async () => {
    const { subscribe } = setup();
    const load = vi.fn().mockRejectedValueOnce("no backend").mockResolvedValue("back");
    const { result } = renderHook(() => useLive(subscribe, load));

    await waitFor(() => {
      expect(result.current[0]).toEqual({ status: "error", error: "no backend" });
    });
    act(() => {
      result.current[1]();
    });
    await waitFor(() => {
      expect(result.current[0]).toEqual({ status: "ready", data: "back" });
    });
  });

  it("stops listening on unmount, even if subscribing was still pending", async () => {
    const { subscribe, stop } = setup();
    const load = vi.fn().mockResolvedValue("x");
    const { unmount } = renderHook(() => useLive(subscribe, load));
    unmount();

    await waitFor(() => {
      expect(stop).toHaveBeenCalledTimes(1);
    });
    expect(load).not.toHaveBeenCalled();
  });
});
