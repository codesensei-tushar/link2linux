import { isTauri } from "@tauri-apps/api/core";
import { createContext, use } from "react";

import type { MirroredNotification, NotificationEvent } from "@/features/notifications/types";

import {
  commands,
  events,
  type AppInfo,
  type CommandError,
  type Device,
  type PairingMethod,
  type PairingState,
  type PhoneTarget,
} from "./bindings";
import { createPreviewApi } from "./preview-api";

export type {
  AppInfo,
  Device,
  DeviceStatus,
  DiscoveredPhone,
  PairingFailure,
  PairingMethod,
  PairingState,
  PhoneTarget,
} from "./bindings";

/** Stops listening to an event stream. */
export type Unsubscribe = () => void;

/**
 * Everything the UI asks of the desktop backend.
 *
 * Components reach it through `useDesktopApi()`, never through the generated
 * bindings directly, so tests and previews can swap in a fake.
 *
 * Commands that start or answer something resolve with nothing: the new state
 * arrives through `onPairingChanged` and `onDevicesChanged`. They reject with
 * an `Error` whose message is fit to show.
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

  unpairDevice(deviceId: string): Promise<void>;
  getPairingState(): Promise<PairingState>;
  startPairing(method: PairingMethod): Promise<void>;
  pairWith(target: PhoneTarget): Promise<void>;
  answerCode(matches: boolean): Promise<void>;
  cancelPairing(): Promise<void>;
  /** Subscribe before reading a snapshot, so nothing is missed in between. */
  onPairingChanged(listener: (state: PairingState) => void): Promise<Unsubscribe>;
  onDevicesChanged(listener: (devices: Device[]) => void): Promise<Unsubscribe>;
}

type Outcome = { status: "ok"; data: null } | { status: "error"; error: CommandError };

async function expectOk(result: Promise<Outcome>): Promise<void> {
  const outcome = await result;
  if (outcome.status === "error") throw new Error(outcome.error.message);
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
  unpairDevice: (deviceId) => expectOk(commands.unpairDevice(deviceId)),
  getPairingState: () => commands.getPairingState(),
  startPairing: (method) => expectOk(commands.startPairing(method)),
  pairWith: (target) => expectOk(commands.pairWith(target)),
  answerCode: (matches) => expectOk(commands.answerCode(matches)),
  cancelPairing: () => commands.cancelPairing(),
  onPairingChanged: (listener) =>
    events.pairingChanged.listen((event) => {
      listener(event.payload);
    }),
  onDevicesChanged: (listener) =>
    events.devicesChanged.listen((event) => {
      listener(event.payload);
    }),
};

/** The backend for the current environment. */
export const defaultApi: DesktopApi = isTauri() ? tauriApi : createPreviewApi();

/** Provide a different `DesktopApi` with `<DesktopApiContext value={...}>`. */
export const DesktopApiContext = createContext<DesktopApi>(defaultApi);

export function useDesktopApi(): DesktopApi {
  return use(DesktopApiContext);
}
