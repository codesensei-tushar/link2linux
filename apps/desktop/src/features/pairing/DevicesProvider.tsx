import { useCallback, useMemo, type ReactNode } from "react";

import { useDesktopApi } from "@/lib/api";
import { useAsync } from "@/lib/use-async";

import { DevicesContext } from "./devices-context";

/** Loads the paired phones and shares them with the pairing page and the status area. */
export function DevicesProvider({ children }: { children: ReactNode }) {
  const api = useDesktopApi();
  const load = useCallback(() => api.listDevices(), [api]);
  const [devices, reload] = useAsync(load);
  const value = useMemo(() => ({ devices, reload }), [devices, reload]);

  return <DevicesContext value={value}>{children}</DevicesContext>;
}
