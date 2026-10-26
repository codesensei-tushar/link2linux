import { isTauri } from "@tauri-apps/api/core";
import { createContext, use } from "react";

import { demoNotifications } from "@/features/notifications/demo";
import type { MirroredNotification, NotificationEvent } from "@/features/notifications/types";

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

  /** The notifications currently mirrored from a phone, in no particular order. */
  listNotifications(deviceId: string): Promise<MirroredNotification[]>;
  /** Follows changes to a phone's notifications. Returns the unsubscribe function. */
  subscribeNotifications(deviceId: string, onEvent: (event: NotificationEvent) => void): () => void;
  /** Dismisses a notification on the phone. */
  dismissNotification(deviceId: string, key: string): Promise<void>;
  /** Runs one of a notification's actions on the phone, with reply text for replies. */
  runNotificationAction(
    deviceId: string,
    key: string,
    actionId: string,
    replyText?: string,
  ): Promise<void>;
  /** A URL usable in `<img>` for an image the phone shared, or null when unavailable. */
  notificationImage(deviceId: string, sha256: string): Promise<string | null>;
}

/** Stands in for the unsubscribe function when there is nothing to follow. */
function noopUnsubscribe(): void {
  // Nothing was subscribed.
}

/** The real backend: typed IPC calls into src-tauri. */
export const tauriApi: DesktopApi = {
  appInfo: () => commands.appInfo(),
  listDevices: () => commands.listDevices(),
  // TODO(core): call the notifications commands once crates/core exposes them.
  listNotifications: () => Promise.resolve([]),
  // TODO(core): subscribe to the notification events once crates/core emits them.
  subscribeNotifications: () => noopUnsubscribe,
  // TODO(core): send the dismissal through the notifications service.
  dismissNotification: () => Promise.resolve(),
  // TODO(core): send the action through the notifications service.
  runNotificationAction: () => Promise.resolve(),
  // TODO(core): serve the images the phone shared.
  notificationImage: () => Promise.resolve(null),
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
  listNotifications: () => Promise.resolve(demoNotifications(Date.now())),
  subscribeNotifications: () => noopUnsubscribe,
  dismissNotification: () => Promise.resolve(),
  runNotificationAction: () => Promise.resolve(),
  notificationImage: () => Promise.resolve(null),
};

/** The backend for the current environment. */
export const defaultApi: DesktopApi = isTauri() ? tauriApi : browserPreviewApi;

/** Provide a different `DesktopApi` with `<DesktopApiContext value={...}>`. */
export const DesktopApiContext = createContext<DesktopApi>(defaultApi);

export function useDesktopApi(): DesktopApi {
  return use(DesktopApiContext);
}
