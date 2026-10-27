import { act } from "@testing-library/react";
import { vi, type Mock } from "vitest";

import type { MirroredNotification, NotificationEvent } from "@/features/notifications/types";
import type {
  AppInfo,
  CallsStatus,
  ClipboardStatus,
  DesktopApi,
  Device,
  FilesDropped,
  FileTransfer,
  FileTransferEvent,
  PairingState,
  PhoneStatus,
  RingStatus,
  ScreenStatus,
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

type Methods = Omit<
  DesktopApi,
  "onPairingChanged" | "onDevicesChanged" | "onClipboardChanged" | "onCallsChanged"
>;

/** A desktop that can take calls, with no phones. */
export function testCallsStatus(overrides: Partial<CallsStatus> = {}): CallsStatus {
  return {
    desktop: "ready",
    enabled: true,
    pairing: null,
    phones: [],
    ...overrides,
  };
}

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

/** A connected phone's status with every part known. */
export function testPhoneStatus(
  overrides: Partial<Extract<PhoneStatus, { state: "current" }>> = {},
): PhoneStatus {
  return {
    state: "current",
    battery: { percent: 76, pluggedIn: false, powerSaver: false },
    cellular: { level: 3, network: "5g", operatorName: "Carrier" },
    wifi: { level: 4 },
    ringer: "normal",
    doNotDisturb: false,
    ...overrides,
  };
}

/** A phone that can show its screen and does not. */
export function testScreen(overrides: Partial<ScreenStatus> = {}): ScreenStatus {
  return {
    phase: "idle",
    blocked: null,
    problem: null,
    ended: null,
    format: null,
    inputProblem: null,
    ...overrides,
  };
}

/** A phone that can ring and does not. */
export function testRing(overrides: Partial<RingStatus> = {}): RingStatus {
  return { phase: "idle", blocked: null, problem: null, ended: null, ...overrides };
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
  getPhoneStatus: Mock<DesktopApi["getPhoneStatus"]>;
  getRingStatus: Mock<DesktopApi["getRingStatus"]>;
  ringPhone: Mock<DesktopApi["ringPhone"]>;
  stopRinging: Mock<DesktopApi["stopRinging"]>;
  /** Pushes a phone's status, as the backend would. */
  emitPhoneStatus(deviceId: string, status: PhoneStatus): void;
  /** Pushes a phone's ring status, as the backend would. */
  emitRing(deviceId: string, ring: RingStatus): void;
  getScreenStatus: Mock<DesktopApi["getScreenStatus"]>;
  startScreen: Mock<DesktopApi["startScreen"]>;
  stopScreen: Mock<DesktopApi["stopScreen"]>;
  sendScreenInput: Mock<DesktopApi["sendScreenInput"]>;
  requestScreenKeyFrame: Mock<DesktopApi["requestScreenKeyFrame"]>;
  /** Pushes a phone's screen status, as the backend would. */
  emitScreen(deviceId: string, screen: ScreenStatus): void;
  /** Sends a frame packet to the page that started the screen last. */
  emitScreenPacket(packet: ArrayBuffer): void;
  getCallsStatus: Mock<DesktopApi["getCallsStatus"]>;
  startBluetoothPairing: Mock<DesktopApi["startBluetoothPairing"]>;
  cancelBluetoothPairing: Mock<DesktopApi["cancelBluetoothPairing"]>;
  confirmBluetoothCode: Mock<DesktopApi["confirmBluetoothCode"]>;
  chooseBluetoothDevice: Mock<DesktopApi["chooseBluetoothDevice"]>;
  connectCalls: Mock<DesktopApi["connectCalls"]>;
  answerCall: Mock<DesktopApi["answerCall"]>;
  declineCall: Mock<DesktopApi["declineCall"]>;
  hangUpCall: Mock<DesktopApi["hangUpCall"]>;
  dialNumber: Mock<DesktopApi["dialNumber"]>;
  sendCallTones: Mock<DesktopApi["sendCallTones"]>;
  setCallMuted: Mock<DesktopApi["setCallMuted"]>;
  /** Pushes a calls status, as the backend would. */
  emitCalls(status: CallsStatus): void;
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
  const callsListeners = new Set<(status: CallsStatus) => void>();
  const transferSubscribers = new Set<(event: FileTransferEvent) => void>();
  const dragListeners = new Set<(over: boolean) => void>();
  const dropListeners = new Set<(drop: FilesDropped) => void>();
  const statusListeners = new Set<(deviceId: string, status: PhoneStatus) => void>();
  const ringListeners = new Set<(deviceId: string, ring: RingStatus) => void>();
  const screenListeners = new Set<(deviceId: string, screen: ScreenStatus) => void>();
  let onScreenPacket: ((packet: ArrayBuffer) => void) | null = null;
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
    getPhoneStatus: vi
      .fn<DesktopApi["getPhoneStatus"]>()
      .mockResolvedValue({ state: "waiting" } satisfies PhoneStatus),
    onPhoneStatusChanged: (deviceId, listener) => {
      const forDevice = (id: string, status: PhoneStatus) => {
        if (id === deviceId) listener(status);
      };
      statusListeners.add(forDevice);
      return Promise.resolve(() => statusListeners.delete(forDevice));
    },
    emitPhoneStatus: (deviceId, status) => {
      act(() => {
        statusListeners.forEach((listener) => {
          listener(deviceId, status);
        });
      });
    },
    getRingStatus: vi.fn<DesktopApi["getRingStatus"]>().mockResolvedValue(testRing()),
    onRingChanged: (deviceId, listener) => {
      const forDevice = (id: string, ring: RingStatus) => {
        if (id === deviceId) listener(ring);
      };
      ringListeners.add(forDevice);
      return Promise.resolve(() => ringListeners.delete(forDevice));
    },
    emitRing: (deviceId, ring) => {
      act(() => {
        ringListeners.forEach((listener) => {
          listener(deviceId, ring);
        });
      });
    },
    ringPhone: vi.fn<DesktopApi["ringPhone"]>().mockResolvedValue(undefined),
    stopRinging: vi.fn<DesktopApi["stopRinging"]>().mockResolvedValue(undefined),
    getScreenStatus: vi.fn<DesktopApi["getScreenStatus"]>().mockResolvedValue(testScreen()),
    onScreenChanged: (deviceId, listener) => {
      const forDevice = (id: string, screen: ScreenStatus) => {
        if (id === deviceId) listener(screen);
      };
      screenListeners.add(forDevice);
      return Promise.resolve(() => screenListeners.delete(forDevice));
    },
    startScreen: vi.fn<DesktopApi["startScreen"]>((_deviceId, _codecs, _maxSize, onPacket) => {
      onScreenPacket = onPacket;
      return Promise.resolve();
    }),
    stopScreen: vi.fn<DesktopApi["stopScreen"]>().mockResolvedValue(undefined),
    sendScreenInput: vi.fn<DesktopApi["sendScreenInput"]>().mockResolvedValue(undefined),
    requestScreenKeyFrame: vi.fn<DesktopApi["requestScreenKeyFrame"]>().mockResolvedValue(undefined),
    emitScreen: (deviceId, screen) => {
      act(() => {
        screenListeners.forEach((listener) => {
          listener(deviceId, screen);
        });
      });
    },
    emitScreenPacket: (packet) => {
      act(() => {
        onScreenPacket?.(packet);
      });
    },
    getCallsStatus: vi.fn<DesktopApi["getCallsStatus"]>().mockResolvedValue(testCallsStatus()),
    onCallsChanged: (listener) => {
      callsListeners.add(listener);
      return Promise.resolve(() => callsListeners.delete(listener));
    },
    startBluetoothPairing: vi
      .fn<DesktopApi["startBluetoothPairing"]>()
      .mockResolvedValue(undefined),
    cancelBluetoothPairing: vi
      .fn<DesktopApi["cancelBluetoothPairing"]>()
      .mockResolvedValue(undefined),
    confirmBluetoothCode: vi.fn<DesktopApi["confirmBluetoothCode"]>().mockResolvedValue(undefined),
    chooseBluetoothDevice: vi
      .fn<DesktopApi["chooseBluetoothDevice"]>()
      .mockResolvedValue(undefined),
    connectCalls: vi.fn<DesktopApi["connectCalls"]>().mockResolvedValue(undefined),
    answerCall: vi.fn<DesktopApi["answerCall"]>().mockResolvedValue(undefined),
    declineCall: vi.fn<DesktopApi["declineCall"]>().mockResolvedValue(undefined),
    hangUpCall: vi.fn<DesktopApi["hangUpCall"]>().mockResolvedValue(undefined),
    dialNumber: vi.fn<DesktopApi["dialNumber"]>().mockResolvedValue(undefined),
    sendCallTones: vi.fn<DesktopApi["sendCallTones"]>().mockResolvedValue(undefined),
    setCallMuted: vi.fn<DesktopApi["setCallMuted"]>().mockResolvedValue(undefined),
    emitCalls: (status) => {
      act(() => {
        callsListeners.forEach((listener) => {
          listener(status);
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
      callsListeners.size +
      dragListeners.size +
      dropListeners.size +
      statusListeners.size +
      ringListeners.size +
      screenListeners.size,
  };
}
