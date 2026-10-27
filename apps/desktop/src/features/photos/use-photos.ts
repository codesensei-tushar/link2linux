import { useCallback } from "react";

import { useDesktopApi, type PhonePhotos } from "@/lib/api";
import { useLive } from "@/lib/use-live";

/** Follows a phone's photos: the list, and whether it can be shown. */
export function usePhotos(deviceId: string) {
  const api = useDesktopApi();
  const subscribe = useCallback(
    (listener: (photos: PhonePhotos) => void) => api.onPhotosChanged(deviceId, listener),
    [api, deviceId],
  );
  const load = useCallback(() => api.listPhotos(deviceId), [api, deviceId]);
  return useLive(subscribe, load);
}
