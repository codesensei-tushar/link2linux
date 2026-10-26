import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { describe, expect, it } from "vitest";

import { DevicesProvider } from "@/features/pairing/DevicesProvider";
import { DesktopApiContext, type Device } from "@/lib/api";
import { createFakeApi, testDevice, type FakeApi } from "@/test/fake-api";

import { FileDropOverlay } from "./FileDropOverlay";

function FilesRoute() {
  const location = useLocation();
  return <p>Files page for {(location.state as { deviceId: string }).deviceId}</p>;
}

/** Renders the overlay; `devices` null leaves the device list as `api` answers it. */
async function renderOverlay(devices: Device[] | null, api: FakeApi = createFakeApi()) {
  if (devices) api.listDevices.mockResolvedValue(devices);
  render(
    <DesktopApiContext value={api}>
      <DevicesProvider>
        <MemoryRouter initialEntries={["/notifications"]}>
          <Routes>
            <Route path="/files" element={<FilesRoute />} />
            <Route path="*" element={<p>Somewhere else</p>} />
          </Routes>
          <FileDropOverlay />
        </MemoryRouter>
      </DevicesProvider>
    </DesktopApiContext>,
  );
  await waitFor(() => {
    expect(api.listDevices).toHaveBeenCalled();
  });
  await waitFor(() => {
    expect(api.listenerCount()).toBeGreaterThanOrEqual(2);
  });
  return api;
}

describe("FileDropOverlay", () => {
  it("shows where dragged files will go", async () => {
    const api = await renderOverlay([testDevice({ id: "a", name: "Pixel", status: "connected" })]);
    await screen.findByText("Somewhere else");
    api.emitFileDrag(true);
    expect(await screen.findByText("Drop to send to Pixel")).toBeInTheDocument();
    api.emitFileDrag(false);
    expect(screen.queryByText("Drop to send to Pixel")).toBeNull();
  });

  it("sends a drop to the only connected phone and opens the files page", async () => {
    const api = await renderOverlay([
      testDevice({ id: "a", name: "Pixel", status: "connected" }),
      testDevice({ id: "b", name: "Galaxy", status: "disconnected" }),
    ]);
    await screen.findByText("Somewhere else");
    api.emitFilesDropped({ dropId: 4, names: ["a.txt"] });
    expect(await screen.findByText("Files page for a")).toBeInTheDocument();
    expect(api.sendDroppedFiles).toHaveBeenCalledWith(4, "a");
  });

  it("asks which phone when it is not obvious", async () => {
    const api = await renderOverlay([
      testDevice({ id: "a", name: "Pixel", status: "connected" }),
      testDevice({ id: "b", name: "Galaxy", status: "connected" }),
    ]);
    await screen.findByText("Somewhere else");
    api.emitFilesDropped({ dropId: 5, names: ["a.txt", "b.txt"] });
    const dialog = await screen.findByRole("dialog", { name: "Send 2 files to which phone?" });
    expect(dialog).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Galaxy" }));
    expect(api.sendDroppedFiles).toHaveBeenCalledWith(5, "b");
    expect(await screen.findByText("Files page for b")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("can be cancelled with Escape", async () => {
    const api = await renderOverlay([
      testDevice({ id: "a", name: "Pixel", status: "disconnected" }),
      testDevice({ id: "b", name: "Galaxy", status: "disconnected" }),
    ]);
    await screen.findByText("Somewhere else");
    api.emitFilesDropped({ dropId: 6, names: ["a.txt"] });
    await screen.findByRole("dialog", { name: "Send a.txt to which phone?" });
    expect(screen.getByRole("button", { name: "Pixel, sends when it connects" })).toHaveFocus();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(api.sendDroppedFiles).not.toHaveBeenCalled();
  });

  it("says why files could not be sent", async () => {
    const api = createFakeApi();
    api.sendDroppedFiles.mockRejectedValue(
      new Error("Folders can't be sent yet. Send the files in them instead."),
    );
    await renderOverlay([testDevice({ id: "a", status: "connected" })], api);
    await screen.findByText("Somewhere else");
    api.emitFilesDropped({ dropId: 7, names: ["Photos"] });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Couldn't send the files: Folders can't be sent yet.",
    );
    expect(screen.getByText("Somewhere else")).toBeInTheDocument();
  });

  it("asks to pair a phone first", async () => {
    const api = await renderOverlay([]);
    await screen.findByText("Somewhere else");
    api.emitFilesDropped({ dropId: 8, names: ["a.txt"] });
    expect(await screen.findByRole("alert")).toHaveTextContent("Pair a phone to send files to it.");
  });

  it("says the phones are still being looked for", async () => {
    const api = createFakeApi();
    api.listDevices.mockReturnValue(new Promise(() => undefined));
    await renderOverlay(null, api);
    api.emitFilesDropped({ dropId: 9, names: ["a.txt"] });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Still looking for your phones. Drop the files again in a moment.",
    );
    expect(api.sendDroppedFiles).not.toHaveBeenCalled();
  });

  it("says when the phones could not be listed", async () => {
    const api = createFakeApi();
    api.listDevices.mockRejectedValue(new Error("no runtime"));
    await renderOverlay(null, api);
    api.emitFilesDropped({ dropId: 9, names: ["a.txt"] });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Your phones couldn't be listed, so the files weren't sent.",
    );
  });

  it("closes the question when the phones go away, and still closes by keyboard", async () => {
    const pixel = testDevice({ id: "a", name: "Pixel", status: "disconnected" });
    const galaxy = testDevice({ id: "b", name: "Galaxy", status: "disconnected" });
    const api = await renderOverlay([pixel, galaxy]);
    await screen.findByText("Somewhere else");
    api.emitFilesDropped({ dropId: 10, names: ["a.txt"] });
    await screen.findByRole("dialog");
    expect(screen.getByRole("button", { name: "Pixel, sends when it connects" })).toHaveFocus();

    // The focused phone goes, and focus with it: Escape still closes.
    api.emitDevices([galaxy]);
    expect(screen.queryByRole("button", { name: /Pixel/ })).toBeNull();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();

    // Every phone goes while the question is open.
    api.emitDevices([pixel, galaxy]);
    api.emitFilesDropped({ dropId: 11, names: ["a.txt"] });
    await screen.findByRole("dialog");
    api.emitDevices([]);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Your phones are gone, so the files weren't sent.",
    );
    expect(api.sendDroppedFiles).not.toHaveBeenCalled();
  });

  it("says a newer drop replaced the one waiting for a phone", async () => {
    const api = await renderOverlay([
      testDevice({ id: "a", name: "Pixel", status: "connected" }),
      testDevice({ id: "b", name: "Galaxy", status: "connected" }),
    ]);
    await screen.findByText("Somewhere else");
    api.emitFilesDropped({ dropId: 12, names: ["first.txt"] });
    await screen.findByRole("dialog", { name: "Send first.txt to which phone?" });
    api.emitFilesDropped({ dropId: 13, names: ["second.txt"] });
    await screen.findByRole("dialog", { name: "Send second.txt to which phone?" });
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Only the newest files you dropped are kept.",
    );
    await userEvent.click(screen.getByRole("button", { name: "Galaxy" }));
    expect(api.sendDroppedFiles).toHaveBeenCalledWith(13, "b");
  });

  it("takes the overlay down when the window hears from the user, and asks to pair first", async () => {
    const api = await renderOverlay([]);
    await screen.findByText("Somewhere else");
    api.emitFileDrag(true);
    expect(await screen.findByText("Pair a phone to send files")).toBeInTheDocument();
    // The drag ended without the desktop saying so.
    fireEvent.pointerMove(window);
    expect(screen.queryByText("Pair a phone to send files")).toBeNull();
  });
});
