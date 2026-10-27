import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { DevicesProvider } from "@/features/pairing/DevicesProvider";
import { DesktopApiContext, type Device, type PhonePhotos, type Photo } from "@/lib/api";
import { createFakeApi, testDevice, testPhoto, type FakeApi } from "@/test/fake-api";

import { PhotosPage } from "./PhotosPage";

/** Stands in for IntersectionObserver: tests say what scrolls into view. */
class FakeObserver {
  static all: FakeObserver[] = [];
  readonly targets = new Set<Element>();

  constructor(readonly callback: IntersectionObserverCallback) {
    FakeObserver.all.push(this);
  }

  observe(target: Element) {
    this.targets.add(target);
  }

  unobserve(target: Element) {
    this.targets.delete(target);
  }

  disconnect() {
    this.targets.clear();
  }

  takeRecords(): IntersectionObserverEntry[] {
    return [];
  }
}

/** Scrolls `element` into view, or out of it, as its observers see it. */
function scroll(element: Element, into = true) {
  act(() => {
    for (const observer of FakeObserver.all) {
      if (!observer.targets.has(element)) continue;
      const entry = { target: element, isIntersecting: into } as IntersectionObserverEntry;
      observer.callback([entry], observer as unknown as IntersectionObserver);
    }
  });
}

/** The `blob:` URLs the page made and let go of. */
let urls: { created: string[]; revoked: string[] };

beforeEach(() => {
  FakeObserver.all = [];
  vi.stubGlobal("IntersectionObserver", FakeObserver);
  urls = { created: [], revoked: [] };
  // jsdom has no object URLs.
  Object.defineProperty(URL, "createObjectURL", {
    configurable: true,
    value: () => {
      const url = `blob:test/${urls.created.length + 1}`;
      urls.created.push(url);
      return url;
    },
  });
  Object.defineProperty(URL, "revokeObjectURL", {
    configurable: true,
    value: (url: string) => {
      urls.revoked.push(url);
    },
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(URL, "createObjectURL");
  Reflect.deleteProperty(URL, "revokeObjectURL");
});

const pixel = testDevice({ id: "phone-1", name: "Pixel", status: "connected" });
const hour = 3_600_000;
const newest = Date.UTC(2026, 9, 27, 9, 30);
const photos: Photo[] = [
  testPhoto({ id: "3", name: "IMG_3.jpg", takenUnixMs: newest, modifiedUnixMs: newest }),
  testPhoto({
    id: "2",
    name: "Screenshot_2.png",
    mimeType: "image/png",
    width: 1080,
    height: 2400,
    takenUnixMs: newest - hour,
    modifiedUnixMs: newest - hour,
  }),
  testPhoto({ id: "1", name: "IMG_1.jpg", takenUnixMs: newest - 2 * hour }),
];

function listed(list: Photo[]): PhonePhotos {
  return { state: "listed", photos: list };
}

function renderPage(
  shown: PhonePhotos | Error,
  devices: Device[] = [pixel],
  api: FakeApi = createFakeApi(),
) {
  api.listDevices.mockResolvedValue(devices);
  if (shown instanceof Error) api.listPhotos.mockRejectedValue(shown);
  else api.listPhotos.mockResolvedValue(shown);
  const view = render(
    <DesktopApiContext value={api}>
      <DevicesProvider>
        <PhotosPage />
      </DevicesProvider>
    </DesktopApiContext>,
  );
  return { api, ...view };
}

/** The grid's button for the photo named `name`. */
function tile(name: string) {
  return screen.getByRole("button", { name: new RegExp(`^${name.replace(".", "\\.")}, `) });
}

/** Opens the photo named `name` and returns the viewer. */
async function openViewer(user: ReturnType<typeof userEvent.setup>, name: string) {
  await screen.findByRole("list", { name: "Photos on Pixel" });
  await user.click(tile(name));
  return screen.getByRole("dialog", { name });
}

describe("PhotosPage", () => {
  it("shows the photos newest first and fetches each thumbnail once it scrolls into view", async () => {
    const { api } = renderPage(listed(photos));

    const grid = await screen.findByRole("list", { name: "Photos on Pixel" });
    expect(screen.getByText("The 3 latest photos and screenshots from Pixel")).toBeVisible();
    const names = within(grid)
      .getAllByRole("button")
      .map((button) => button.getAttribute("aria-label")?.split(", ")[0]);
    expect(names).toEqual(["IMG_3.jpg", "Screenshot_2.png", "IMG_1.jpg"]);
    expect(api.photoThumbnail).not.toHaveBeenCalled();

    scroll(tile("IMG_3.jpg"));
    expect(api.photoThumbnail).toHaveBeenCalledWith("phone-1", "3", 256);
    await waitFor(() => {
      expect(tile("IMG_3.jpg").querySelector("img")).toHaveAttribute("src", "blob:test/1");
    });
    expect(tile("Screenshot_2.png").querySelector("img")).toBeNull();

    // Scrolling away and back keeps what came.
    scroll(tile("IMG_3.jpg"), false);
    scroll(tile("IMG_3.jpg"));
    expect(api.photoThumbnail).toHaveBeenCalledTimes(1);
  });

  it("asks again for a thumbnail that did not come once it is back in view", async () => {
    const api = createFakeApi();
    api.photoThumbnail.mockRejectedValueOnce(new Error("The photo didn't come through."));
    renderPage(listed(photos), [pixel], api);
    await screen.findByRole("list", { name: "Photos on Pixel" });

    scroll(tile("IMG_1.jpg"));
    await waitFor(() => {
      expect(tile("IMG_1.jpg").querySelector(".photo-tile__icon")).not.toBeNull();
    });
    scroll(tile("IMG_1.jpg"), false);
    scroll(tile("IMG_1.jpg"));
    await waitFor(() => {
      expect(tile("IMG_1.jpg").querySelector("img")).toHaveAttribute("src", "blob:test/1");
    });
    expect(api.photoThumbnail).toHaveBeenCalledTimes(2);
  });

  it("lets go of a thumbnail when its photo changes or the page closes", async () => {
    const [first] = photos.slice(2);
    if (!first) throw new Error("no photo");
    const { api, unmount } = renderPage(listed([first]));
    await screen.findByRole("list", { name: "Photos on Pixel" });
    scroll(tile("IMG_1.jpg"));
    await waitFor(() => {
      expect(tile("IMG_1.jpg").querySelector("img")).toHaveAttribute("src", "blob:test/1");
    });

    // Another phone's photos are not this one's.
    api.emitPhotos("phone-2", listed([]));
    expect(tile("IMG_1.jpg")).toBeVisible();

    api.emitPhotos("phone-1", listed([{ ...first, modifiedUnixMs: first.modifiedUnixMs + 1 }]));
    expect(urls.revoked).toEqual(["blob:test/1"]);
    expect(tile("IMG_1.jpg").querySelector("img")).toBeNull();
    scroll(tile("IMG_1.jpg"));
    await waitFor(() => {
      expect(tile("IMG_1.jpg").querySelector("img")).toHaveAttribute("src", "blob:test/2");
    });
    expect(api.photoThumbnail).toHaveBeenCalledTimes(2);

    unmount();
    expect(urls.revoked).toEqual(["blob:test/1", "blob:test/2"]);
  });

  it.each([
    ["disconnected", "Pixel isn't connected", /while it's connected/],
    ["turnedOff", "Photos are off on Pixel", /turn on Photos in the link2linux app on the phone/],
    ["permissionRequired", "Pixel needs your permission", /allow it to see your photos/],
    ["unavailable", "Pixel isn't sharing photos right now", /show up here again/],
    ["unsupported", "Update link2linux on Pixel", /too old to share photos/],
  ] as const)("explains why a phone whose photos are %s shows none", async (state, title, text) => {
    renderPage({ state, photos: [] });

    const notice = await screen.findByRole("region", { name: title });
    expect(within(notice).getByText(text)).toBeVisible();
    expect(screen.queryByRole("list")).not.toBeInTheDocument();
  });

  it("waits for the list and follows it as it changes", async () => {
    const { api } = renderPage({ state: "waiting", photos: [] });

    expect(await screen.findByText("Getting the list of photos from Pixel…")).toBeVisible();
    api.emitPhotos("phone-1", listed([]));
    expect(screen.getByRole("heading", { name: "No recent photos" })).toBeVisible();
    api.emitPhotos("phone-1", listed(photos));
    expect(screen.getByRole("list", { name: "Photos on Pixel" })).toBeVisible();
    api.emitPhotos("phone-1", { state: "permissionRequired", photos: [] });
    expect(screen.getByRole("heading", { name: "Pixel needs your permission" })).toBeVisible();
  });

  it("says why the list didn't load, and tries again", async () => {
    const user = userEvent.setup();
    const { api } = renderPage(new Error("link2linux didn't start, so this isn't available."));

    expect(await screen.findByRole("heading", { name: "Couldn't load photos" })).toBeVisible();
    expect(screen.getByText("link2linux didn't start, so this isn't available.")).toBeVisible();
    api.listPhotos.mockResolvedValue(listed(photos));
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("list", { name: "Photos on Pixel" })).toBeVisible();
  });

  it("opens a photo larger, saves it, then opens it or shows it in its folder", async () => {
    const user = userEvent.setup();
    const { api } = renderPage(listed(photos));
    api.savePhoto.mockResolvedValue({ name: "IMG_3 (1).jpg", folder: "Pictures" });

    const viewer = await openViewer(user, "IMG_3.jpg");
    expect(within(viewer).getByRole("button", { name: "Close" })).toHaveFocus();
    const picture = await within(viewer).findByRole("img", { name: "IMG_3.jpg" });
    expect(api.photoThumbnail).toHaveBeenCalledWith("phone-1", "3", 1600);
    expect(picture).toHaveAttribute("src", "blob:test/1");
    expect(within(viewer).getByText(/4080 × 3072 · 2\.4 MB$/)).toBeVisible();

    await user.click(within(viewer).getByRole("button", { name: "Save" }));
    expect(api.savePhoto).toHaveBeenCalledWith("phone-1", "3");
    expect(await within(viewer).findByText("Saved as IMG_3 (1).jpg in Pictures")).toBeVisible();
    expect(within(viewer).queryByRole("button", { name: "Save" })).not.toBeInTheDocument();

    await user.click(within(viewer).getByRole("button", { name: "Open" }));
    expect(api.openSavedPhoto).toHaveBeenCalledWith("phone-1", "3");
    await user.click(within(viewer).getByRole("button", { name: "Show in folder" }));
    expect(api.showSavedPhoto).toHaveBeenCalledWith("phone-1", "3");

    // The page remembers what was saved while it is open.
    await user.click(within(viewer).getByRole("button", { name: "Close" }));
    await user.click(tile("IMG_3.jpg"));
    expect(screen.getByText("Saved as IMG_3 (1).jpg in Pictures")).toBeVisible();
  });

  it("shows a photo saved earlier in this run as already saved", async () => {
    const user = userEvent.setup();
    const [newest, ...rest] = photos;
    if (!newest) throw new Error("no photo");
    const { unmount } = renderPage(
      listed([{ ...newest, saved: { name: "IMG_3 (1).jpg", folder: "Downloads" } }, ...rest]),
    );

    const viewer = await openViewer(user, "IMG_3.jpg");
    expect(within(viewer).getByText("Saved as IMG_3 (1).jpg in Downloads")).toBeVisible();
    expect(within(viewer).getByRole("button", { name: "Open" })).toBeVisible();
    expect(within(viewer).queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    unmount();

    // A new version of the photo was not saved.
    renderPage(listed([{ ...newest, modifiedUnixMs: newest.modifiedUnixMs + 1 }]));
    const again = await openViewer(user, "IMG_3.jpg");
    expect(within(again).getByRole("button", { name: "Save" })).toBeVisible();
  });

  it("steps through the photos and closes, letting go of the larger pictures", async () => {
    const user = userEvent.setup();
    const { api } = renderPage(listed(photos));

    let viewer = await openViewer(user, "IMG_3.jpg");
    await within(viewer).findByRole("img", { name: "IMG_3.jpg" });
    expect(within(viewer).getByRole("button", { name: "Previous photo" })).toBeDisabled();

    await user.click(within(viewer).getByRole("button", { name: "Next photo" }));
    viewer = screen.getByRole("dialog", { name: "Screenshot_2.png" });
    await within(viewer).findByRole("img", { name: "Screenshot_2.png" });
    expect(api.photoThumbnail).toHaveBeenLastCalledWith("phone-1", "2", 1600);
    expect(urls.revoked).toEqual(["blob:test/1"]);

    await user.keyboard("{ArrowRight}");
    viewer = screen.getByRole("dialog", { name: "IMG_1.jpg" });
    expect(within(viewer).getByRole("button", { name: "Next photo" })).toBeDisabled();
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("dialog", { name: "Screenshot_2.png" })).toBeVisible();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(tile("IMG_3.jpg")).toHaveFocus();
    await waitFor(() => {
      expect([...urls.revoked].sort()).toEqual([...urls.created].sort());
    });
  });

  it("fetches only the photo the user stops at when stepping quickly", async () => {
    const user = userEvent.setup();
    const { api } = renderPage(listed(photos));
    const viewer = await openViewer(user, "IMG_3.jpg");
    await within(viewer).findByRole("img", { name: "IMG_3.jpg" });

    await user.keyboard("{ArrowRight}{ArrowRight}");
    const last = screen.getByRole("dialog", { name: "IMG_1.jpg" });
    await within(last).findByRole("img", { name: "IMG_1.jpg" });
    expect(api.photoThumbnail.mock.calls.map(([, id]) => id)).toEqual(["3", "1"]);
  });

  it("keeps Tab within the viewer", async () => {
    const user = userEvent.setup();
    renderPage(listed(photos));
    const viewer = await openViewer(user, "IMG_1.jpg");
    const close = within(viewer).getByRole("button", { name: "Close" });

    await user.tab({ shift: true });
    expect(within(viewer).getByRole("button", { name: "Save" })).toHaveFocus();
    await user.tab();
    expect(close).toHaveFocus();
  });

  it("shows why a photo didn't load or save", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.photoThumbnail.mockRejectedValueOnce(
      new Error("The photo didn't come through. Try again."),
    );
    api.savePhoto.mockRejectedValueOnce(
      new Error("The photo couldn't be saved in your Pictures folder."),
    );
    renderPage(listed(photos), [pixel], api);

    const viewer = await openViewer(user, "IMG_3.jpg");
    expect(await within(viewer).findByRole("alert")).toHaveTextContent(
      "The photo didn't come through. Try again.",
    );
    await user.click(within(viewer).getByRole("button", { name: "Try again" }));
    expect(await within(viewer).findByRole("img", { name: "IMG_3.jpg" })).toBeVisible();

    await user.click(within(viewer).getByRole("button", { name: "Save" }));
    expect(await within(viewer).findByRole("alert")).toHaveTextContent(
      "The photo couldn't be saved in your Pictures folder.",
    );
    expect(within(viewer).getByRole("button", { name: "Save" })).toBeEnabled();

    // The failure was this photo's, not the next one's.
    await user.click(within(viewer).getByRole("button", { name: "Next photo" }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("closes the viewer when its photo leaves the phone", async () => {
    const user = userEvent.setup();
    const { api } = renderPage(listed(photos));
    await openViewer(user, "IMG_3.jpg");

    api.emitPhotos("phone-1", listed(photos.slice(1)));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    // It stays closed if the photo comes back.
    api.emitPhotos("phone-1", listed(photos));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("shows the connected phone first and switches to another", async () => {
    const user = userEvent.setup();
    const galaxy = testDevice({ id: "phone-2", name: "Galaxy", status: "disconnected" });
    const { api } = renderPage(listed(photos), [galaxy, pixel]);

    expect(await screen.findByRole("list", { name: "Photos on Pixel" })).toBeVisible();
    api.listPhotos.mockResolvedValue({ state: "disconnected", photos: [] });
    await user.click(screen.getByRole("radio", { name: /Galaxy/ }));
    expect(await screen.findByRole("heading", { name: "Galaxy isn't connected" })).toBeVisible();
    expect(api.listPhotos).toHaveBeenLastCalledWith("phone-2");
  });

  it("asks for a phone when none is paired", async () => {
    renderPage(listed([]), []);
    expect(await screen.findByRole("heading", { name: "No phone paired" })).toBeVisible();
  });
});
