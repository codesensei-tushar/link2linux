import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it } from "vitest";

import { DesktopApiContext } from "@/lib/api";
import { createFakeApi, testNotification, type FakeApi } from "@/test/fake-api";

import { useNotifications } from "./use-notifications";

function setup(api: FakeApi) {
  const wrapper = ({ children }: { children: ReactNode }) => (
    <DesktopApiContext value={api}>{children}</DesktopApiContext>
  );
  return renderHook(() => useNotifications("phone-1"), { wrapper });
}

describe("useNotifications", () => {
  it("applies events that arrive before the first list once it loads", async () => {
    const api = createFakeApi();
    let resolve: (list: ReturnType<typeof testNotification>[]) => void = () => undefined;
    api.listNotifications.mockReturnValue(
      new Promise((r) => {
        resolve = r;
      }),
    );
    const { result } = setup(api);

    act(() => {
      api.emitNotification({ type: "removed", key: "a" });
    });
    await act(async () => {
      resolve([testNotification({ key: "a" }), testNotification({ key: "b" })]);
      await Promise.resolve();
    });

    const { state } = result.current;
    expect(state.status === "ready" && state.notifications.map((n) => n.key)).toEqual(["b"]);
  });

  it("reloads after a failure", async () => {
    const api = createFakeApi();
    api.listNotifications.mockRejectedValueOnce(new Error("offline"));
    const { result } = setup(api);
    await waitFor(() => {
      expect(result.current.state).toEqual({ status: "error", error: "offline" });
    });

    act(() => {
      result.current.reload();
    });
    expect(result.current.state.status).toBe("loading");
    await waitFor(() => {
      expect(result.current.state.status).toBe("ready");
    });
    expect(api.listNotifications).toHaveBeenCalledTimes(2);
    expect(api.notificationSubscribers()).toBe(1);
  });
});
