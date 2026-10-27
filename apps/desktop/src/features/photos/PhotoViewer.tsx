import {
  ChevronLeft,
  ChevronRight,
  CircleCheck,
  Download,
  ExternalLink,
  FolderOpen,
  RefreshCw,
  X,
} from "lucide-react";
import { useEffect, useEffectEvent, useId, useRef, useState } from "react";

import { Button, Spinner } from "@/components/ui";
import { useRun } from "@/features/calls/use-run";
import { useDesktopApi, type Device, type Photo, type SavedPhoto } from "@/lib/api";
import { errorMessage } from "@/lib/utils";

import { photoDetails, photoKey } from "./format";
import { thumbnailSize } from "./thumbnails";

type Preview = { key: string; url: string } | { key: string; error: string };

/**
 * How long the viewer waits before it asks for a photo's picture. A request
 * can't be taken back, so stepping quickly through the photos fetches only
 * the one the user stops at.
 */
const PREVIEW_DELAY_MS = 150;

/**
 * A larger picture of `photo` for the viewer, as a `blob:` URL that is
 * revoked once another photo, or none, is shown.
 */
function usePreview(deviceId: string, photo: Photo) {
  const api = useDesktopApi();
  const key = photoKey(photo);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let current = true;
    let url: string | null = null;
    const maxSize = thumbnailSize("viewer", globalThis.devicePixelRatio || 1);
    const timer = setTimeout(() => {
      api.photoThumbnail(deviceId, photo.id, maxSize).then(
        (image) => {
          if (!current) return;
          url = URL.createObjectURL(image);
          setPreview({ key, url });
        },
        (cause: unknown) => {
          if (current) setPreview({ key, error: errorMessage(cause) });
        },
      );
    }, PREVIEW_DELAY_MS);
    return () => {
      current = false;
      clearTimeout(timer);
      if (url !== null) URL.revokeObjectURL(url);
    };
  }, [api, deviceId, photo.id, key, attempt]);

  // What came for the photo shown before is not this one's.
  const shown = preview?.key === key ? preview : null;
  const retry = () => {
    setPreview(null);
    setAttempt((n) => n + 1);
  };
  return { preview: shown, retry };
}

/** Keeps Tab and Shift+Tab within `container`, and brings focus back into it. */
function keepFocusIn(event: KeyboardEvent, container: HTMLElement) {
  const buttons = [...container.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
  const first = buttons[0];
  const last = buttons.at(-1);
  const inside = buttons.some((button) => button === document.activeElement);
  if (!inside || (event.shiftKey && document.activeElement === first)) {
    event.preventDefault();
    (event.shiftKey ? last : first)?.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first?.focus();
  }
}

export interface PhotoViewerProps {
  phone: Device;
  /** The photos the viewer steps through, newest first. */
  photos: Photo[];
  photo: Photo;
  /** What this version was saved as during this run of the app. */
  saved: SavedPhoto | undefined;
  onSaved: (photo: Photo, saved: SavedPhoto) => void;
  /** Shows another photo instead. */
  onSelect: (photo: Photo) => void;
  onClose: () => void;
}

/**
 * One photo, larger, with a way to save it on this computer and then open
 * it or show it in its folder. A modal dialog: Escape closes it, the arrow
 * keys step through the photos, Tab stays inside, and focus goes back to the
 * photo that opened it.
 */
export function PhotoViewer({
  phone,
  photos,
  photo,
  saved,
  onSaved,
  onSelect,
  onClose,
}: PhotoViewerProps) {
  const api = useDesktopApi();
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const [error, setError] = useState<string | null>(null);
  const { run, busy } = useRun(setError);
  const { preview, retry } = usePreview(phone.id, photo);

  // A failure belongs to the photo it happened with.
  const [errorFor, setErrorFor] = useState(photo.id);
  if (errorFor !== photo.id) {
    setErrorFor(photo.id);
    setError(null);
  }

  useEffect(() => {
    const opener = document.activeElement;
    closeRef.current?.focus();
    return () => {
      if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
    };
  }, []);

  const index = photos.findIndex((p) => p.id === photo.id);
  const previous = index > 0 ? photos[index - 1] : undefined;
  const next = index >= 0 ? photos[index + 1] : undefined;

  // Listens on the document: stepping to the last photo disables the
  // button that had focus, and focus leaves the dialog with it.
  const onKeyDown = useEffectEvent((event: KeyboardEvent) => {
    if (event.key === "Escape") {
      // Closes the viewer and nothing else that listens for Escape.
      event.preventDefault();
      event.stopPropagation();
      onClose();
    } else if (event.key === "ArrowLeft" && previous) {
      onSelect(previous);
    } else if (event.key === "ArrowRight" && next) {
      onSelect(next);
    } else if (event.key === "Tab" && dialogRef.current) {
      keepFocusIn(event, dialogRef.current);
    }
  });

  useEffect(() => {
    const listener = (event: KeyboardEvent) => {
      onKeyDown(event);
    };
    document.addEventListener("keydown", listener, true);
    return () => {
      document.removeEventListener("keydown", listener, true);
    };
  }, []);

  const save = () => {
    run(() =>
      api.savePhoto(phone.id, photo.id).then((result) => {
        onSaved(photo, result);
      }),
    );
  };

  let picture;
  if (!preview) {
    picture = <Spinner label="Loading the photo…" />;
  } else if ("error" in preview) {
    picture = (
      <div className="photo-viewer__problem">
        <p role="alert">{preview.error}</p>
        <Button size="sm" icon={RefreshCw} onClick={retry}>
          Try again
        </Button>
      </div>
    );
  } else {
    picture = <img className="photo-viewer__image" src={preview.url} alt={photo.name} />;
  }

  return (
    <div className="photo-viewer-backdrop" role="presentation">
      <div
        ref={dialogRef}
        className="photo-viewer"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
      >
        <header className="photo-viewer__header">
          <div>
            <h2 id={titleId} className="photo-viewer__title">
              {photo.name}
            </h2>
            <p className="photo-viewer__details">{photoDetails(photo)}</p>
          </div>
          <Button ref={closeRef} variant="ghost" icon={X} aria-label="Close" onClick={onClose} />
        </header>

        <div className="photo-viewer__stage">
          <Button
            variant="ghost"
            icon={ChevronLeft}
            aria-label="Previous photo"
            disabled={!previous}
            onClick={() => {
              if (previous) onSelect(previous);
            }}
          />
          <div className="photo-viewer__picture">{picture}</div>
          <Button
            variant="ghost"
            icon={ChevronRight}
            aria-label="Next photo"
            disabled={!next}
            onClick={() => {
              if (next) onSelect(next);
            }}
          />
        </div>

        <footer className="photo-viewer__actions">
          <p className="photo-viewer__saved" aria-live="polite">
            {saved && (
              <>
                <CircleCheck aria-hidden="true" />
                {`Saved as ${saved.name} in ${saved.folder}`}
              </>
            )}
          </p>
          {saved ? (
            <>
              <Button
                icon={ExternalLink}
                onClick={() => {
                  run(() => api.openSavedPhoto(phone.id, photo.id));
                }}
              >
                Open
              </Button>
              <Button
                icon={FolderOpen}
                onClick={() => {
                  run(() => api.showSavedPhoto(phone.id, photo.id));
                }}
              >
                Show in folder
              </Button>
            </>
          ) : (
            <Button variant="primary" icon={Download} disabled={busy} onClick={save}>
              {busy ? "Saving…" : "Save"}
            </Button>
          )}
          {error && (
            <p role="alert" className="photo-viewer__error">
              {error}
            </p>
          )}
        </footer>
      </div>
    </div>
  );
}
