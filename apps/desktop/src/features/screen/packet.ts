import type { ScreenCodec } from "@/lib/api";

/** How the frames that follow a packet's format are encoded. */
export interface PacketFormat {
  codec: ScreenCodec;
  width: number;
  height: number;
  /** For H.264, the avcC record; empty for JPEG. */
  config: Uint8Array<ArrayBuffer>;
}

/** One frame of a phone's screen, as the backend sends it. */
export interface ScreenPacket {
  keyFrame: boolean;
  formatId: number;
  /** When the phone captured it, in microseconds from a start the phone picked. */
  timestampUs: number;
  /** Set on the first frame in a new format, which is always a key frame. */
  format: PacketFormat | null;
  data: Uint8Array<ArrayBuffer>;
}

const HEADER_LEN = 14;
const FORMAT_LEN = 11;
const FRAME = 1;
const KEY_FRAME = 1;
const HAS_FORMAT = 2;

/**
 * Reads a packet: a header, then the frame's data. Numbers are big-endian. The header is the kind
 * (1: a frame), flags (bit 0 a key frame, bit 1 a format follows the header), the format's id in 4
 * bytes and the capture time in 8. A format is the codec (1 H.264, 2 JPEG), the width and the
 * height in 4 bytes each, and the codec's configuration with its length in 2 bytes before it.
 * Returns null for anything else.
 */
export function parsePacket(buffer: ArrayBuffer): ScreenPacket | null {
  const view = new DataView(buffer);
  if (view.byteLength < HEADER_LEN || view.getUint8(0) !== FRAME) return null;
  const flags = view.getUint8(1);
  let offset = HEADER_LEN;
  let format: PacketFormat | null = null;
  if (flags & HAS_FORMAT) {
    if (view.byteLength < offset + FORMAT_LEN) return null;
    const codecByte = view.getUint8(offset);
    const codec: ScreenCodec | null = codecByte === 1 ? "h264" : codecByte === 2 ? "jpeg" : null;
    const configLen = view.getUint16(offset + 9);
    if (codec === null || view.byteLength < offset + FORMAT_LEN + configLen) return null;
    format = {
      codec,
      width: view.getUint32(offset + 1),
      height: view.getUint32(offset + 5),
      config: new Uint8Array(buffer, offset + FORMAT_LEN, configLen),
    };
    offset += FORMAT_LEN + configLen;
  }
  return {
    keyFrame: (flags & KEY_FRAME) !== 0,
    formatId: view.getUint32(2),
    timestampUs: Number(view.getBigUint64(6)),
    format,
    data: new Uint8Array(buffer, offset),
  };
}

/**
 * The WebCodecs codec string for an avcC record: `avc1.` and its profile, constraint flags and
 * level in hexadecimal. Null when the record is too short or of an unknown version.
 */
export function avcCodecString(avcC: Uint8Array): string | null {
  if (avcC.length < 7 || avcC[0] !== 1) return null;
  const hex = (index: number) => (avcC[index] ?? 0).toString(16).padStart(2, "0").toUpperCase();
  return `avc1.${hex(1)}${hex(2)}${hex(3)}`;
}

/** Writes a packet as the backend does, for tests and the browser preview. */
export function encodePacket(packet: ScreenPacket): ArrayBuffer {
  const format = packet.format;
  const formatLen = format ? FORMAT_LEN + format.config.length : 0;
  const buffer = new ArrayBuffer(HEADER_LEN + formatLen + packet.data.length);
  const view = new DataView(buffer);
  view.setUint8(0, FRAME);
  view.setUint8(1, (packet.keyFrame ? KEY_FRAME : 0) | (format ? HAS_FORMAT : 0));
  view.setUint32(2, packet.formatId);
  view.setBigUint64(6, BigInt(packet.timestampUs));
  const bytes = new Uint8Array(buffer);
  if (format) {
    view.setUint8(HEADER_LEN, format.codec === "h264" ? 1 : 2);
    view.setUint32(HEADER_LEN + 1, format.width);
    view.setUint32(HEADER_LEN + 5, format.height);
    view.setUint16(HEADER_LEN + 9, format.config.length);
    bytes.set(format.config, HEADER_LEN + FORMAT_LEN);
  }
  bytes.set(packet.data, HEADER_LEN + formatLen);
  return buffer;
}
