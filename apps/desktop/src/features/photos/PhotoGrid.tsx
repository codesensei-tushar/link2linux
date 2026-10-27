import { ImageOff } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { useDesktopApi, type Device, type Photo } from "@/lib/api";

import { photoKey, takenText } from "./format";
import { thumbnailQueue, thumbnailSize, type ThumbnailQueue } from "./thumbnails";
import { useInView } from "./use-in-view";

/**
 * A `blob:` URL for a thumbnail fetched through `queue` once `wanted`, or
 * null until it has come. The URL is revoked when the tile unmounts, which
 * is when its photo, or that version of it, is no longer shown.
 */
function useThumbnail(queue: ThumbnailQueue, photoId: string, maxSize: number, wanted: boolean) {
  const [url, setUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  // A thumbnail that did not come is asked for again when it comes back
  // into view.
  if (failed && !wanted) setFailed(false);

  useEffect(() => {
    if (!wanted || url !== null || failed) return;
    // Scrolling past withdraws the wish, so only what is on screen is
    // fetched.
    return queue.request(photoId, maxSize, (outcome) => {
      if ("image" in outcome) setUrl(URL.createObjectURL(outcome.image));
      else setFailed(true);
    });
  }, [queue, photoId, maxSize, wanted, url, failed]);

  useEffect(() => {
    if (url === null) return;
    return () => {
      URL.revokeObjectURL(url);
    };
  }, [url]);

  return { url, failed };
}

function PhotoTile({
  photo,
  queue,
  size,
  onOpen,
}: {
  photo: Photo;
  queue: ThumbnailQueue;
  size: number;
  onOpen: (photo: Photo) => void;
}) {
  const [ref, inView] = useInView();
  const { url, failed } = useThumbnail(queue, photo.id, size, inView);
  return (
    <li className="photo-tile">
      <button
        ref={ref}
        type="button"
        className="photo-tile__button"
        aria-label={`${photo.name}, ${takenText(photo)}`}
        onClick={() => {
          onOpen(photo);
        }}
      >
        {url ? (
          <img className="photo-tile__image" src={url} alt="" draggable={false} />
        ) : (
          failed && <ImageOff className="photo-tile__icon" aria-hidden="true" />
        )}
      </button>
    </li>
  );
}

export interface PhotoGridProps {
  phone: Device;
  /** Newest first, as the phone lists them. */
  photos: Photo[];
  onOpen: (photo: Photo) => void;
}

/** The phone's photos as a grid of thumbnails, each fetched once it scrolls into view. */
export function PhotoGrid({ phone, photos, onOpen }: PhotoGridProps) {
  const api = useDesktopApi();
  const queue = useMemo(
    () => thumbnailQueue((photoId, maxSize) => api.photoThumbnail(phone.id, photoId, maxSize)),
    [api, phone.id],
  );
  const size = thumbnailSize("grid", globalThis.devicePixelRatio || 1);
  return (
    <ul className="photo-grid" aria-label={`Photos on ${phone.name}`}>
      {photos.map((photo) => (
        // A new version of a photo is a new tile, with its own thumbnail.
        <PhotoTile key={photoKey(photo)} photo={photo} queue={queue} size={size} onOpen={onOpen} />
      ))}
    </ul>
  );
}
