import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";

import { DevicesProvider } from "@/features/pairing/DevicesProvider";
import { DesktopApiContext, type Device, type FileTransfer } from "@/lib/api";
import { createFakeApi, testDevice, testTransfer, type FakeApi } from "@/test/fake-api";

import { FilesPage } from "./FilesPage";

function renderPage(
  transfers: FileTransfer[] = [],
  devices: Device[] = [testDevice({ id: "phone-1", name: "Pixel", status: "connected" })],
  api: FakeApi = createFakeApi(),
  routeState?: { deviceId: string },
) {
  api.listDevices.mockResolvedValue(devices);
  api.listTransfers.mockResolvedValue(transfers);
  render(
    <DesktopApiContext value={api}>
      <DevicesProvider>
        <MemoryRouter initialEntries={[{ pathname: "/files", state: routeState }]}>
          <FilesPage />
        </MemoryRouter>
      </DevicesProvider>
    </DesktopApiContext>,
  );
  return api;
}

const photo = { name: "photo.jpg", size: 2_000_000, mimeType: "image/jpeg", saved: true };

afterEach(() => {
  vi.useRealTimers();
});

describe("FilesPage", () => {
  it("lists transfers newest first with their state and size", async () => {
    renderPage([
      testTransfer({
        id: "old",
        direction: "incoming",
        status: { kind: "completed" },
        files: [photo],
        done: 2_000_000,
        startedUnixMs: Date.now() - 3_600_000,
      }),
      testTransfer({
        id: "new",
        direction: "outgoing",
        status: { kind: "running" },
        files: [{ ...photo, name: "report.pdf", saved: false }],
        size: 4_000_000,
        done: 1_000_000,
      }),
    ]);

    const list = await screen.findByRole("list", { name: "Transfers with Pixel" });
    const sending = screen.getByRole("article", { name: "To Pixel: report.pdf" });
    const received = screen.getByRole("article", { name: "From Pixel: photo.jpg" });
    expect(within(list).getAllByRole("article")).toEqual([sending, received]);
    expect(sending).toHaveTextContent(/report\.pdf.*Sending… 25% · 1\.0 MB of 4\.0 MB/);
    expect(
      within(sending).getByRole("progressbar", { name: "Progress of report.pdf" }),
    ).toHaveAttribute("value", "1000000");
    expect(received).toHaveTextContent(/photo\.jpg.*Saved in Downloads · 2\.0 MB/);
    expect(within(received).queryByRole("progressbar")).toBeNull();
  });

  it("sends files through the file chooser", async () => {
    const api = renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Send files to Pixel" }));
    expect(api.pickFilesToSend).toHaveBeenCalledWith("phone-1");
    expect(screen.getByText("No transfers yet")).toBeInTheDocument();
  });

  it("follows progress, shows the speed, and announces the end", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(10_000);
    const running = testTransfer({ id: "t", size: 4_000_000, done: 0, files: [photo] });
    const api = renderPage([running]);
    await screen.findByText(/Receiving… 0%/);

    api.emitTransfer({ type: "progress", id: "t", done: 1_000_000 });
    vi.setSystemTime(11_000);
    api.emitTransfer({ type: "progress", id: "t", done: 2_000_000 });
    expect(screen.getByRole("article")).toHaveTextContent(
      "Receiving… 50% · 2.0 MB of 4.0 MB · 1.0 MB/s",
    );

    api.emitTransfer({
      type: "changed",
      transfer: { ...running, status: { kind: "completed" }, done: 4_000_000 },
    });
    expect(screen.getByRole("status")).toHaveTextContent("Received photo.jpg");
    expect(screen.getByRole("article")).not.toHaveTextContent("MB/s");
  });

  it("drops the speed when bytes stop, and measures afresh after a pause", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(10_000);
    const running = testTransfer({ id: "t", size: 8_000_000, done: 0, files: [photo] });
    const api = renderPage([running]);
    const card = await screen.findByRole("article", { name: "From Pixel: photo.jpg" });

    api.emitTransfer({ type: "progress", id: "t", done: 1_000_000 });
    vi.setSystemTime(11_000);
    api.emitTransfer({ type: "progress", id: "t", done: 2_000_000 });
    expect(card).toHaveTextContent("1.0 MB/s");

    // Nothing comes for a while: the page ticks and the speed goes.
    vi.setSystemTime(20_000);
    await waitFor(
      () => {
        expect(card).not.toHaveTextContent("MB/s");
      },
      { timeout: 3_000 },
    );

    // After a pause, the speed is measured from the new bytes only.
    const paused = { ...running, done: 2_000_000 };
    api.emitTransfer({ type: "changed", transfer: { ...paused, status: { kind: "interrupted" } } });
    vi.setSystemTime(100_000);
    api.emitTransfer({ type: "changed", transfer: paused });
    expect(card).not.toHaveTextContent("B/s");
    vi.setSystemTime(101_000);
    api.emitTransfer({ type: "progress", id: "t", done: 3_000_000 });
    expect(card).toHaveTextContent("Receiving… 37% · 3.0 MB of 8.0 MB · 1.0 MB/s");
  });

  it("announces every ending, also with the same words", async () => {
    const api = renderPage([]);
    await screen.findByText("No transfers yet");
    const status = screen.getByRole("status");
    const arrived = (id: string) => {
      api.emitTransfer({
        type: "changed",
        transfer: testTransfer({ id, status: { kind: "completed" }, files: [photo] }),
      });
    };
    arrived("one");
    const first = status.textContent;
    arrived("two");
    expect(status).toHaveTextContent("Received photo.jpg");
    expect(status.textContent).not.toBe(first);
  });

  it("keeps a removed transfer out of the list, unless it starts again", async () => {
    const done = testTransfer({ id: "t", status: { kind: "completed" }, files: [photo] });
    const api = renderPage([done]);
    await screen.findByRole("article", { name: "From Pixel: photo.jpg" });
    api.emitTransfer({ type: "removed", id: "t" });
    api.emitTransfer({
      type: "changed",
      transfer: { ...done, status: { kind: "cancelled", byPhone: false } },
    });
    api.emitTransfer({ type: "progress", id: "t", done: 1 });
    expect(screen.queryByRole("article")).toBeNull();

    api.emitTransfer({ type: "changed", transfer: { ...done, status: { kind: "running" } } });
    expect(screen.getByRole("article", { name: "From Pixel: photo.jpg" })).toBeInTheDocument();
  });

  it("opens, shows, cancels and removes transfers", async () => {
    const api = renderPage([
      testTransfer({ id: "running", files: [{ ...photo, name: "big.iso", saved: false }] }),
      testTransfer({
        id: "two",
        status: { kind: "completed" },
        files: [photo, { ...photo, name: "second.jpg" }],
      }),
    ]);
    const running = await screen.findByRole("article", { name: "From Pixel: big.iso" });
    expect(within(running).queryByRole("button", { name: "Open" })).toBeNull();
    await userEvent.click(within(running).getByRole("button", { name: "Cancel" }));
    expect(api.cancelTransfer).toHaveBeenCalledWith("phone-1", "running");

    const done = screen.getByRole("article", { name: "From Pixel: photo.jpg and 1 more" });
    expect(within(done).queryByRole("button", { name: "Cancel" })).toBeNull();
    await userEvent.click(within(done).getByRole("button", { name: "Open second.jpg" }));
    expect(api.openReceivedFile).toHaveBeenCalledWith("phone-1", "two", 1);
    await userEvent.click(within(done).getByRole("button", { name: "Show in folder" }));
    expect(api.showReceivedFile).toHaveBeenCalledWith("phone-1", "two", 0);
    await userEvent.click(within(done).getByRole("button", { name: "Remove from list" }));
    expect(api.forgetTransfer).toHaveBeenCalledWith("phone-1", "two");

    api.emitTransfer({ type: "removed", id: "two" });
    expect(screen.queryByRole("article", { name: /photo\.jpg and 1 more/ })).toBeNull();
  });

  it("says when an action fails", async () => {
    const api = createFakeApi();
    api.openReceivedFile.mockRejectedValue(new Error("No application opens it."));
    renderPage(
      [testTransfer({ id: "one", status: { kind: "completed" }, files: [photo] })],
      undefined,
      api,
    );
    await userEvent.click(await screen.findByRole("button", { name: "Open" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Couldn't open the file: No application opens it.",
    );
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows events that arrive before the list", async () => {
    const api = createFakeApi();
    let resolve: (list: FileTransfer[]) => void = () => undefined;
    api.listTransfers.mockReturnValue(
      new Promise((r) => {
        resolve = r;
      }),
    );
    api.listDevices.mockResolvedValue([testDevice({ status: "connected", name: "Pixel" })]);
    render(
      <DesktopApiContext value={api}>
        <DevicesProvider>
          <MemoryRouter>
            <FilesPage />
          </MemoryRouter>
        </DevicesProvider>
      </DesktopApiContext>,
    );
    await waitFor(() => {
      expect(api.subscribeTransfers).toHaveBeenCalled();
    });
    api.emitTransfer({ type: "changed", transfer: testTransfer({ id: "late", files: [photo] }) });
    act(() => {
      resolve([]);
    });
    expect(await screen.findByRole("article", { name: /photo\.jpg/ })).toBeInTheDocument();
  });

  it("lets the user pick the phone when several are paired", async () => {
    const api = renderPage(
      [],
      [
        testDevice({ id: "a", name: "Pixel", status: "disconnected" }),
        testDevice({ id: "b", name: "Galaxy", status: "connected" }),
        testDevice({ id: "c", name: "Gone", status: "revoked" }),
      ],
    );
    const picker = await screen.findByRole("group", { name: "Phone" });
    expect(within(picker).getByRole("radio", { name: /Galaxy/ })).toBeChecked();
    expect(within(picker).queryByRole("radio", { name: /Gone/ })).toBeNull();
    await waitFor(() => {
      expect(api.listTransfers).toHaveBeenLastCalledWith("b");
    });

    await userEvent.click(within(picker).getByRole("radio", { name: /Pixel/ }));
    await waitFor(() => {
      expect(api.listTransfers).toHaveBeenLastCalledWith("a");
    });
    expect(
      await screen.findByText("Pixel isn't connected: files wait up to an hour for it."),
    ).toBeInTheDocument();
  });

  it("opens on the phone files were just dropped for", async () => {
    const api = renderPage(
      [],
      [
        testDevice({ id: "a", name: "Pixel", status: "connected" }),
        testDevice({ id: "b", name: "Galaxy", status: "connected" }),
      ],
      undefined,
      { deviceId: "b" },
    );
    expect(await screen.findByRole("radio", { name: /Galaxy/ })).toBeChecked();
    await waitFor(() => {
      expect(api.listTransfers).toHaveBeenLastCalledWith("b");
    });
  });

  it("asks for a phone when none is paired", async () => {
    renderPage([], []);
    expect(await screen.findByText("No phone paired")).toBeInTheDocument();
  });
});
