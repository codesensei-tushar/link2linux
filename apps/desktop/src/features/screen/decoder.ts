import { avcCodecString, parsePacket, type PacketFormat } from "./packet";

/** A decoded picture: a WebCodecs frame or a bitmap. The receiver draws it and closes it. */
export type Picture = (VideoFrame | ImageBitmap) & { close(): void };

/** What the decoder uses of the browser, so tests can stand in for it. */
export interface DecoderEnv {
  VideoDecoder?: typeof VideoDecoder;
  EncodedVideoChunk?: typeof EncodedVideoChunk;
  createImageBitmap: (image: Blob) => Promise<ImageBitmap>;
}

export interface ScreenDecoderOptions {
  /** Shows a picture of `width` by `height` pixels, then closes it. */
  draw(picture: Picture, width: number, height: number): void;
  /** Frames are being dropped up to the next key frame: ask the phone for one. */
  askKeyFrame(): void;
  /** The browser cannot decode the phone's H.264: ask for JPEG instead. */
  unsupported(): void;
  env?: DecoderEnv;
}

/** H.264 frames waiting in the decoder before it counts as behind: about a tenth of a second. */
export const MAX_DECODE_QUEUE = 6;
/** JPEG frames being decoded at once; the ones after them are skipped. */
export const MAX_JPEG_DECODING = 2;
/** Asks for key frames at most this often, in milliseconds, as the protocol allows. */
export const KEY_FRAME_EVERY_MS = 1000;

const H264_PROBES = ["avc1.42E01F", "avc1.4D401F", "avc1.640028"];

function defaultEnv(): DecoderEnv {
  return {
    VideoDecoder: globalThis.VideoDecoder,
    EncodedVideoChunk: globalThis.EncodedVideoChunk,
    createImageBitmap: (image) => globalThis.createImageBitmap(image),
  };
}

/**
 * Whether this webview decodes H.264 with WebCodecs, for one of the common profiles a phone
 * encodes in. Without a decoder (for example without GStreamer's libav plugin), the page asks
 * for JPEG.
 */
export async function h264Supported(env: DecoderEnv = defaultEnv()): Promise<boolean> {
  const decoder = env.VideoDecoder;
  if (decoder === undefined || env.EncodedVideoChunk === undefined) return false;
  for (const codec of H264_PROBES) {
    try {
      const support = await decoder.isConfigSupported({ codec, optimizeForLatency: true });
      if (support.supported === true) return true;
    } catch {
      // A probe the webview rejects says nothing about the others.
    }
  }
  return false;
}

/**
 * Turns the packets of one stream into pictures. A frame that cannot be decoded, or that comes
 * while the decoder is behind, is dropped with every frame up to the next key frame, which it
 * asks the phone for. Packets of other formats than the one being decoded are dropped too.
 */
export class ScreenDecoder {
  private readonly options: ScreenDecoderOptions;
  private readonly env: DecoderEnv;
  private format: (PacketFormat & { id: number; codecString: string | null }) | null = null;
  private video: VideoDecoder | null = null;
  private waitingForKey = true;
  private jpegDecoding = 0;
  private lastKeyAsk = Number.NEGATIVE_INFINITY;
  private closed = false;

  constructor(options: ScreenDecoderOptions) {
    this.options = options;
    this.env = options.env ?? defaultEnv();
  }

  /** Takes one packet from the backend. */
  push(buffer: ArrayBuffer): void {
    if (this.closed) return;
    const packet = parsePacket(buffer);
    if (packet === null) return;
    if (packet.format !== null) this.useFormat(packet.formatId, packet.format);
    const format = this.format;
    if (format?.id !== packet.formatId) return;
    if (this.waitingForKey && !packet.keyFrame) {
      this.askKeyFrame();
      return;
    }
    if (format.codec === "jpeg") {
      this.decodeJpeg(packet.data, format.width, format.height);
      return;
    }
    const video = this.video ?? this.openVideo();
    if (video === null) return;
    if (video.decodeQueueSize > MAX_DECODE_QUEUE) {
      this.dropToKeyFrame();
      return;
    }
    const Chunk = this.env.EncodedVideoChunk;
    if (Chunk === undefined) return;
    try {
      video.decode(
        new Chunk({
          type: packet.keyFrame ? "key" : "delta",
          timestamp: packet.timestampUs,
          data: packet.data,
        }),
      );
      if (packet.keyFrame) this.waitingForKey = false;
    } catch {
      this.dropToKeyFrame();
    }
  }

  /** Stops decoding: what is still in flight is dropped. */
  close(): void {
    this.closed = true;
    this.closeVideo();
  }

  private useFormat(id: number, format: PacketFormat): void {
    this.closeVideo();
    this.waitingForKey = true;
    const codecString = format.codec === "h264" ? avcCodecString(format.config) : null;
    this.format = { ...format, id, codecString };
    if (format.codec === "h264") {
      if (codecString === null) {
        this.format = null;
        this.options.unsupported();
        return;
      }
      void this.checkSupport(id, codecString, format.config);
    }
  }

  /** Asks the webview whether it decodes this exact stream, and gives up on H.264 if not. */
  private async checkSupport(
    id: number,
    codec: string,
    description: Uint8Array<ArrayBuffer>,
  ): Promise<void> {
    const decoder = this.env.VideoDecoder;
    let supported: boolean;
    try {
      supported =
        (await decoder?.isConfigSupported({ codec, description, optimizeForLatency: true }))
          ?.supported === true;
    } catch {
      supported = false;
    }
    if (!supported && !this.closed && this.format?.id === id) {
      this.format = null;
      this.closeVideo();
      this.options.unsupported();
    }
  }

  private openVideo(): VideoDecoder | null {
    const format = this.format;
    const Decoder = this.env.VideoDecoder;
    if (format?.codecString == null || Decoder === undefined) return null;
    const { width, height } = format;
    const video = new Decoder({
      output: (frame) => {
        if (this.closed || this.video !== video) {
          frame.close();
          return;
        }
        try {
          // The drawer closes the frame, including when drawing throws.
          this.options.draw(frame, width, height);
        } catch {
          // A frame that cannot be drawn must not fail the decoder.
        }
      },
      error: () => {
        // A decoder that failed is closed: the next key frame starts a new one.
        if (this.video === video) {
          this.video = null;
          this.dropToKeyFrame();
        }
      },
    });
    try {
      video.configure({
        codec: format.codecString,
        description: format.config,
        optimizeForLatency: true,
      });
    } catch {
      // configure() failing leaves the decoder unusable. Drop it, or the
      // callback cycle keeps it, and wait for a key frame.
      try {
        video.close();
      } catch {
        // Closed already.
      }
      this.dropToKeyFrame();
      return null;
    }
    this.video = video;
    return video;
  }

  private closeVideo(): void {
    const video = this.video;
    this.video = null;
    if (video !== null && video.state !== "closed") {
      try {
        video.close();
      } catch {
        // Closed already.
      }
    }
  }

  private decodeJpeg(data: Uint8Array<ArrayBuffer>, width: number, height: number): void {
    // Every JPEG frame stands alone, so one that comes while two decode is
    // skipped. The phone is still asked for a key frame, as when any frame
    // is dropped.
    if (this.jpegDecoding >= MAX_JPEG_DECODING) {
      this.askKeyFrame();
      return;
    }
    this.jpegDecoding += 1;
    const formatId = this.format?.id;
    this.env.createImageBitmap(new Blob([data], { type: "image/jpeg" })).then(
      (bitmap) => {
        this.jpegDecoding -= 1;
        if (this.closed || this.format?.id !== formatId) {
          bitmap.close();
          return;
        }
        try {
          this.options.draw(bitmap, width, height);
        } catch {
          // The drawer closes the bitmap, including when drawing throws.
        }
      },
      () => {
        this.jpegDecoding -= 1;
        this.askKeyFrame();
      },
    );
  }

  private dropToKeyFrame(): void {
    this.waitingForKey = true;
    this.askKeyFrame();
  }

  private askKeyFrame(): void {
    const now = performance.now();
    if (now - this.lastKeyAsk < KEY_FRAME_EVERY_MS) return;
    this.lastKeyAsk = now;
    this.options.askKeyFrame();
  }
}
