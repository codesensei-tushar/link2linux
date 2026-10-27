import { describe, expect, it } from "vitest";

import { imageBlob, imageType } from "./image-blob";

const jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x00];
const png = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00];
const webp = [0x52, 0x49, 0x46, 0x46, 0x10, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50, 0x56];

describe("imageType", () => {
  it("knows the images the phone sends by their first bytes", () => {
    expect(imageType(Uint8Array.from(jpeg))).toBe("image/jpeg");
    expect(imageType(Uint8Array.from(png))).toBe("image/png");
    expect(imageType(Uint8Array.from(webp))).toBe("image/webp");
  });

  it("gives no type to anything else", () => {
    expect(imageType(new Uint8Array())).toBe("");
    expect(imageType(Uint8Array.from([0xff, 0xd8]))).toBe("");
    // RIFF holds other formats too, such as WAV.
    expect(imageType(Uint8Array.from([...webp.slice(0, 8), 0x57, 0x41, 0x56, 0x45]))).toBe("");
  });
});

describe("imageBlob", () => {
  it("keeps the bytes from an ArrayBuffer or from an array of numbers", async () => {
    const fromBuffer = imageBlob(Uint8Array.from(png).buffer);
    expect(fromBuffer.type).toBe("image/png");
    expect([...new Uint8Array(await fromBuffer.arrayBuffer())]).toEqual(png);

    const fromNumbers = imageBlob(jpeg);
    expect(fromNumbers.type).toBe("image/jpeg");
    expect(fromNumbers.size).toBe(jpeg.length);
  });
});
