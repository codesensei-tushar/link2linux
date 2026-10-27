import { describe, expect, it, vi } from "vitest";

import { thumbnailQueue, thumbnailSize, type ThumbnailOutcome } from "./thumbnails";

/** A fetch whose answers the test gives by hand. */
function manualFetch() {
  const answers = new Map<string, { resolve: (blob: Blob) => void; reject: (e: Error) => void }>();
  const fetch = vi.fn(
    (photoId: string) =>
      new Promise<Blob>((resolve, reject) => {
        answers.set(photoId, { resolve, reject });
      }),
  );
  const answer = async (photoId: string, outcome: Blob | Error) => {
    const pending = answers.get(photoId);
    if (!pending) throw new Error(`${photoId} was not asked for`);
    if (outcome instanceof Error) pending.reject(outcome);
    else pending.resolve(outcome);
    // Lets the queue's own promise handlers run.
    await new Promise((resolve) => setTimeout(resolve, 0));
  };
  return { fetch, answer, asked: () => fetch.mock.calls.map(([id]) => id) };
}

describe("thumbnailQueue", () => {
  it("asks for at most `limit` at a time, in order, and passes the answers on", async () => {
    const { fetch, answer, asked } = manualFetch();
    const queue = thumbnailQueue(fetch, 2);
    const outcomes: Record<string, ThumbnailOutcome> = {};
    for (const id of ["a", "b", "c"]) {
      queue.request(id, 256, (outcome) => {
        outcomes[id] = outcome;
      });
    }
    expect(asked()).toEqual(["a", "b"]);
    expect(fetch).toHaveBeenCalledWith("a", 256);

    const image = new Blob(["x"]);
    await answer("a", image);
    expect(outcomes.a).toEqual({ image });
    expect(asked()).toEqual(["a", "b", "c"]);

    await answer("b", new Error("The photo didn't come through. Try again."));
    expect(outcomes.b).toEqual({ error: "The photo didn't come through. Try again." });
  });

  it("never sends a withdrawn wish, and drops the answer to one already sent", async () => {
    const { fetch, answer, asked } = manualFetch();
    const queue = thumbnailQueue(fetch, 1);
    const done = vi.fn();
    const withdrawSent = queue.request("a", 256, done);
    const withdrawWaiting = queue.request("b", 256, done);
    queue.request("c", 256, done);

    withdrawWaiting();
    withdrawSent();
    await answer("a", new Blob(["x"]));
    expect(done).not.toHaveBeenCalled();
    // The slot "a" held goes to "c": "b" is no longer wanted.
    expect(asked()).toEqual(["a", "c"]);

    await answer("c", new Blob(["y"]));
    expect(done).toHaveBeenCalledTimes(1);
  });
});

describe("thumbnailSize", () => {
  it("asks for larger pictures on dense screens, from a few fixed sizes", () => {
    expect(thumbnailSize("grid", 1)).toBe(256);
    expect(thumbnailSize("grid", 2)).toBe(384);
    expect(thumbnailSize("viewer", 1)).toBe(1600);
    expect(thumbnailSize("viewer", 3)).toBe(2048);
  });
});
