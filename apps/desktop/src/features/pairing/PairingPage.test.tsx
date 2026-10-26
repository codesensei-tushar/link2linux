import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AppProviders } from "@/app/providers";
import type { PairingState } from "@/lib/api";
import { createFakeApi, type FakeApi } from "@/test/fake-api";
import { qrOffer } from "@/test/pairing-fixtures";

import { PairingPage } from "./PairingPage";

async function renderPage(api: FakeApi = createFakeApi(), initial?: PairingState) {
  if (initial) api.getPairingState.mockResolvedValue(initial);
  render(
    <AppProviders api={api}>
      <PairingPage />
    </AppProviders>,
  );
  // Let the subscriptions and snapshots settle.
  await act(async () => {
    await Promise.resolve();
  });
  return api;
}

afterEach(() => {
  vi.useRealTimers();
});

describe("PairingPage: start", () => {
  it("shows the empty state and starts QR pairing from its primary action", async () => {
    const user = userEvent.setup();
    const api = await renderPage();

    expect(screen.getByRole("heading", { level: 1, name: "Pairing" })).toBeInTheDocument();
    expect(await screen.findByRole("region", { name: "No phone paired yet" })).toBeVisible();
    const start = screen.getByRole("button", { name: "Pair a phone" });
    expect(start).toHaveClass("button--primary");

    await user.click(start);
    expect(api.startPairing).toHaveBeenCalledExactlyOnceWith("qrCode");
  });

  it("shows a refused start and stays on the same screen", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.startPairing.mockRejectedValueOnce(new Error("That isn't possible right now."));
    await renderPage(api);

    await user.click(await screen.findByRole("button", { name: "Pair a phone" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("That isn't possible right now.");
  });

  it("picks up a pairing already under way when the page opens", async () => {
    await renderPage(createFakeApi(), qrOffer());
    expect(screen.getByRole("heading", { name: "Scan with your phone" })).toBeInTheDocument();
  });
});

describe("PairingPage: QR code", () => {
  it("shows the code, the phone's view of this computer and a countdown", async () => {
    const api = await renderPage();
    api.emitPairing(qrOffer({ expiresInMs: 272_000 }));

    const title = screen.getByRole("heading", { name: "Scan with your phone" });
    expect(title).toHaveFocus();
    const image = screen.getByRole("img", { name: "QR code for pairing with Desk" });
    expect(image.getAttribute("src")).toMatch(/^data:image\/svg\+xml/);
    expect(screen.getByRole("timer")).toHaveTextContent("New code in 4:32");
    expect(screen.getByText("Desk", { selector: "strong" })).toBeInTheDocument();
  });

  it("counts down locally and shows a renewed code without moving focus", async () => {
    vi.useFakeTimers();
    const api = await renderPage();
    api.emitPairing(qrOffer({ expiresInMs: 30_000 }));
    const title = screen.getByRole("heading", { name: "Scan with your phone" });
    expect(screen.getByRole("timer")).toHaveTextContent("0:30");

    act(() => {
      vi.advanceTimersByTime(10_000);
    });
    expect(screen.getByRole("timer")).toHaveTextContent("0:20");

    const input = screen.getByRole("button", { name: "Type the phone's address instead" });
    input.focus();
    api.emitPairing(qrOffer({ uri: "link2linux://pair?v=1&n=Desk&x=2", expiresInMs: 300_000 }));
    expect(screen.getByRole("timer")).toHaveTextContent("5:00");
    expect(screen.getByRole("status", { hidden: true })).toHaveTextContent(
      "A new code is showing.",
    );
    expect(title).not.toHaveFocus();
    expect(input).toHaveFocus();
  });

  it("switches to code comparison with 'Can't scan?'", async () => {
    const user = userEvent.setup();
    const api = await renderPage(createFakeApi(), qrOffer());

    await user.click(screen.getByRole("button", { name: "Can't scan? Pair with a code" }));
    expect(api.cancelPairing).toHaveBeenCalledOnce();
    expect(api.startPairing).toHaveBeenCalledExactlyOnceWith("sas");
  });

  it("lets the user type an address and shows why one was refused", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.pairWith.mockRejectedValueOnce(new Error("Enter an address like 192.168.1.20."));
    await renderPage(api, qrOffer());

    await user.click(screen.getByRole("button", { name: "Type the phone's address instead" }));
    const field = screen.getByLabelText("Phone address");
    expect(field).toHaveFocus();
    expect(screen.getByRole("button", { name: "Connect" })).toBeDisabled();

    await user.type(field, " nope ");
    await user.click(screen.getByRole("button", { name: "Connect" }));
    expect(api.pairWith).toHaveBeenLastCalledWith({ kind: "address", address: "nope" });
    expect(await screen.findByRole("alert")).toHaveTextContent("Enter an address like");
    expect(field).toHaveAttribute("aria-invalid", "true");

    await user.clear(field);
    await user.type(field, "192.168.1.20:27380{Enter}");
    expect(api.pairWith).toHaveBeenLastCalledWith({
      kind: "address",
      address: "192.168.1.20:27380",
    });
  });

  it("cancels and returns focus to the button that started pairing", async () => {
    const user = userEvent.setup();
    const api = await renderPage();
    await user.click(await screen.findByRole("button", { name: "Pair a phone" }));
    api.emitPairing(qrOffer());

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(api.cancelPairing).toHaveBeenCalledOnce();
    api.emitPairing({ step: "idle" });
    expect(screen.getByRole("button", { name: "Pair a phone" })).toHaveFocus();
  });
});
