import { errorMessage } from "@/lib/utils";

/** What came of asking for a thumbnail. */
export type ThumbnailOutcome = { image: Blob } | { error: string };

export interface ThumbnailQueue {
  /**
   * Asks for a thumbnail of photo `photoId`, at most `maxSize` pixels on its
   * longest side, and calls `done` with what came of it. Returns a function
   * that withdraws the wish: one still waiting is never sent, and the answer
   * to one already sent is dropped.
   */
  request(photoId: string, maxSize: number, done: (outcome: ThumbnailOutcome) => void): () => void;
}

/**
 * Fetches thumbnails at most `limit` at a time, in the order they were asked
 * for.
 *
 * A request the page has made can't be taken back, so the backend fetches
 * every thumbnail it was asked for. Waiting here instead means a photo the
 * user scrolled past is withdrawn before it costs the phone anything. The
 * limit stays under the 8 thumbnail requests the desktop keeps open with the
 * phone, so the viewer's larger picture never waits behind the grid there.
 */
export function thumbnailQueue(
  fetch: (photoId: string, maxSize: number) => Promise<Blob>,
  limit = 6,
): ThumbnailQueue {
  interface Wish {
    photoId: string;
    maxSize: number;
    done: (outcome: ThumbnailOutcome) => void;
    withdrawn: boolean;
  }
  const waiting: Wish[] = [];
  let running = 0;

  const next = () => {
    while (running < limit) {
      const wish = waiting.shift();
      if (!wish) return;
      running += 1;
      fetch(wish.photoId, wish.maxSize)
        .then(
          (image) => {
            if (!wish.withdrawn) wish.done({ image });
          },
          (cause: unknown) => {
            if (!wish.withdrawn) wish.done({ error: errorMessage(cause) });
          },
        )
        .finally(() => {
          running -= 1;
          next();
        });
    }
  };

  return {
    request(photoId, maxSize, done) {
      const wish: Wish = { photoId, maxSize, done, withdrawn: false };
      waiting.push(wish);
      next();
      return () => {
        wish.withdrawn = true;
        const index = waiting.indexOf(wish);
        if (index >= 0) waiting.splice(index, 1);
      };
    },
  };
}

/**
 * The longest side to ask for: enough for a grid cell, or for the viewer, at
 * this screen's pixel density. There are only a few sizes, so the desktop's
 * memory of the thumbnails it fetched serves them again.
 */
export function thumbnailSize(use: "grid" | "viewer", pixelRatio: number): number {
  const dense = pixelRatio > 1.5;
  if (use === "grid") return dense ? 384 : 256;
  return dense ? 2048 : 1600;
}
