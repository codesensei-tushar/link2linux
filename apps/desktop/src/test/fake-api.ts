import { act } from "@testing-library/react";
import { vi, type Mock } from "vitest";

import type { MirroredNotification, NotificationEvent } from "@/features/notifications/types";
import type { AppInfo, DesktopApi, Device, PairingState } from "@/lib/api";

export const testAppInfo: AppInfo = {
  name: "link2linux",
  version: "0.1.0",
  identifier: "dev.link2linux.desktop",
  os: "linux",
  arch: "x86_64",
};

export function testDevice(overrides: Partial<Device> = {}): Device {
  return {
    id: "phone-1",
    name: "Test phone",
    model: null,
    status: "disconnected",
    pairedAtUnixMs: Date.UTC(2026, 9, 1, 12),
    lastConnectedUnixMs: null,
    ...overrides,
  };
}

type Methods = Omit<DesktopApi, "onPairingChanged" | "onDevicesChanged">;

export function testNotification(
  overrides: Partial<MirroredNotification> = {},
): MirroredNotification {
  return {
    key: "n1",
    appName: "Chat",
    appIcon: null,
    whenUnixMs: Date.now(),
    title: "Ada",
    text: "Lunch at noon?",
    subText: "",
    category: "message",
    importance: "default",
    ongoing: false,
    dismissible: true,
    group: "",
    actions: [],
    messages: [],
    conversationTitle: "",
    isGroupConversation: false,
    progress: null,
    largeIcon: null,
    picture: null,
    content: "full",
    visibility: "private",
    ...overrides,
  };
}

export interface FakeApi extends DesktopApi {
  appInfo: Mock<DesktopApi["appInfo"]>;
  listDevices: Mock<DesktopApi["listDevices"]>;
  listNotifications: Mock<DesktopApi["listNotifications"]>;
  subscribeNotifications: Mock<DesktopApi["subscribeNotifications"]>;
  dismissNotification: Mock<DesktopApi["dismissNotification"]>;
  runNotificationAction: Mock<DesktopApi["runNotificationAction"]>;
  notificationImage: Mock<DesktopApi["notificationImage"]>;
  /** Delivers an event to every active notification subscriber. */
  emitNotification: (event: NotificationEvent) => void;
  /** How many notification subscriptions are active. */
  notificationSubscribers: () => number;
  unpairDevice: Mock<DesktopApi["unpairDevice"]>;
  getPairingState: Mock<DesktopApi["getPairingState"]>;
  startPairing: Mock<DesktopApi["startPairing"]>;
  pairWith: Mock<DesktopApi["pairWith"]>;
  answerCode: Mock<DesktopApi["answerCode"]>;
  cancelPairing: Mock<DesktopApi["cancelPairing"]>;
  getSettings: Mock<DesktopApi["getSettings"]>;
  setSettings: Mock<DesktopApi["setSettings"]>;
  /** Pushes a pairing event, as the backend would. */
  emitPairing(state: PairingState): void;
  /** Pushes a devices event, as the backend would. */
  emitDevices(devices: Device[]): void;
  /** How many listeners are attached right now. */
  listenerCount(): number;
}

/** A `DesktopApi` backed by spies. Starts idle with no paired phones. */
export function createFakeApi(): FakeApi {
  const pairingListeners = new Set<(state: PairingState) => void>();
  const deviceListeners = new Set<(devices: Device[]) => void>();
  const subscribers = new Set<(event: NotificationEvent) => void>();
  const ok = () => vi.fn<Methods["startPairing"]>().mockResolvedValue(undefined);

  return {
    appInfo: vi.fn<DesktopApi["appInfo"]>().mockResolvedValue(testAppInfo),
    listDevices: vi.fn<DesktopApi["listDevices"]>().mockResolvedValue([]),
    listNotifications: vi.fn<DesktopApi["listNotifications"]>().mockResolvedValue([]),
    subscribeNotifications: vi.fn<DesktopApi["subscribeNotifications"]>((_deviceId, onEvent) => {
      subscribers.add(onEvent);
      return () => {
        subscribers.delete(onEvent);
      };
    }),
    dismissNotification: vi.fn<DesktopApi["dismissNotification"]>().mockResolvedValue(undefined),
    runNotificationAction: vi
      .fn<DesktopApi["runNotificationAction"]>()
      .mockResolvedValue(undefined),
    notificationImage: vi.fn<DesktopApi["notificationImage"]>().mockResolvedValue(null),
    emitNotification: (event) => {
      for (const subscriber of [...subscribers]) subscriber(event);
    },
    notificationSubscribers: () => subscribers.size,
    unpairDevice: vi.fn<DesktopApi["unpairDevice"]>().mockResolvedValue(undefined),
    getPairingState: vi
      .fn<DesktopApi["getPairingState"]>()
      .mockResolvedValue({ step: "idle" } satisfies PairingState),
    startPairing: ok(),
    pairWith: vi.fn<DesktopApi["pairWith"]>().mockResolvedValue(undefined),
    answerCode: vi.fn<DesktopApi["answerCode"]>().mockResolvedValue(undefined),
    cancelPairing: vi.fn<DesktopApi["cancelPairing"]>().mockResolvedValue(undefined),
    getSettings: vi
      .fn<DesktopApi["getSettings"]>()
      .mockResolvedValue({ showNotificationText: false }),
    setSettings: vi.fn<DesktopApi["setSettings"]>().mockResolvedValue(undefined),
    onPairingChanged: (listener) => {
      pairingListeners.add(listener);
      return Promise.resolve(() => pairingListeners.delete(listener));
    },
    onDevicesChanged: (listener) => {
      deviceListeners.add(listener);
      return Promise.resolve(() => deviceListeners.delete(listener));
    },
    emitPairing: (state) => {
      act(() => {
        pairingListeners.forEach((listener) => {
          listener(state);
        });
      });
    },
    emitDevices: (devices) => {
      act(() => {
        deviceListeners.forEach((listener) => {
          listener(devices);
        });
      });
    },
    listenerCount: () => pairingListeners.size + deviceListeners.size,
  };
}
