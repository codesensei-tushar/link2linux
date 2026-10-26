import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AppProviders } from "@/app/providers";
import type { PairingFailure, PairingState } from "@/lib/api";
import { createFakeApi, testDevice, type FakeApi } from "@/test/fake-api";
import { qrOffer, sasCode, searching } from "@/test/pairing-fixtures";

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

describe("PairingPage: code comparison", () => {
  it("lists phones, waits while none are found, and dials the chosen one", async () => {
    const user = userEvent.setup();
    const api = await renderPage(createFakeApi(), searching());
    expect(screen.getByRole("heading", { name: "Choose your phone" })).toHaveFocus();
    expect(screen.getByRole("status")).toHaveTextContent("Looking for phones on this network");

    api.emitPairing(
      searching({
        phones: [
          { id: "a", name: "Pixel 9", address: "192.168.1.42:27380" },
          { id: "b", name: "Galaxy S24", address: "192.168.1.57:27380" },
        ],
      }),
    );
    const list = screen.getByRole("list", { name: "Phones ready to pair" });
    expect(within(list).getAllByRole("button")).toHaveLength(2);

    await user.click(within(list).getByRole("button", { name: /Galaxy S24/ }));
    expect(api.pairWith).toHaveBeenCalledExactlyOnceWith({ kind: "discovered", id: "b" });
  });

  it("explains the last failure and how many tries are left", async () => {
    await renderPage(
      createFakeApi(),
      searching({ lastFailure: "verificationFailed", attemptsLeft: 1 }),
    );
    expect(screen.getByRole("alert")).toHaveTextContent("The codes didn't match");
    expect(screen.getByText("You have 1 try left.")).toBeInTheDocument();
  });

  it("offers the typed address and a QR code here too", async () => {
    const user = userEvent.setup();
    const api = await renderPage(createFakeApi(), searching({ attemptsLeft: 2 }));
    expect(screen.getByText("You have 2 tries left.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Type the phone's address instead" })).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Scan a QR code instead" }));
    expect(api.startPairing).toHaveBeenCalledExactlyOnceWith("qrCode");
  });

  it("shows progress while connecting and verifying", async () => {
    const api = await renderPage();
    api.emitPairing({ step: "connecting", method: "sas", phoneName: "Pixel 9" });
    expect(screen.getByRole("heading", { name: "Connecting to Pixel 9" })).toHaveFocus();
    expect(screen.getByRole("status")).toHaveTextContent("Opening a secure connection");

    api.emitPairing({ step: "verifying", method: "qrCode", phoneName: null });
    expect(screen.getByRole("heading", { name: "Checking the connection" })).toHaveFocus();
    expect(screen.getByRole("status")).toHaveTextContent("Making sure your phone is the phone");
  });

  it("shows the code grouped, readable by screen readers, and sends the answer", async () => {
    const user = userEvent.setup();
    const api = await renderPage(createFakeApi(), sasCode());

    expect(screen.getByRole("heading", { name: "Compare the codes with Pixel 9" })).toHaveFocus();
    expect(screen.getByLabelText("Code: 4 8 2 9 1 3")).toHaveTextContent("482 913");
    expect(screen.getByRole("timer")).toHaveTextContent("Code valid for 0:52");

    await user.click(screen.getByRole("button", { name: "Codes match" }));
    expect(api.answerCode).toHaveBeenLastCalledWith(true);
    await user.click(screen.getByRole("button", { name: "Codes don't match" }));
    expect(api.answerCode).toHaveBeenLastCalledWith(false);
  });

  it("waits for the phone once confirmed here", async () => {
    await renderPage(createFakeApi(), sasCode({ confirmedHere: true }));
    expect(screen.getByRole("status")).toHaveTextContent("Waiting for Pixel 9 to confirm");
    expect(screen.queryByRole("button", { name: "Codes match" })).not.toBeInTheDocument();
  });
});

describe("PairingPage: finished", () => {
  it("celebrates success and dismisses on Done", async () => {
    const user = userEvent.setup();
    const api = await renderPage();
    api.emitPairing({
      step: "paired",
      device: testDevice({ name: "Pixel 9", status: "connected" }),
    });

    expect(screen.getByRole("heading", { name: "Paired with Pixel 9" })).toHaveFocus();
    await user.click(screen.getByRole("button", { name: "Done" }));
    expect(api.cancelPairing).toHaveBeenCalledOnce();
  });

  const reasons: [PairingFailure, string][] = [
    ["rejected", "The phone refused the connection"],
    ["declinedByPhone", "Pairing was declined on the phone"],
    ["verificationFailed", "The codes didn't match"],
    ["timedOut", "Pairing took too long"],
    ["notExpected", "The phone wasn't expecting this computer"],
    ["unsupported", "The apps can't pair with each other"],
    ["expired", "The code expired"],
    ["phoneNotFound", "Couldn't reach the phone"],
    ["connectionLost", "The connection dropped"],
    ["tooManyAttempts", "Too many tries"],
  ];

  it.each(reasons)("explains %s", async (reason, title) => {
    const api = await renderPage();
    api.emitPairing({ step: "failed", method: "sas", reason });

    expect(screen.getByRole("heading", { name: title })).toHaveFocus();
    expect(screen.getByRole("alert")).not.toBeEmptyDOMElement();
  });

  it("restarts the same method, offers the other, and closes", async () => {
    const user = userEvent.setup();
    const api = await renderPage(createFakeApi(), {
      step: "failed",
      method: "qrCode",
      reason: "expired",
    });

    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(api.startPairing).toHaveBeenLastCalledWith("qrCode");
    await user.click(screen.getByRole("button", { name: "Pair with a code instead" }));
    expect(api.startPairing).toHaveBeenLastCalledWith("sas");
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(api.cancelPairing).toHaveBeenCalledTimes(3);
  });

  it("asks to start again after too many tries", async () => {
    await renderPage(createFakeApi(), {
      step: "failed",
      method: "sas",
      reason: "tooManyAttempts",
    });
    expect(screen.getByRole("button", { name: "Start again" })).toBeInTheDocument();
  });
});
