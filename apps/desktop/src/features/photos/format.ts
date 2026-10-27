import { ImageOff, ShieldAlert, Smartphone, Unplug, type LucideIcon } from "lucide-react";

import { formatBytes } from "@/features/files/list";
import type { Photo, PhotosStatus } from "@/lib/api";

/** The states in which the page explains why it shows no photos. */
export type PhotosNoticeState = Exclude<PhotosStatus, "listed" | "waiting">;

/** Why the page shows no photos, and what the user can do about it. */
export function photosNotice(
  state: PhotosNoticeState,
  phone: string,
): { icon: LucideIcon; title: string; description: string } {
  switch (state) {
    case "disconnected":
      return {
        icon: Unplug,
        title: `${phone} isn't connected`,
        description: "Its recent photos show up here while it's connected.",
      };
    case "turnedOff":
      return {
        icon: ImageOff,
        title: `Photos are off on ${phone}`,
        description:
          "To see its recent photos here, turn on Photos in the link2linux app on the phone.",
      };
    case "permissionRequired":
      return {
        icon: ShieldAlert,
        title: `${phone} needs your permission`,
        description:
          "Open the link2linux app on the phone and allow it to see your photos. If you allow only some of them, only those show up here.",
      };
    case "unavailable":
      return {
        icon: ImageOff,
        title: `${phone} isn't sharing photos right now`,
        description: "They show up here again once the link2linux app on the phone can share them.",
      };
    case "unsupported":
      return {
        icon: Smartphone,
        title: `Update link2linux on ${phone}`,
        description: "The app on the phone is too old to share photos with this computer.",
      };
  }
}

/** When a photo was taken, or else last changed, such as "27 Oct 2026, 09:30". */
export function takenText(photo: Photo): string {
  const when = photo.takenUnixMs > 0 ? photo.takenUnixMs : photo.modifiedUnixMs;
  return new Date(when).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/** What the viewer says about a photo: when it was taken, its size in pixels and in bytes. */
export function photoDetails(photo: Photo): string {
  const pixels = photo.width > 0 && photo.height > 0 ? `${photo.width} × ${photo.height}` : null;
  return [takenText(photo), pixels, formatBytes(photo.size)].filter(Boolean).join(" · ");
}

/** Identifies one version of a photo: a new version is a new picture to fetch. */
export function photoKey(photo: Photo): string {
  return `${photo.id}@${photo.modifiedUnixMs}`;
}
