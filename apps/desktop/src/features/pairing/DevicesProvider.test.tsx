import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { DesktopApiContext, type DesktopApi } from "@/lib/api";
import { createFakeApi, testDevice } from "@/test/fake-api";

import { DevicesProvider } from "./DevicesProvider";
import { useDevices } from "./devices-context";

function Probe() {
  const { devices, reload } = useDevices();
  return (
    <>
      <output>
        {devices.status === "ready"
          ? devices.data.map((d) => d.name).join(", ") || "none"
          : devices.status === "error"
            ? `error: ${devices.error}`
            : "loading"}
      </output>
      <button type="button" onClick={reload}>
        reload
      </button>
    </>
  );
}

function renderProbe(api: DesktopApi) {
  return render(
    <DesktopApiContext value={api}>
      <DevicesProvider>
        <Probe />
      </DevicesProvider>
    </DesktopApiContext>,
  );
}

describe("DevicesProvider", () => {
  it("loads paired phones from the backend", async () => {
    const api = createFakeApi();
    api.listDevices.mockResolvedValue([testDevice({ name: "Pixel" })]);
    renderProbe(api);

    expect(screen.getByRole("status")).toHaveTextContent("loading");
    expect(await screen.findByText("Pixel")).toBeInTheDocument();
  });

  it("reports a backend failure and recovers on reload", async () => {
    const api = createFakeApi();
    api.listDevices.mockRejectedValueOnce("backend unavailable");
    renderProbe(api);

    expect(await screen.findByText("error: backend unavailable")).toBeInTheDocument();

    act(() => {
      screen.getByRole("button", { name: "reload" }).click();
    });
    expect(await screen.findByText("none")).toBeInTheDocument();
    expect(api.listDevices).toHaveBeenCalledTimes(2);
  });

  it("throws a clear error when used outside the provider", () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined); // React logs the throw
    expect(() => render(<Probe />)).toThrow(/inside <DevicesProvider>/);
  });
});
