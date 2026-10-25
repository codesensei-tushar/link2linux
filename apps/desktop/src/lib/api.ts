import { createContext, use } from "react";

import { commands, type AppInfo, type Device } from "./bindings";

export type { AppInfo, Device, DeviceStatus } from "./bindings";

/**
 * Everything the UI asks of the desktop backend.
 *
 * Components reach it through `useDesktopApi()`, never through the generated
 * bindings directly, so tests and previews can swap in a fake.
 */
export interface DesktopApi {
  appInfo(): Promise<AppInfo>;
  listDevices(): Promise<Device[]>;
}

/** The real backend: typed IPC calls into src-tauri. */
export const tauriApi: DesktopApi = {
  appInfo: () => commands.appInfo(),
  listDevices: () => commands.listDevices(),
};

/** Provide a different `DesktopApi` with `<DesktopApiContext value={...}>`. */
export const DesktopApiContext = createContext<DesktopApi>(tauriApi);

export function useDesktopApi(): DesktopApi {
  return use(DesktopApiContext);
}
