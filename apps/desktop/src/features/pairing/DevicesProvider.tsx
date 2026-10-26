import { useCallback, useMemo, type ReactNode } from "react";

import { useDesktopApi } from "@/lib/api";
import { useLive } from "@/lib/use-live";

import { DevicesContext } from "./devices-context";

/**
 * Follows the paired phones and shares them with the pairing page and the
 * status area. Status changes (connected, reconnecting, ...) arrive as events.
 */
export function DevicesProvider({ children }: { children: ReactNode }) {
  const api = useDesktopApi();
  const subscribe = useCallback(
    (listener: Parameters<typeof api.onDevicesChanged>[0]) => api.onDevicesChanged(listener),
    [api],
  );
  const load = useCallback(() => api.listDevices(), [api]);
  const [devices, reload] = useLive(subscribe, load);
  const value = useMemo(() => ({ devices, reload }), [devices, reload]);

  return <DevicesContext value={value}>{children}</DevicesContext>;
}
