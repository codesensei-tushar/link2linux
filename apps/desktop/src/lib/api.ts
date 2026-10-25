import { isTauri } from "@tauri-apps/api/core";
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

/**
 * Stand-in for running the UI in a plain browser (`pnpm dev` without Tauri),
 * where IPC does not exist: no phones, and a version marked as a preview.
 */
export const browserPreviewApi: DesktopApi = {
  appInfo: () =>
    Promise.resolve({
      name: "link2linux",
      version: "browser preview",
      identifier: "dev.link2linux.desktop",
      os: "browser",
      arch: "",
    }),
  listDevices: () => Promise.resolve([]),
};

/** The backend for the current environment. */
export const defaultApi: DesktopApi = isTauri() ? tauriApi : browserPreviewApi;

/** Provide a different `DesktopApi` with `<DesktopApiContext value={...}>`. */
export const DesktopApiContext = createContext<DesktopApi>(defaultApi);

export function useDesktopApi(): DesktopApi {
  return use(DesktopApiContext);
}
