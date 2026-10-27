import type { Photo } from "@/lib/api";

const HOUR_MS = 3_600_000;

/** The colours, top to bottom, a demo photo is drawn with. */
const gradients: readonly (readonly [string, string])[] = [
  ["#6ca6d9", "#e8d5a9"],
  ["#2f6b3a", "#9cc57a"],
  ["#f28c48", "#5b2a6e"],
  ["#1d2b4f", "#7f9cc9"],
  ["#d9d4cc", "#8a7f73"],
  ["#f4c2c2", "#b03a48"],
];
const screenshot = ["#f5f5f7", "#d5dbe5"] as const;

/** Recent photos for the browser preview, newest first: camera photos and now and then a screenshot. */
export function demoPhotos(now: number): Photo[] {
  return Array.from({ length: 24 }, (_, index) => {
    const taken = now - index * 5 * HOUR_MS - (index % 3) * 600_000;
    const isScreenshot = index % 7 === 3;
    const upright = index % 5 === 1;
    const number = 4200 - index;
    return {
      id: String(10_024 - index),
      name: isScreenshot ? `Screenshot_${number}.png` : `IMG_${number}.jpg`,
      mimeType: isScreenshot ? "image/png" : "image/jpeg",
      size: isScreenshot ? 380_000 + index * 1_100 : 2_100_000 + index * 37_000,
      takenUnixMs: taken,
      modifiedUnixMs: taken,
      width: isScreenshot ? 1080 : upright ? 3072 : 4080,
      height: isScreenshot ? 2400 : upright ? 4080 : 3072,
      saved: null,
    };
  });
}

/** A gradient shaped like `photo`, standing in for its thumbnail. */
export function demoThumbnail(photo: Photo): Blob {
  const [top, bottom] =
    photo.mimeType === "image/png"
      ? screenshot
      : (gradients[Number(photo.id) % gradients.length] ?? screenshot);
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${photo.width} ${photo.height}">` +
    `<linearGradient id="g" x2="0" y2="1"><stop offset="0" stop-color="${top}"/>` +
    `<stop offset="1" stop-color="${bottom}"/></linearGradient>` +
    `<rect width="100%" height="100%" fill="url(#g)"/></svg>`;
  return new Blob([svg], { type: "image/svg+xml" });
}
