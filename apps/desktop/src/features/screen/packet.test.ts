import { describe, expect, it } from "vitest";

import { avcCodecString, encodePacket, parsePacket } from "./packet";

const bytes = (values: number[]) => new Uint8Array(values).buffer;

describe("parsePacket", () => {
  it("reads the header the backend writes", () => {
    // The same bytes as the runtime's own packet test.
    const packet = parsePacket(
      bytes([1, 1, 1, 2, 3, 4, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0xaa, 0xbb]),
    );
    expect(packet).toMatchObject({ keyFrame: true, formatId: 0x01020304, format: null });
    expect(packet?.timestampUs).toBe(Number(0x1122334455667788n));
    expect([...(packet?.data ?? [])]).toEqual([0xaa, 0xbb]);
  });

  it("reads the format on the first frame of a format", () => {
    const header = [1, 3, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 5];
    const format = [1, 0, 0, 2, 0xd0, 0, 0, 6, 0x18, 0, 4, 1, 0x64, 0, 0x1f];
    const packet = parsePacket(bytes([...header, ...format, 0xaa, 0xbb]));
    expect(packet?.format).toMatchObject({ codec: "h264", width: 720, height: 1560 });
    expect([...(packet?.format?.config ?? [])]).toEqual([1, 0x64, 0, 0x1f]);
    expect([...(packet?.data ?? [])]).toEqual([0xaa, 0xbb]);
    expect(packet?.timestampUs).toBe(5);
  });

  it("refuses what is not a whole frame packet", () => {
    expect(parsePacket(bytes([1, 0, 0]))).toBeNull();
    expect(parsePacket(bytes([2, ...new Array<number>(13).fill(0)]))).toBeNull();
    // A format that claims more configuration than there is.
    const header = [1, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
    expect(parsePacket(bytes([...header, 2, 0, 0, 0, 1, 0, 0, 0, 1, 0, 9]))).toBeNull();
    expect(parsePacket(bytes([...header, 7, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0]))).toBeNull();
  });

  it("reads back what encodePacket writes", () => {
    const data = new Uint8Array([0xff, 0xd8, 0xff]);
    const packet = parsePacket(
      encodePacket({
        keyFrame: true,
        formatId: 3,
        timestampUs: 123_456,
        format: { codec: "jpeg", width: 360, height: 780, config: new Uint8Array(0) },
        data,
      }),
    );
    expect(packet).toMatchObject({ keyFrame: true, formatId: 3, timestampUs: 123_456 });
    expect(packet?.format).toMatchObject({ codec: "jpeg", width: 360, height: 780 });
    expect([...(packet?.data ?? [])]).toEqual([...data]);
  });
});

describe("avcCodecString", () => {
  it("names the profile, constraints and level of an avcC record", () => {
    expect(avcCodecString(new Uint8Array([1, 0x64, 0x00, 0x1f, 0xff, 0xe1, 0]))).toBe(
      "avc1.64001F",
    );
    expect(avcCodecString(new Uint8Array([1, 0x42, 0xe0, 0x1f, 0xff, 0xe1, 0]))).toBe(
      "avc1.42E01F",
    );
  });

  it("refuses a record that is too short or of another version", () => {
    expect(avcCodecString(new Uint8Array([1, 0x64, 0, 0x1f]))).toBeNull();
    expect(avcCodecString(new Uint8Array([0, 0x64, 0, 0x1f, 0, 0, 0]))).toBeNull();
  });
});
