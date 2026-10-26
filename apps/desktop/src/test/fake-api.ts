import { act } from "@testing-library/react";
import { vi, type Mock } from "vitest";

import type { MirroredNotification, NotificationEvent } from "@/features/notifications/types";
import type {
  AppInfo,
  ClipboardStatus,
  DesktopApi,
  Device,
  FilesDropped,
  FileTransfer,
  FileTransferEvent,
  PairingState,
} from "@/lib/api";

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

type Methods = Omit<DesktopApi, "onPairingChanged" | "onDevicesChanged" | "onClipboardChanged">;

/** A clipboard shared automatically, with no phones and nothing sent yet. */
export function testClipboardStatus(overrides: Partial<ClipboardStatus> = {}): ClipboardStatus {
  return {
    mode: "automatic",
    phones: [],
    activity: [],
    problem: null,
    held: null,
    ...overrides,
  };
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

export function testTransfer(overrides: Partial<FileTransfer> = {}): FileTransfer {
  return {
    id: "t1",
    direction: "incoming",
    status: { kind: "running" },
    files: [{ name: "photo.jpg", size: 2_000_000, mimeType: "image/jpeg", saved: false }],
    size: 2_000_000,
    done: 500_000,
    startedUnixMs: Date.now(),
    ...overrides,
  };
}

export interface FakeApi extends DesktopApi {
  appInfo: Mock<DesktopApi["appInfo"]>;
  startupError: Mock<DesktopApi["startupError"]>;
  restartApp: Mock<DesktopApi["restartApp"]>;
  listDevices: Mock<DesktopApi["listDevices"]>;
  listNotifications: Mock<DesktopApi["listNotifications"]>;
  subscribeNotifications: Mock<DesktopApi["subscribeNotifications"]>;
  dismissNotification: Mock<DesktopApi["dismissNotification"]>;
  runNotificationAction: Mock<DesktopApi["runNotificationAction"]>;
  notificationImage: Mock<DesktopApi["notificationImage"]>;
  getClipboardStatus: Mock<DesktopApi["getClipboardStatus"]>;
  sendClipboard: Mock<DesktopApi["sendClipboard"]>;
  /** Pushes a clipboard status, as the backend would. */
  emitClipboard(status: ClipboardStatus): void;
  /** Delivers an event to every active notification subscriber. */
  emitNotification: (event: NotificationEvent) => void;
  /** How many notification subscriptions are active. */
  notificationSubscribers: () => number;
  listTransfers: Mock<DesktopApi["listTransfers"]>;
  subscribeTransfers: Mock<DesktopApi["subscribeTransfers"]>;
  pickFilesToSend: Mock<DesktopApi["pickFilesToSend"]>;
  sendDroppedFiles: Mock<DesktopApi["sendDroppedFiles"]>;
  cancelTransfer: Mock<DesktopApi["cancelTransfer"]>;
  forgetTransfer: Mock<DesktopApi["forgetTransfer"]>;
  openReceivedFile: Mock<DesktopApi["openReceivedFile"]>;
  showReceivedFile: Mock<DesktopApi["showReceivedFile"]>;
  /** Delivers an event to every active transfer subscriber. */
  emitTransfer: (event: FileTransferEvent) => void;
  /** Says files are dragged over the window, or not anymore. */
  emitFileDrag: (over: boolean) => void;
  /** Says files were dropped on the window. */
  emitFilesDropped: (drop: FilesDropped) => void;
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
  const clipboardListeners = new Set<(status: ClipboardStatus) => void>();
  const transferSubscribers = new Set<(event: FileTransferEvent) => void>();
  const dragListeners = new Set<(over: boolean) => void>();
  const dropListeners = new Set<(drop: FilesDropped) => void>();
  const ok = () => vi.fn<Methods["startPairing"]>().mockResolvedValue(undefined);

  return {
    appInfo: vi.fn<DesktopApi["appInfo"]>().mockResolvedValue(testAppInfo),
    startupError: vi.fn<DesktopApi["startupError"]>().mockResolvedValue(null),
    restartApp: vi.fn<DesktopApi["restartApp"]>().mockResolvedValue(undefined),
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
    getClipboardStatus: vi
      .fn<DesktopApi["getClipboardStatus"]>()
      .mockResolvedValue(testClipboardStatus()),
    onClipboardChanged: (listener) => {
      clipboardListeners.add(listener);
      return Promise.resolve(() => clipboardListeners.delete(listener));
    },
    sendClipboard: vi.fn<DesktopApi["sendClipboard"]>().mockResolvedValue(undefined),
    emitClipboard: (status) => {
      act(() => {
        clipboardListeners.forEach((listener) => {
          listener(status);
        });
      });
    },
    emitNotification: (event) => {
      for (const subscriber of [...subscribers]) subscriber(event);
    },
    notificationSubscribers: () => subscribers.size,
    listTransfers: vi.fn<DesktopApi["listTransfers"]>().mockResolvedValue([]),
    subscribeTransfers: vi.fn<DesktopApi["subscribeTransfers"]>((_deviceId, onEvent) => {
      transferSubscribers.add(onEvent);
      return () => {
        transferSubscribers.delete(onEvent);
      };
    }),
    pickFilesToSend: vi.fn<DesktopApi["pickFilesToSend"]>().mockResolvedValue(true),
    sendDroppedFiles: vi.fn<DesktopApi["sendDroppedFiles"]>().mockResolvedValue(undefined),
    cancelTransfer: vi.fn<DesktopApi["cancelTransfer"]>().mockResolvedValue(undefined),
    forgetTransfer: vi.fn<DesktopApi["forgetTransfer"]>().mockResolvedValue(undefined),
    openReceivedFile: vi.fn<DesktopApi["openReceivedFile"]>().mockResolvedValue(undefined),
    showReceivedFile: vi.fn<DesktopApi["showReceivedFile"]>().mockResolvedValue(undefined),
    emitTransfer: (event) => {
      act(() => {
        for (const subscriber of [...transferSubscribers]) subscriber(event);
      });
    },
    onFileDrag: (listener) => {
      dragListeners.add(listener);
      return Promise.resolve(() => dragListeners.delete(listener));
    },
    onFilesDropped: (listener) => {
      dropListeners.add(listener);
      return Promise.resolve(() => dropListeners.delete(listener));
    },
    emitFileDrag: (over) => {
      act(() => {
        dragListeners.forEach((listener) => {
          listener(over);
        });
      });
    },
    emitFilesDropped: (drop) => {
      act(() => {
        dropListeners.forEach((listener) => {
          listener(drop);
        });
      });
    },
    unpairDevice: vi.fn<DesktopApi["unpairDevice"]>().mockResolvedValue(undefined),
    getPairingState: vi
      .fn<DesktopApi["getPairingState"]>()
      .mockResolvedValue({ step: "idle" } satisfies PairingState),
    startPairing: ok(),
    pairWith: vi.fn<DesktopApi["pairWith"]>().mockResolvedValue(undefined),
    answerCode: vi.fn<DesktopApi["answerCode"]>().mockResolvedValue(undefined),
    cancelPairing: vi.fn<DesktopApi["cancelPairing"]>().mockResolvedValue(undefined),
    getSettings: vi.fn<DesktopApi["getSettings"]>().mockResolvedValue({
      showNotificationText: false,
      shareClipboard: true,
      sharePasswords: false,
    }),
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
    listenerCount: () =>
      pairingListeners.size +
      deviceListeners.size +
      clipboardListeners.size +
      dragListeners.size +
      dropListeners.size,
  };
}
