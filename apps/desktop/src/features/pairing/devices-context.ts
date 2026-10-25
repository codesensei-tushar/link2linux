import { createContext, use } from "react";

import type { Device } from "@/lib/api";
import type { AsyncState } from "@/lib/use-async";

export interface DevicesValue {
  /** Paired phones, loaded once and shared by every screen. */
  devices: AsyncState<Device[]>;
  reload: () => void;
}

export const DevicesContext = createContext<DevicesValue | null>(null);

export function useDevices(): DevicesValue {
  const value = use(DevicesContext);
  if (!value) {
    throw new Error("useDevices() must be called inside <DevicesProvider>");
  }
  return value;
}
