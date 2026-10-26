import { vi, type Mock } from "vitest";

import type { MirroredNotification, NotificationEvent } from "@/features/notifications/types";
import type { AppInfo, DesktopApi, Device } from "@/lib/api";

export const testAppInfo: AppInfo = {
  name: "link2linux",
  version: "0.1.0",
  identifier: "dev.link2linux.desktop",
  os: "linux",
  arch: "x86_64",
};

export function testDevice(overrides: Partial<Device> = {}): Device {
  return { id: "phone-1", name: "Test phone", model: null, status: "disconnected", ...overrides };
}

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
}

/** A `DesktopApi` backed by spies. Starts with no paired phones. */
export function createFakeApi(): FakeApi {
  const subscribers = new Set<(event: NotificationEvent) => void>();
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
  };
}
