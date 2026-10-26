import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { describe, expect, it } from "vitest";

import type { ClipboardActivity, ClipboardPhone, ClipboardStatus } from "@/lib/api";
import { DesktopApiContext } from "@/lib/api";
import { createFakeApi, testClipboardStatus, type FakeApi } from "@/test/fake-api";

import { ClipboardPage } from "./ClipboardPage";
import { problemText } from "./copy";

const now = Date.now();

const pixel: ClipboardPhone = { deviceId: "phone-1", name: "Pixel", state: "ready" };

function activity(overrides: Partial<ClipboardActivity> = {}): ClipboardActivity {
  return {
    deviceId: "phone-1",
    phoneName: "Pixel",
    direction: "sent",
    bytes: 12,
    sensitive: false,
    atUnixMs: now - 120_000,
    ...overrides,
  };
}

function renderPage(status: Partial<ClipboardStatus> = {}, api: FakeApi = createFakeApi()) {
  api.getClipboardStatus.mockResolvedValue(testClipboardStatus(status));
  render(
    <MemoryRouter>
      <DesktopApiContext value={api}>
        <ClipboardPage />
      </DesktopApiContext>
    </MemoryRouter>,
  );
  return api;
}

describe("ClipboardPage", () => {
  it("says how it shares and lists what went where, without the text", async () => {
    renderPage({
      phones: [pixel, { deviceId: "phone-2", name: "Galaxy", state: "turnedOff" }],
      activity: [
        activity({ direction: "received", bytes: 42, sensitive: true, atUnixMs: now - 5_000 }),
        activity({ bytes: 1_234, atUnixMs: now - 3_600_000 }),
      ],
    });

    expect(await screen.findByRole("heading", { name: "Shares automatically" })).toBeVisible();
    const items = within(screen.getByRole("list", { name: "Recent activity" })).getAllByRole(
      "listitem",
    );
    expect(items[0]).toHaveTextContent(/Received from Pixel.*42 bytes · password.*Just now/);
    expect(items[1]).toHaveTextContent(/Sent to Pixel.*1.2 KB.*1 h ago/);

    const phones = within(screen.getByRole("region", { name: "Phones" })).getAllByRole("listitem");
    expect(phones[0]).toHaveTextContent("PixelSharing");
    expect(phones[1]).toHaveTextContent("GalaxyOff on the phone");
  });

  it("explains a desktop that shares only while the window is focused", async () => {
    renderPage({
      mode: "whenFocused",
      phones: [pixel],
      held: { deviceId: "phone-1", phoneName: "Pixel", bytes: 2_048, sensitive: false },
    });

    expect(
      await screen.findByRole("heading", {
        name: "Shares when this window is focused or when you click Send",
      }),
    ).toBeVisible();
    expect(screen.getByText(/Text from Pixel \(2 KB\) is waiting/)).toBeVisible();
    expect(screen.getByText("Nothing shared yet", { exact: false })).toBeVisible();
  });

  it("sends the clipboard and shows what came of it", async () => {
    const user = userEvent.setup();
    const api = renderPage({ phones: [pixel] });

    await user.click(await screen.findByRole("button", { name: "Send clipboard to phone" }));
    expect(api.sendClipboard).toHaveBeenCalledOnce();

    api.emitClipboard(testClipboardStatus({ phones: [pixel], activity: [activity()] }));
    expect(await screen.findByText("Sent to Pixel")).toBeVisible();
  });

  it("says why the clipboard can't be sent", async () => {
    renderPage({ phones: [{ ...pixel, state: "disconnected" }] });

    const send = await screen.findByRole("button", { name: "Send clipboard to phone" });
    expect(send).toBeDisabled();
    expect(send).toHaveAccessibleDescription("No phone can take the clipboard right now.");
  });

  it("shows when a send failed", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.sendClipboard.mockRejectedValue(new Error("link2linux didn't start."));
    renderPage({ phones: [pixel] }, api);

    await user.click(await screen.findByRole("button", { name: "Send clipboard to phone" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("link2linux didn't start.");
  });

  it("points to the settings while sharing is off", async () => {
    const api = createFakeApi();
    api.getSettings.mockResolvedValue({ shareClipboard: false });
    renderPage({ phones: [pixel] }, api);

    expect(await screen.findByRole("heading", { name: "Clipboard sharing is off" })).toBeVisible();
    expect(screen.getByRole("link", { name: "Turn it on in Settings" })).toHaveAttribute(
      "href",
      "/settings",
    );
    expect(
      screen.getByRole("button", { name: "Send clipboard to phone" }),
    ).toHaveAccessibleDescription("Clipboard sharing is off.");
  });

  it("has no send button when the clipboard can't be reached", async () => {
    renderPage({ mode: "unavailable", phones: [pixel] });

    expect(
      await screen.findByRole("heading", { name: "The clipboard isn't available" }),
    ).toBeVisible();
    expect(screen.queryByRole("button", { name: "Send clipboard to phone" })).toBeNull();
  });

  it("says what went wrong last", async () => {
    renderPage({
      phones: [pixel],
      problem: {
        kind: "turnedOffOnPhone",
        deviceId: "phone-1",
        phoneName: "Pixel",
        bytes: null,
        atUnixMs: now - 60_000,
      },
    });

    expect(
      await screen.findByText(
        /Clipboard sharing is off on Pixel\. Turn it on in the link2linux app/,
      ),
    ).toHaveTextContent("1 min ago");
  });

  it("offers to try again when the clipboard doesn't load", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.getClipboardStatus.mockRejectedValueOnce(new Error("No backend."));
    render(
      <MemoryRouter>
        <DesktopApiContext value={api}>
          <ClipboardPage />
        </DesktopApiContext>
      </MemoryRouter>,
    );

    expect(await screen.findByText("Couldn't load the clipboard")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("heading", { name: "Shares automatically" })).toBeVisible();
  });
});

describe("problemText", () => {
  const problem = { deviceId: null, phoneName: null, bytes: null, atUnixMs: now };

  it.each([
    [{ kind: "tooLarge", bytes: 300_000 }, "too long to share (293 KB). Up to 256 KB"],
    [{ kind: "passwordNotShared" }, "turn on Share passwords in Settings"],
    [{ kind: "noText" }, "no text on the clipboard"],
    [{ kind: "sharingOff" }, "off on this computer"],
    [{ kind: "noPhone" }, "connects within a minute still gets it"],
    [{ kind: "phoneUnavailable", phoneName: "Pixel" }, "Pixel couldn't put the text"],
    [{ kind: "failed" }, "didn't reach your phone"],
  ] as const)("explains %o", (overrides, expected) => {
    expect(problemText({ ...problem, ...overrides })).toContain(expected);
  });
});
