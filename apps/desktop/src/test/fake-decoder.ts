import { vi } from "vitest";

import type { DecoderEnv } from "@/features/screen/decoder";

/** A stand-in for WebCodecs' EncodedVideoChunk that keeps what it was made from. */
export class FakeChunk {
  readonly type: "key" | "delta";
  readonly timestamp: number;
  readonly data: Uint8Array;

  constructor(init: { type: "key" | "delta"; timestamp: number; data: Uint8Array }) {
    this.type = init.type;
    this.timestamp = init.timestamp;
    this.data = init.data;
  }
}

/** A stand-in for WebCodecs' VideoDecoder that keeps the chunks it gets. */
export class FakeVideoDecoder {
  /** What isConfigSupported answers, by codec string; missing ones are supported. */
  static support = new Map<string, boolean>();
  static instances: FakeVideoDecoder[] = [];

  static isConfigSupported(config: { codec: string }) {
    return Promise.resolve({ supported: FakeVideoDecoder.support.get(config.codec) ?? true });
  }

  state: "unconfigured" | "configured" | "closed" = "unconfigured";
  decodeQueueSize = 0;
  readonly chunks: FakeChunk[] = [];
  config: { codec: string; description?: Uint8Array } | null = null;

  constructor(private readonly init: { output(frame: unknown): void; error(e: unknown): void }) {
    FakeVideoDecoder.instances.push(this);
  }

  configure(config: { codec: string; description?: Uint8Array }) {
    this.config = config;
    this.state = "configured";
  }

  decode(chunk: FakeChunk) {
    this.chunks.push(chunk);
  }

  close() {
    this.state = "closed";
  }

  /** The decoder puts out a frame. */
  output(): { close: ReturnType<typeof vi.fn> } {
    const frame = { close: vi.fn() };
    this.init.output(frame);
    return frame;
  }

  /** The decoder fails, which closes it. */
  fail() {
    this.state = "closed";
    this.init.error(new Error("decoding failed"));
  }

  static reset() {
    FakeVideoDecoder.support = new Map();
    FakeVideoDecoder.instances = [];
  }
}

/** A browser for the decoder: with WebCodecs unless `webCodecs` is false. */
export function fakeDecoderEnv(webCodecs = true): DecoderEnv & {
  createImageBitmap: ReturnType<typeof vi.fn>;
} {
  return {
    VideoDecoder: webCodecs ? (FakeVideoDecoder as unknown as typeof VideoDecoder) : undefined,
    EncodedVideoChunk: webCodecs ? (FakeChunk as unknown as typeof EncodedVideoChunk) : undefined,
    createImageBitmap: vi.fn(() =>
      Promise.resolve({ width: 360, height: 780, close: vi.fn() } as unknown as ImageBitmap),
    ),
  };
}
