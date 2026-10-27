import { Channel, isTauri } from "@tauri-apps/api/core";
import { createContext, use } from "react";

import type { MirroredNotification, NotificationEvent } from "@/features/notifications/types";

import {
  commands,
  events,
  type AppInfo,
  type CallsStatus,
  type ClipboardStatus,
  type CommandError,
  type Device,
  type FilesDropped,
  type FileTransfer,
  type FileTransferEvent,
  type PairingMethod,
  type PairingState,
  type PhoneStatus,
  type PhoneTarget,
  type RingStatus,
  type ScreenCodec,
  type ScreenInput,
  type ScreenStatus,
  type Settings,
  type StartupError,
} from "./bindings";
import { createPreviewApi } from "./preview-api";

export type {
  AppInfo,
  BluetoothChoice,
  BluetoothPairing,
  CallLink,
  CallProblem,
  CallStateView,
  CallView,
  CallsDesktop,
  CallsStatus,
  PairingCode,
  PairingRequestState,
  PhoneCalls,
  ClipboardActivity,
  ClipboardMode,
  ClipboardPhone,
  ClipboardPhoneState,
  ClipboardProblem,
  ClipboardProblemKind,
  ClipboardStatus,
  Device,
  DeviceStatus,
  DiscoveredPhone,
  FilesDropped,
  FileTransfer,
  FileTransferEvent,
  PairingFailure,
  PairingMethod,
  PairingState,
  PhoneBattery,
  PhoneCellular,
  PhoneStatus,
  PhoneTarget,
  PhoneWifi,
  RingEnd,
  RingerMode,
  RingPhase,
  RingProblem,
  RingStatus,
  ScreenCodec,
  ScreenEnd,
  ScreenFormat,
  ScreenInput,
  ScreenInputProblem,
  ScreenKey,
  ScreenPhase,
  ScreenPoint,
  ScreenProblem,
  ScreenStatus,
  Settings,
  StartupError,
  TransferDirection,
  TransferFailure,
  TransferFileInfo,
  TransferStatus,
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
  /** Why the backend did not start, or null when it did. */
  startupError(): Promise<StartupError | null>;
  /** Starts the app again; on success the promise never settles. */
  restartApp(): Promise<void>;
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

  /** How the clipboard is shared, each phone's state and what went where lately. */
  getClipboardStatus(): Promise<ClipboardStatus>;
  /** Subscribe before reading a snapshot, so nothing is missed in between. */
  onClipboardChanged(listener: (status: ClipboardStatus) => void): Promise<Unsubscribe>;
  /** Sends what the clipboard holds to every phone. What came of it arrives as a new status. */
  sendClipboard(): Promise<void>;
  /** The file transfers with a phone, oldest first. */
  listTransfers(deviceId: string): Promise<FileTransfer[]>;
  /** Follows changes to a phone's transfers. Returns the unsubscribe function. */
  subscribeTransfers(deviceId: string, onEvent: (event: FileTransferEvent) => void): () => void;
  /** Asks for files with the desktop's file chooser and sends them; resolves with whether any were chosen. */
  pickFilesToSend(deviceId: string): Promise<boolean>;
  /** Sends the files of a drop on the window, named by its `dropId`, to a phone. */
  sendDroppedFiles(dropId: number, deviceId: string): Promise<void>;
  /** Cancels a transfer that has not finished, in either direction. */
  cancelTransfer(deviceId: string, transferId: string): Promise<void>;
  /** Removes a finished transfer from the list. Received files stay. */
  forgetTransfer(deviceId: string, transferId: string): Promise<void>;
  /** Opens a file that arrived from a phone with the desktop's default application. */
  openReceivedFile(deviceId: string, transferId: string, fileIndex: number): Promise<void>;
  /** Shows a file that arrived from a phone in its folder. */
  showReceivedFile(deviceId: string, transferId: string, fileIndex: number): Promise<void>;
  /** Files are dragged over the window (`true`), or not anymore. */
  onFileDrag(listener: (over: boolean) => void): Promise<Unsubscribe>;
  /** Files were dropped on the window. */
  onFilesDropped(listener: (drop: FilesDropped) => void): Promise<Unsubscribe>;

  /** A phone's battery, signal and sound mode. */
  getPhoneStatus(deviceId: string): Promise<PhoneStatus>;
  /** Subscribe before reading a snapshot, so nothing is missed in between. */
  onPhoneStatusChanged(
    deviceId: string,
    listener: (status: PhoneStatus) => void,
  ): Promise<Unsubscribe>;
  /** Whether a phone rings, and why it cannot or did not. */
  getRingStatus(deviceId: string): Promise<RingStatus>;
  /** Subscribe before reading a snapshot, so nothing is missed in between. */
  onRingChanged(deviceId: string, listener: (ring: RingStatus) => void): Promise<Unsubscribe>;
  /** Makes a phone ring for a minute, even when it is set to silent. What came of it arrives as a new ring status. */
  ringPhone(deviceId: string): Promise<void>;
  /** Stops a phone ringing. */
  stopRinging(deviceId: string): Promise<void>;

  /** What the page shows of a phone's screen. */
  getScreenStatus(deviceId: string): Promise<ScreenStatus>;
  onScreenChanged(deviceId: string, listener: (screen: ScreenStatus) => void): Promise<Unsubscribe>;
  /**
   * Asks a phone to show its screen, in one of `codecs` (most preferred first) with its longer
   * side at most `maxSize` pixels. The phone asks its user first. Each frame arrives at
   * `onPacket` (see `features/screen/packet.ts`) until the stream ends; how it goes arrives as
   * new screen statuses.
   */
  startScreen(
    deviceId: string,
    codecs: ScreenCodec[],
    maxSize: number,
    onPacket: (packet: ArrayBuffer) => void,
  ): Promise<void>;
  stopScreen(deviceId: string): Promise<void>;
  /** Sends what the user did on the phone's screen. A refusal shows in the screen status. */
  sendScreenInput(deviceId: string, input: ScreenInput): Promise<void>;
  /** Asks for a key frame, after a frame could not be decoded or the page fell behind. */
  requestScreenKeyFrame(deviceId: string): Promise<void>;
  /** The phones' calls and this computer's part in them. */
  getCallsStatus(): Promise<CallsStatus>;
  /** Subscribe before reading a snapshot, so nothing is missed in between. */
  onCallsChanged(listener: (status: CallsStatus) => void): Promise<Unsubscribe>;
  /** Opens Bluetooth pairing with a phone for 2 minutes and asks the phone to pair. */
  startBluetoothPairing(deviceId: string): Promise<void>;
  cancelBluetoothPairing(): Promise<void>;
  /** Answers a pairing prompt: whether both devices show the same code. */
  confirmBluetoothCode(promptId: number, matches: boolean): Promise<void>;
  /** Picks the paired Bluetooth device at `address` as the phone's. */
  chooseBluetoothDevice(deviceId: string, address: string): Promise<void>;
  /** Connects the phone's hands-free link with this computer. */
  connectCalls(deviceId: string): Promise<void>;
  answerCall(deviceId: string, callId: string): Promise<void>;
  declineCall(deviceId: string, callId: string): Promise<void>;
  hangUpCall(deviceId: string, callId: string): Promise<void>;
  /** Calls a number from the phone. */
  dialNumber(deviceId: string, number: string): Promise<void>;
  /** Sends touch tones in the phone's active call. */
  sendCallTones(deviceId: string, tones: string): Promise<void>;
  setCallMuted(deviceId: string, muted: boolean): Promise<void>;

  unpairDevice(deviceId: string): Promise<void>;
  getPairingState(): Promise<PairingState>;
  startPairing(method: PairingMethod): Promise<void>;
  pairWith(target: PhoneTarget): Promise<void>;
  answerCode(matches: boolean): Promise<void>;
  cancelPairing(): Promise<void>;
  /** Subscribe before reading a snapshot, so nothing is missed in between. */
  onPairingChanged(listener: (state: PairingState) => void): Promise<Unsubscribe>;
  onDevicesChanged(listener: (devices: Device[]) => void): Promise<Unsubscribe>;

  getSettings(): Promise<Settings>;
  /** Applies the settings at once. Rejects when they could not be saved for the next start. */
  setSettings(settings: Settings): Promise<void>;
}

type Outcome = { status: "ok"; data: null } | { status: "error"; error: CommandError };

async function expectOk(result: Promise<Outcome>): Promise<void> {
  const outcome = await result;
  if (outcome.status === "error") throw new Error(outcome.error.message);
}

type DataOutcome<T> = { status: "ok"; data: T } | { status: "error"; error: CommandError };

async function expectData<T>(result: Promise<DataOutcome<T>>): Promise<T> {
  const outcome = await result;
  if (outcome.status === "error") throw new Error(outcome.error.message);
  return outcome.data;
}

/** The real backend: typed IPC calls into src-tauri. */
export const tauriApi: DesktopApi = {
  appInfo: () => commands.appInfo(),
  startupError: () => commands.startupError(),
  restartApp: () => commands.restartApp(),
  listDevices: () => commands.listDevices(),
  listNotifications: (deviceId) => expectData(commands.listNotifications(deviceId)),
  subscribeNotifications: (deviceId, onEvent) => {
    // `listen` is asynchronous, but the unsubscribe function is needed at once.
    let active = true;
    let unlisten: (() => void) | undefined;
    void events.notificationChanged
      .listen((event) => {
        if (active && event.payload.deviceId === deviceId) onEvent(event.payload.event);
      })
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      });
    return () => {
      active = false;
      unlisten?.();
    };
  },
  dismissNotification: (deviceId, key) => expectOk(commands.dismissNotification(deviceId, key)),
  runNotificationAction: (deviceId, key, actionId, replyText) =>
    expectOk(commands.runNotificationAction(deviceId, key, actionId, replyText ?? null)),
  notificationImage: (deviceId, sha256) => expectData(commands.notificationImage(deviceId, sha256)),
  getClipboardStatus: () => commands.getClipboardStatus(),
  onClipboardChanged: (listener) =>
    events.clipboardChanged.listen((event) => {
      listener(event.payload);
    }),
  sendClipboard: () => expectOk(commands.sendClipboard()),
  listTransfers: (deviceId) => expectData(commands.listTransfers(deviceId)),
  subscribeTransfers: (deviceId, onEvent) => {
    let active = true;
    let unlisten: (() => void) | undefined;
    void events.transferChanged
      .listen((event) => {
        if (active && event.payload.deviceId === deviceId) onEvent(event.payload.event);
      })
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      });
    return () => {
      active = false;
      unlisten?.();
    };
  },
  pickFilesToSend: (deviceId) => expectData(commands.pickFilesToSend(deviceId)),
  sendDroppedFiles: (dropId, deviceId) => expectOk(commands.sendDroppedFiles(dropId, deviceId)),
  cancelTransfer: (deviceId, transferId) => expectOk(commands.cancelTransfer(deviceId, transferId)),
  forgetTransfer: (deviceId, transferId) => expectOk(commands.forgetTransfer(deviceId, transferId)),
  openReceivedFile: (deviceId, transferId, fileIndex) =>
    expectOk(commands.openReceivedFile(deviceId, transferId, fileIndex)),
  showReceivedFile: (deviceId, transferId, fileIndex) =>
    expectOk(commands.showReceivedFile(deviceId, transferId, fileIndex)),
  onFileDrag: (listener) =>
    events.fileDrag.listen((event) => {
      listener(event.payload.over);
    }),
  onFilesDropped: (listener) =>
    events.filesDropped.listen((event) => {
      listener(event.payload);
    }),
  getPhoneStatus: (deviceId) => expectData(commands.getPhoneStatus(deviceId)),
  onPhoneStatusChanged: (deviceId, listener) =>
    events.phoneStatusChanged.listen((event) => {
      if (event.payload.deviceId === deviceId) listener(event.payload.status);
    }),
  getRingStatus: (deviceId) => expectData(commands.getRingStatus(deviceId)),
  onRingChanged: (deviceId, listener) =>
    events.ringChanged.listen((event) => {
      if (event.payload.deviceId === deviceId) listener(event.payload.ring);
    }),
  ringPhone: (deviceId) => expectOk(commands.ringPhone(deviceId)),
  stopRinging: (deviceId) => expectOk(commands.stopRinging(deviceId)),
  getScreenStatus: (deviceId) => expectData(commands.getScreenStatus(deviceId)),
  onScreenChanged: (deviceId, listener) =>
    events.screenChanged.listen((event) => {
      if (event.payload.deviceId === deviceId) listener(event.payload.screen);
    }),
  startScreen: (deviceId, codecs, maxSize, onPacket) => {
    // Frames travel as raw bytes and arrive as ArrayBuffers.
    const frames = new Channel<unknown>((message) => {
      if (message instanceof ArrayBuffer) onPacket(message);
    });
    return expectOk(commands.startScreen(deviceId, codecs, maxSize, frames));
  },
  stopScreen: (deviceId) => expectOk(commands.stopScreen(deviceId)),
  sendScreenInput: (deviceId, input) => expectOk(commands.sendScreenInput(deviceId, input)),
  requestScreenKeyFrame: (deviceId) => expectOk(commands.requestScreenKeyFrame(deviceId)),
  getCallsStatus: () => commands.getCallsStatus(),
  onCallsChanged: (listener) =>
    events.callsChanged.listen((event) => {
      listener(event.payload);
    }),
  startBluetoothPairing: (deviceId) => expectOk(commands.startBluetoothPairing(deviceId)),
  cancelBluetoothPairing: () => commands.cancelBluetoothPairing(),
  confirmBluetoothCode: (promptId, matches) =>
    expectOk(commands.confirmBluetoothCode(promptId, matches)),
  chooseBluetoothDevice: (deviceId, address) =>
    expectOk(commands.chooseBluetoothDevice(deviceId, address)),
  connectCalls: (deviceId) => expectOk(commands.connectCalls(deviceId)),
  answerCall: (deviceId, callId) => expectOk(commands.answerCall(deviceId, callId)),
  declineCall: (deviceId, callId) => expectOk(commands.declineCall(deviceId, callId)),
  hangUpCall: (deviceId, callId) => expectOk(commands.hangUpCall(deviceId, callId)),
  dialNumber: (deviceId, number) => expectOk(commands.dialNumber(deviceId, number)),
  sendCallTones: (deviceId, tones) => expectOk(commands.sendCallTones(deviceId, tones)),
  setCallMuted: (deviceId, muted) => expectOk(commands.setCallMuted(deviceId, muted)),
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
  getSettings: () => commands.getSettings(),
  setSettings: (settings) => expectOk(commands.setSettings(settings)),
};

/** The backend for the current environment. */
export const defaultApi: DesktopApi = isTauri() ? tauriApi : createPreviewApi();

/** Provide a different `DesktopApi` with `<DesktopApiContext value={...}>`. */
export const DesktopApiContext = createContext<DesktopApi>(defaultApi);

export function useDesktopApi(): DesktopApi {
  return use(DesktopApiContext);
}
