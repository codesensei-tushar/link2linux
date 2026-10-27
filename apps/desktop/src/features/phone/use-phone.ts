import { useCallback } from "react";

import { useDesktopApi, type PhoneStatus, type RingStatus } from "@/lib/api";
import { useLive } from "@/lib/use-live";

/** Follows a phone's status, or null until it is known. */
export function usePhoneStatus(deviceId: string): PhoneStatus | null {
  const api = useDesktopApi();
  const subscribe = useCallback(
    (listener: (status: PhoneStatus) => void) => api.onPhoneStatusChanged(deviceId, listener),
    [api, deviceId],
  );
  const load = useCallback(() => api.getPhoneStatus(deviceId), [api, deviceId]);
  const [state] = useLive(subscribe, load);
  return state.status === "ready" ? state.data : null;
}

/** Follows ringing a phone, or null until it is known. */
export function useRingStatus(deviceId: string): RingStatus | null {
  const api = useDesktopApi();
  const subscribe = useCallback(
    (listener: (ring: RingStatus) => void) => api.onRingChanged(deviceId, listener),
    [api, deviceId],
  );
  const load = useCallback(() => api.getRingStatus(deviceId), [api, deviceId]);
  const [state] = useLive(subscribe, load);
  return state.status === "ready" ? state.data : null;
}
