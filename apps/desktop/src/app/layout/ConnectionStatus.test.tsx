import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { AppProviders } from "@/app/providers";
import { createFakeApi, testDevice, type FakeApi } from "@/test/fake-api";

import { ConnectionStatus } from "./ConnectionStatus";

function renderStatus(api: FakeApi) {
  render(
    <AppProviders api={api}>
      <ConnectionStatus />
    </AppProviders>,
  );
}

describe("ConnectionStatus", () => {
  it("says no phone is paired and shows the app version", async () => {
    renderStatus(createFakeApi());

    expect(await screen.findByText("No phone paired")).toBeInTheDocument();
    expect(screen.getByText("Pair a phone to get started")).toBeInTheDocument();
    expect(await screen.findByText("link2linux 0.1.0")).toBeInTheDocument();
  });

  it("names a paired phone that is out of reach", async () => {
    const api = createFakeApi();
    api.listDevices.mockResolvedValue([testDevice({ name: "Pixel" })]);
    renderStatus(api);

    expect(await screen.findByText("Not connected")).toBeInTheDocument();
    expect(screen.getByText("Pixel is out of reach")).toBeInTheDocument();
  });

  it("prefers a connected phone over a disconnected one", async () => {
    const api = createFakeApi();
    api.listDevices.mockResolvedValue([
      testDevice({ id: "a", name: "Old phone" }),
      testDevice({ id: "b", name: "Pixel", status: "connected" }),
    ]);
    renderStatus(api);

    expect(await screen.findByText("Connected")).toBeInTheDocument();
    expect(screen.getByText("Pixel")).toBeInTheDocument();
  });

  it("reports when the backend does not answer, and announces changes", async () => {
    const api = createFakeApi();
    api.listDevices.mockRejectedValue(new Error("no backend"));
    renderStatus(api);

    expect(await screen.findByText("Unavailable")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("The desktop service didn't respond");
  });

  it("shows a phone that is reconnecting", async () => {
    const api = createFakeApi();
    api.listDevices.mockResolvedValue([testDevice({ name: "Pixel", status: "reconnecting" })]);
    renderStatus(api);

    expect(await screen.findByText("Reconnecting…")).toBeInTheDocument();
    expect(screen.getByText("Reaching Pixel")).toBeInTheDocument();
  });

  it("follows device events without reloading", async () => {
    const api = createFakeApi();
    api.listDevices.mockResolvedValue([testDevice({ name: "Pixel" })]);
    renderStatus(api);
    expect(await screen.findByText("Pixel is out of reach")).toBeInTheDocument();

    api.emitDevices([testDevice({ name: "Pixel", status: "connected" })]);
    expect(screen.getByRole("status")).toHaveTextContent("Connected");
    api.emitDevices([]);
    expect(screen.getByRole("status")).toHaveTextContent("No phone paired");
    expect(api.listDevices).toHaveBeenCalledOnce();
  });
});
