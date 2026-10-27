import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { DevicesProvider } from "@/features/pairing/DevicesProvider";
import { DesktopApiContext, type Device, type ScreenFormat, type ScreenStatus } from "@/lib/api";
import { FakeVideoDecoder, fakeDecoderEnv } from "@/test/fake-decoder";
import { createFakeApi, testDevice, testScreen, type FakeApi } from "@/test/fake-api";

import type { DecoderEnv } from "./decoder";
import { encodePacket } from "./packet";
import { ScreenPage } from "./ScreenPage";
import { DecoderEnvContext } from "./use-screen";

const pixel = testDevice({ id: "phone-1", name: "Pixel", status: "connected" });
const jpegFormat: ScreenFormat = { id: 1, codec: "jpeg", width: 360, height: 780, input: true };

function renderPage(
  options: { status?: ScreenStatus; devices?: Device[]; env?: DecoderEnv } = {},
): { api: FakeApi; env: DecoderEnv; unmount: () => void } {
  const api = createFakeApi();
  const env = options.env ?? fakeDecoderEnv();
  api.listDevices.mockResolvedValue(options.devices ?? [pixel]);
  api.getScreenStatus.mockResolvedValue(options.status ?? testScreen());
  const { unmount } = render(
    <DesktopApiContext value={api}>
      <DecoderEnvContext value={env}>
        <DevicesProvider>
          <ScreenPage />
        </DevicesProvider>
      </DecoderEnvContext>
    </DesktopApiContext>,
  );
  return { api, env, unmount };
}

/** Starts the screen and lets it show `format`. */
async function showing(format: ScreenFormat = jpegFormat) {
  const page = renderPage();
  await userEvent.click(await screen.findByRole("button", { name: "Show Pixel's screen" }));
  page.api.emitScreen("phone-1", testScreen({ phase: "showing", format }));
  return page;
}

const drawImage = vi.fn();

beforeEach(() => {
  FakeVideoDecoder.reset();
  drawImage.mockReset();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({
    drawImage,
  } as unknown as CanvasRenderingContext2D);
  vi.spyOn(HTMLCanvasElement.prototype, "getBoundingClientRect").mockReturnValue({
    left: 0,
    top: 0,
    width: 180,
    height: 390,
  } as DOMRect);
  HTMLCanvasElement.prototype.setPointerCapture = vi.fn();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("ScreenPage", () => {
  it("asks for H.264 first where the webview decodes it", async () => {
    const { api } = renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Show Pixel's screen" }));
    expect(api.startScreen).toHaveBeenCalledWith(
      "phone-1",
      ["h264", "jpeg"],
      1920,
      expect.any(Function),
    );
    expect(screen.queryByText(/gstreamer1.0-libav/)).not.toBeInTheDocument();
  });

  it("asks only for JPEG without a decoder, and says how to get one", async () => {
    const { api } = renderPage({ env: fakeDecoderEnv(false) });
    await userEvent.click(await screen.findByRole("button", { name: "Show Pixel's screen" }));
    expect(api.startScreen).toHaveBeenCalledWith("phone-1", ["jpeg"], 1280, expect.any(Function));
    expect(screen.getByText(/gstreamer1.0-libav/)).toBeInTheDocument();
  });

  it("falls back to JPEG once when the webview cannot decode the phone's H.264", async () => {
    FakeVideoDecoder.support = new Map([["avc1.64001F", false]]);
    const { api } = await showing({ ...jpegFormat, codec: "h264", width: 720, height: 1560 });
    const h264 = encodePacket({
      keyFrame: true,
      formatId: 1,
      timestampUs: 0,
      format: {
        codec: "h264",
        width: 720,
        height: 1560,
        config: new Uint8Array([1, 0x64, 0, 0x1f, 0xff, 0xe1, 0]),
      },
      data: new Uint8Array([0, 0, 0, 1, 0x65]),
    });
    api.emitScreenPacket(h264);
    await waitFor(() => {
      expect(api.startScreen).toHaveBeenLastCalledWith(
        "phone-1",
        ["jpeg"],
        1280,
        expect.any(Function),
      );
    });

    // H.264 again, though only JPEG was asked for: the page gives up.
    api.emitScreenPacket(h264);
    await waitFor(() => {
      expect(api.stopScreen).toHaveBeenCalledWith("phone-1");
    });
    expect(screen.getByRole("alert")).toHaveTextContent("can't show the video Pixel sends");
    api.emitScreen("phone-1", testScreen());
    expect(screen.getByText(/gstreamer1.0-libav/)).toBeInTheDocument();
  });

  it("says the user has to agree on the phone, and can cancel", async () => {
    const { api } = renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Show Pixel's screen" }));
    api.emitScreen("phone-1", testScreen({ phase: "asking" }));
    expect(screen.getByRole("note")).toHaveTextContent(
      "Check Pixel: it asks whether to share its screen with this computer.",
    );
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(api.stopScreen).toHaveBeenCalledWith("phone-1");
  });

  it("draws the frames it gets", async () => {
    const { api, env } = await showing();
    api.emitScreenPacket(
      encodePacket({
        keyFrame: true,
        formatId: 1,
        timestampUs: 0,
        format: { codec: "jpeg", width: 360, height: 780, config: new Uint8Array(0) },
        data: new Uint8Array([0xff, 0xd8]),
      }),
    );
    expect(env.createImageBitmap).toHaveBeenCalledTimes(1);
    await waitFor(() => {
      expect(drawImage).toHaveBeenCalledTimes(1);
    });
    const canvas = screen.getByLabelText("Pixel's screen");
    expect((canvas as HTMLCanvasElement).width).toBe(360);
    expect((canvas as HTMLCanvasElement).height).toBe(780);
  });

  it("sends taps, keys and typing while the phone takes input", async () => {
    const { api } = await showing();
    const canvas = screen.getByLabelText("Pixel's screen");
    fireEvent.pointerDown(canvas, { clientX: 90, clientY: 195, button: 0, pointerId: 1 });
    fireEvent.pointerUp(canvas, { clientX: 90, clientY: 195, pointerId: 1 });
    await userEvent.click(screen.getByRole("button", { name: "Back" }));
    const typing = screen.getByRole("textbox", { name: "Type on Pixel" });
    await userEvent.type(typing, "hi{Backspace}{Enter}");

    await waitFor(() => {
      expect(api.sendScreenInput).toHaveBeenCalledTimes(5);
    });
    const sent = api.sendScreenInput.mock.calls.map(([, input]) => input);
    expect(sent[0]).toEqual({
      kind: "gesture",
      formatId: 1,
      points: [{ x: 180, y: 390, timeMs: 0 }],
    });
    // Quick typing goes as one text, before the key that follows it.
    expect(sent.slice(1)).toEqual([
      { kind: "key", key: "back" },
      { kind: "text", text: "hi" },
      { kind: "key", key: "backspace" },
      { kind: "key", key: "enter" },
    ]);
  });

  it("sends typed text once the typing pauses", async () => {
    const { api } = await showing();
    await userEvent.type(screen.getByRole("textbox", { name: "Type on Pixel" }), "ok");
    expect(api.sendScreenInput).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(api.sendScreenInput).toHaveBeenCalledWith("phone-1", { kind: "text", text: "ok" });
    });
    expect(api.sendScreenInput).toHaveBeenCalledTimes(1);
  });

  it("says when the phone's field doesn't take the typing", async () => {
    const { api } = await showing();
    api.emitScreen(
      "phone-1",
      testScreen({ phase: "showing", format: jpegFormat, inputProblem: "cannotType" }),
    );
    expect(screen.getByRole("alert")).toHaveTextContent("password field");
  });

  it("turns a wheel turn into one swipe", async () => {
    const { api } = await showing();
    vi.useFakeTimers();
    const canvas = screen.getByLabelText("Pixel's screen");
    fireEvent.wheel(canvas, { deltaY: 30, clientX: 90, clientY: 195 });
    fireEvent.wheel(canvas, { deltaY: 30, clientX: 90, clientY: 195 });
    act(() => {
      vi.advanceTimersByTime(100);
    });
    vi.useRealTimers();
    await waitFor(() => {
      expect(api.sendScreenInput).toHaveBeenCalledTimes(1);
    });
    const swipe = api.sendScreenInput.mock.calls[0]?.[1];
    expect(swipe?.kind === "gesture" && swipe.points.length).toBeGreaterThan(2);
  });

  it("offers no control while it is off on the phone", async () => {
    const { api } = await showing({ ...jpegFormat, input: false });
    expect(screen.getByRole("button", { name: "Back" })).toBeDisabled();
    expect(screen.getByText(/Control is off on Pixel/)).toBeInTheDocument();
    const canvas = screen.getByLabelText("Pixel's screen");
    fireEvent.pointerDown(canvas, { clientX: 1, clientY: 1, button: 0, pointerId: 1 });
    fireEvent.pointerUp(canvas, { clientX: 1, clientY: 1, pointerId: 1 });
    expect(api.sendScreenInput).not.toHaveBeenCalled();
  });

  it("says why input did not work", async () => {
    const { api } = await showing();
    api.emitScreen(
      "phone-1",
      testScreen({ phase: "showing", format: jpegFormat, inputProblem: "noTextField" }),
    );
    expect(screen.getByRole("alert")).toHaveTextContent("Tap a text field");
  });

  it("stops the stream when the page closes", async () => {
    const { api, unmount } = await showing();
    await userEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(api.stopScreen).toHaveBeenCalledTimes(1);
    api.emitScreen("phone-1", testScreen());

    await userEvent.click(await screen.findByRole("button", { name: "Show Pixel's screen" }));
    unmount();
    expect(api.stopScreen).toHaveBeenCalledTimes(2);
  });

  it("says why the stream ended or cannot start", async () => {
    renderPage({ status: testScreen({ ended: "declined" }) });
    expect(await screen.findByText("Sharing was declined on Pixel.")).toBeInTheDocument();

    renderPage({ status: testScreen({ blocked: "turnedOff" }) });
    expect(await screen.findByText(/Screen sharing is off on Pixel/)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Show Pixel's screen" })).toHaveLength(1);
  });

  it("asks for a phone when none is paired", async () => {
    renderPage({ devices: [] });
    expect(await screen.findByText("No phone paired")).toBeInTheDocument();
  });
});
