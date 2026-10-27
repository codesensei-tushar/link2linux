/**
 * A `Blob` of an encoded image as the IPC delivered it: an `ArrayBuffer`, or
 * an array of numbers where the webview fell back to messages for IPC. Its
 * type comes from the image's first bytes, since raw bytes travel without
 * one.
 */
export function imageBlob(data: ArrayBuffer | ArrayLike<number>): Blob {
  const bytes = data instanceof ArrayBuffer ? new Uint8Array(data) : Uint8Array.from(data);
  return new Blob([bytes], { type: imageType(bytes) });
}

/** The MIME type of a JPEG, PNG or WebP image, or "" for anything else. */
export function imageType(bytes: Uint8Array): string {
  const startsWith = (signature: number[], at = 0) =>
    signature.every((byte, index) => bytes[at + index] === byte);
  if (startsWith([0xff, 0xd8, 0xff])) return "image/jpeg";
  if (startsWith([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return "image/png";
  // "RIFF", a length, then "WEBP".
  if (startsWith([0x52, 0x49, 0x46, 0x46]) && startsWith([0x57, 0x45, 0x42, 0x50], 8)) {
    return "image/webp";
  }
  return "";
}
