import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { DesktopApiContext } from "@/lib/api";
import { createFakeApi, testDevice, type FakeApi } from "@/test/fake-api";

import { DevicesProvider } from "./DevicesProvider";
import { PairingPage } from "./PairingPage";

function renderPage(api: FakeApi = createFakeApi()) {
  render(
    <DesktopApiContext value={api}>
      <DevicesProvider>
        <PairingPage />
      </DevicesProvider>
    </DesktopApiContext>,
  );
  return api;
}

describe("PairingPage", () => {
  it("shows the empty state with a primary action when no phone is paired", async () => {
    renderPage();

    expect(screen.getByRole("heading", { level: 1, name: "Pairing" })).toBeInTheDocument();
    expect(await screen.findByRole("region", { name: "No phone paired yet" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Pair a phone" })).toHaveClass("button--primary");
  });

  it("opens the pairing steps and returns focus when cancelled", async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(await screen.findByRole("button", { name: "Pair a phone" }));
    const title = screen.getByRole("heading", { name: "Pair a new phone" });
    expect(title).toHaveFocus();
    expect(screen.getByRole("note")).toHaveTextContent("isn't available in this build yet");

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("button", { name: "Pair a phone" })).toHaveFocus();
  });

  it("lists paired phones with their connection status", async () => {
    const api = createFakeApi();
    api.listDevices.mockResolvedValue([
      testDevice({ id: "a", name: "Work phone", model: "Pixel 9", status: "connected" }),
      testDevice({ id: "b", name: "Old phone" }),
    ]);
    renderPage(api);

    const list = await screen.findByRole("list", { name: "Paired phones" });
    const items = screen.getAllByRole("listitem");
    expect(list).toContainElement(items[0] ?? null);
    expect(items[0]).toHaveTextContent("Work phonePixel 9Connected");
    expect(items[1]).toHaveTextContent("Old phoneNot connected");
    expect(screen.getByRole("button", { name: "Pair another phone" })).toBeInTheDocument();
  });

  it("explains a loading failure and retries", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.listDevices.mockRejectedValueOnce(new Error("The desktop service did not answer."));
    renderPage(api);

    expect(await screen.findByText("The desktop service did not answer.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("region", { name: "No phone paired yet" })).toBeVisible();
  });
});
