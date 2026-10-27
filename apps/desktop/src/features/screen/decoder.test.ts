import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { FakeChunk, FakeVideoDecoder, fakeDecoderEnv } from "@/test/fake-decoder";

import { KEY_FRAME_EVERY_MS, MAX_DECODE_QUEUE, ScreenDecoder, h264Supported } from "./decoder";
import { encodePacket, type PacketFormat } from "./packet";

const avcC = new Uint8Array([1, 0x64, 0x00, 0x1f, 0xff, 0xe1, 0]);
const h264: PacketFormat = { codec: "h264", width: 720, height: 1560, config: avcC };
const jpeg: PacketFormat = { codec: "jpeg", width: 360, height: 780, config: new Uint8Array(0) };

function frame(
  timestampUs: number,
  keyFrame: boolean,
  options: { format?: PacketFormat; formatId?: number } = {},
): ArrayBuffer {
  return encodePacket({
    keyFrame,
    formatId: options.formatId ?? 1,
    timestampUs,
    format: options.format ?? null,
    data: new Uint8Array([timestampUs & 0xff]),
  });
}

function setUp(webCodecs = true) {
  const env = fakeDecoderEnv(webCodecs);
  const draw = vi.fn<(picture: { close(): void }, width: number, height: number) => void>();
  const askKeyFrame = vi.fn();
  const unsupported = vi.fn();
  const decoder = new ScreenDecoder({ env, draw, askKeyFrame, unsupported });
  return { env, draw, askKeyFrame, unsupported, decoder };
}

/** Lets promises settle. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  FakeVideoDecoder.reset();
  vi.spyOn(performance, "now").mockReturnValue(10_000);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("h264Supported", () => {
  it("needs WebCodecs and a profile the webview decodes", async () => {
    expect(await h264Supported(fakeDecoderEnv(false))).toBe(false);
    expect(await h264Supported(fakeDecoderEnv())).toBe(true);
    FakeVideoDecoder.support = new Map([
      ["avc1.42E01F", false],
      ["avc1.4D401F", false],
      ["avc1.640028", false],
    ]);
    expect(await h264Supported(fakeDecoderEnv())).toBe(false);
  });
});

describe("ScreenDecoder with H.264", () => {
  it("configures from the avcC and decodes key and delta frames", () => {
    const { decoder, draw } = setUp();
    decoder.push(frame(0, true, { format: h264 }));
    decoder.push(frame(1, false));
    const [video] = FakeVideoDecoder.instances;
    expect(video?.config?.codec).toBe("avc1.64001F");
    expect([...(video?.config?.description ?? [])]).toEqual([...avcC]);
    expect(video?.chunks.map((chunk) => [chunk.type, chunk.timestamp])).toEqual([
      ["key", 0],
      ["delta", 1],
    ]);
    expect(video?.chunks[0]).toBeInstanceOf(FakeChunk);

    video?.output();
    expect(draw).toHaveBeenCalledWith(expect.anything(), 720, 1560);
  });

  it("drops frames before a key frame and asks for one at most once a second", () => {
    const { decoder, askKeyFrame } = setUp();
    decoder.push(frame(0, false, { format: h264 }));
    decoder.push(frame(1, false));
    expect(askKeyFrame).toHaveBeenCalledTimes(1);
    expect(FakeVideoDecoder.instances).toHaveLength(0);

    vi.spyOn(performance, "now").mockReturnValue(10_000 + KEY_FRAME_EVERY_MS);
    decoder.push(frame(2, false));
    expect(askKeyFrame).toHaveBeenCalledTimes(2);
    decoder.push(frame(3, true));
    expect(FakeVideoDecoder.instances[0]?.chunks).toHaveLength(1);
  });

  it("skips to the next key frame when the decoder falls behind", () => {
    const { decoder, askKeyFrame } = setUp();
    decoder.push(frame(0, true, { format: h264 }));
    const video = FakeVideoDecoder.instances[0];
    if (!video) throw new Error("no decoder");
    video.decodeQueueSize = MAX_DECODE_QUEUE + 1;
    decoder.push(frame(1, false));
    expect(askKeyFrame).toHaveBeenCalledTimes(1);
    video.decodeQueueSize = 0;
    decoder.push(frame(2, false));
    expect(video.chunks).toHaveLength(1);
    decoder.push(frame(3, true));
    expect(video.chunks).toHaveLength(2);
  });

  it("starts a new decoder at the key frame after an error", () => {
    const { decoder, askKeyFrame } = setUp();
    decoder.push(frame(0, true, { format: h264 }));
    FakeVideoDecoder.instances[0]?.fail();
    expect(askKeyFrame).toHaveBeenCalledTimes(1);
    decoder.push(frame(1, false));
    decoder.push(frame(2, true));
    expect(FakeVideoDecoder.instances).toHaveLength(2);
    expect(FakeVideoDecoder.instances[1]?.chunks.map((chunk) => chunk.type)).toEqual(["key"]);
  });

  it("gives up on H.264 the webview cannot decode", async () => {
    FakeVideoDecoder.support = new Map([["avc1.64001F", false]]);
    const { decoder, unsupported } = setUp();
    decoder.push(frame(0, true, { format: h264 }));
    await settle();
    expect(unsupported).toHaveBeenCalledTimes(1);
    decoder.push(frame(1, true));
    expect(FakeVideoDecoder.instances.at(-1)?.state).toBe("closed");
  });

  it("drops what comes out after it is closed, and packets of other formats", () => {
    const { decoder, draw } = setUp();
    decoder.push(frame(0, true, { format: h264 }));
    decoder.push(frame(1, true, { formatId: 2 }));
    expect(FakeVideoDecoder.instances[0]?.chunks).toHaveLength(1);
    const video = FakeVideoDecoder.instances[0];
    decoder.close();
    // A decoder that is closed may still put out a frame it had.
    const late = video?.output();
    expect(late?.close).toHaveBeenCalled();
    expect(draw).not.toHaveBeenCalled();
  });
});

describe("ScreenDecoder with JPEG", () => {
  it("draws each frame and skips frames while two decode", async () => {
    const { decoder, env, draw } = setUp(false);
    decoder.push(frame(0, true, { format: jpeg }));
    decoder.push(frame(1, true));
    decoder.push(frame(2, true));
    expect(env.createImageBitmap).toHaveBeenCalledTimes(2);
    await settle();
    expect(draw).toHaveBeenCalledTimes(2);
    expect(draw).toHaveBeenCalledWith(expect.anything(), 360, 780);
    decoder.push(frame(3, true));
    expect(env.createImageBitmap).toHaveBeenCalledTimes(3);
  });
});
