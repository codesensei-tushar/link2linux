import { encodePacket } from "./packet";

const WIDTH = 360;
const HEIGHT = 780;

/** The pretend phone's format in the browser preview. */
export const demoScreenFormat = {
  id: 1,
  codec: "jpeg" as const,
  width: WIDTH,
  height: HEIGHT,
  input: true,
};

/**
 * Frame `index` of a pretend phone screen for the browser preview: a plain home screen with a dot
 * moving across it, as a JPEG packet. Null where the browser cannot draw it.
 */
export async function demoScreenPacket(index: number): Promise<ArrayBuffer | null> {
  if (typeof document === "undefined") return null;
  const canvas = document.createElement("canvas");
  canvas.width = WIDTH;
  canvas.height = HEIGHT;
  const context = canvas.getContext("2d");
  if (!context) return null;
  context.fillStyle = "#202430";
  context.fillRect(0, 0, WIDTH, HEIGHT);
  const colors = ["#e57373", "#81c784", "#64b5f6", "#ffd54f", "#ba68c8", "#4db6ac"];
  for (let row = 0; row < 4; row++) {
    for (let col = 0; col < 4; col++) {
      context.fillStyle = colors[(row * 4 + col) % colors.length] ?? "#ffffff";
      context.fillRect(28 + col * 80, 150 + row * 100, 56, 56);
    }
  }
  context.fillStyle = "#ffffff";
  context.beginPath();
  context.arc(40 + (index % 6) * 56, 600, 14, 0, Math.PI * 2);
  context.fill();
  const blob = await new Promise<Blob | null>((resolve) => {
    canvas.toBlob(resolve, "image/jpeg", 0.7);
  });
  if (!blob) return null;
  return encodePacket({
    keyFrame: true,
    formatId: demoScreenFormat.id,
    timestampUs: index * 100_000,
    format:
      index === 0
        ? { codec: "jpeg", width: WIDTH, height: HEIGHT, config: new Uint8Array(0) }
        : null,
    data: new Uint8Array(await blob.arrayBuffer()),
  });
}
