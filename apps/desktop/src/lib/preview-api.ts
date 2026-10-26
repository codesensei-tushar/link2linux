import { demoTransfers } from "@/features/files/demo";
import { demoNotifications } from "@/features/notifications/demo";

import type {
  ClipboardStatus,
  DesktopApi,
  Device,
  PairingState,
  PhoneTarget,
  Settings,
  Unsubscribe,
} from "./api";

const DAY_MS = 86_400_000;

/**
 * Stand-in for running the UI in a plain browser (`pnpm dev` without Tauri),
 * where IPC does not exist. It plays a short, fixed story so every screen can
 * be looked at. `?preview=qr|code|success|failed|empty|startup-error` opens on that screen,
 * and `?preview=clipboard-focus` shows a clipboard shared only while the window has focus.
 */
export function createPreviewApi(search: string = globalThis.location.search): DesktopApi {
  const scenario = new URLSearchParams(search).get("preview");
  const now = Date.now();
  let devices: Device[] =
    scenario === "empty"
      ? []
      : [
          {
            id: "preview-1",
            name: "Pixel 9",
            model: "Pixel 9",
            status: "connected",
            pairedAtUnixMs: now - 3 * DAY_MS,
            lastConnectedUnixMs: now,
          },
          {
            id: "preview-2",
            name: "Old Galaxy",
            model: "Galaxy S21",
            status: "disconnected",
            pairedAtUnixMs: now - 40 * DAY_MS,
            lastConnectedUnixMs: now - 2 * DAY_MS,
          },
        ];
  let settings: Settings = {
    showNotificationText: false,
    shareClipboard: true,
    sharePasswords: false,
  };
  const focusOnly = scenario === "clipboard-focus";
  let clipboard: ClipboardStatus = {
    mode: focusOnly ? "whenFocused" : "automatic",
    phones: devices.map((device) => ({
      deviceId: device.id,
      name: device.name,
      state: device.status === "connected" ? "ready" : "disconnected",
    })),
    activity:
      devices.length === 0
        ? []
        : [
            {
              deviceId: "preview-1",
              phoneName: "Pixel 9",
              direction: "received",
              bytes: 42,
              sensitive: false,
              atUnixMs: now - 4 * 60_000,
            },
            {
              deviceId: "preview-1",
              phoneName: "Pixel 9",
              direction: "sent",
              bytes: 1_204,
              sensitive: false,
              atUnixMs: now - 26 * 60_000,
            },
          ],
    problem: null,
    held:
      focusOnly && devices.length > 0
        ? { deviceId: "preview-1", phoneName: "Pixel 9", bytes: 42, sensitive: false }
        : null,
  };
  const pairingListeners = new Set<(state: PairingState) => void>();
  const deviceListeners = new Set<(devices: Device[]) => void>();
  const clipboardListeners = new Set<(status: ClipboardStatus) => void>();

  const qr: PairingState = {
    step: "qrOffer",
    svg: previewSvg(),
    offerNumber: 1,
    desktopName: "This computer",
    expiresInMs: 272_000,
    lifetimeMs: 300_000,
    lastFailure: null,
  };
  const phones = [
    { id: "a", name: "Pixel 9", address: "192.168.1.42:27380" },
    { id: "b", name: "Galaxy S24", address: "192.168.1.57:27380" },
  ];
  const code: PairingState = {
    step: "sasCode",
    code: "482913",
    phoneName: "Pixel 9",
    expiresInMs: 52_000,
    lifetimeMs: 60_000,
    confirmedHere: false,
  };
  const paired: PairingState = {
    step: "paired",
    device: {
      id: "preview-new",
      name: "Pixel 9",
      model: "Pixel 9",
      status: "connected",
      pairedAtUnixMs: now,
      lastConnectedUnixMs: now,
    },
  };
  const starts: Record<string, PairingState> = {
    qr,
    code,
    success: paired,
    failed: { step: "failed", method: "qrCode", reason: "expired" },
  };
  let pairing: PairingState = starts[scenario ?? ""] ?? { step: "idle" };

  const setPairing = (state: PairingState) => {
    pairing = state;
    pairingListeners.forEach((listener) => {
      listener(state);
    });
  };
  const setDevices = (next: Device[]) => {
    devices = next;
    deviceListeners.forEach((listener) => {
      listener(next);
    });
  };
  const setClipboard = (next: ClipboardStatus) => {
    clipboard = next;
    clipboardListeners.forEach((listener) => {
      listener(next);
    });
  };
  const listen = <T>(set: Set<(value: T) => void>, listener: (value: T) => void) => {
    set.add(listener);
    const stop: Unsubscribe = () => set.delete(listener);
    return Promise.resolve(stop);
  };

  return {
    appInfo: () =>
      Promise.resolve({
        name: "link2linux",
        version: "browser preview",
        identifier: "dev.link2linux.desktop",
        os: "browser",
        arch: "",
      }),
    startupError: () =>
      Promise.resolve(
        scenario === "startup-error"
          ? {
              message: "link2linux couldn't open your keyring.",
              hint: "Your keyring is locked or not running. Unlock it, then try again.",
              detail: "identity: the keyring that holds the private key is unavailable",
            }
          : null,
      ),
    restartApp: () => Promise.resolve(),
    listDevices: () => Promise.resolve(devices),
    listNotifications: () => Promise.resolve(demoNotifications(Date.now())),
    subscribeNotifications: () => () => undefined,
    dismissNotification: () => Promise.resolve(),
    runNotificationAction: () => Promise.resolve(),
    notificationImage: () => Promise.resolve(null),
    getClipboardStatus: () => Promise.resolve(clipboard),
    onClipboardChanged: (listener) => listen(clipboardListeners, listener),
    sendClipboard: () => {
      const ready = clipboard.phones.filter((phone) => phone.state === "ready");
      const at = Date.now();
      setClipboard({
        ...clipboard,
        activity: [
          ...ready.map((phone) => ({
            deviceId: phone.deviceId,
            phoneName: phone.name,
            direction: "sent" as const,
            bytes: 18,
            sensitive: false,
            atUnixMs: at,
          })),
          ...clipboard.activity,
        ].slice(0, 20),
        problem:
          ready.length === 0
            ? { kind: "noPhone", deviceId: null, phoneName: null, bytes: null, atUnixMs: at }
            : null,
      });
      return Promise.resolve();
    },
    listTransfers: (deviceId) =>
      Promise.resolve(deviceId === "preview-1" ? demoTransfers(Date.now()) : []),
    subscribeTransfers: () => () => undefined,
    pickFilesToSend: () => Promise.reject(new Error("The file chooser needs the desktop app.")),
    sendDroppedFiles: () => Promise.resolve(),
    cancelTransfer: () => Promise.resolve(),
    forgetTransfer: () => Promise.resolve(),
    openReceivedFile: () => Promise.resolve(),
    showReceivedFile: () => Promise.resolve(),
    onFileDrag: () => Promise.resolve(() => undefined),
    onFilesDropped: () => Promise.resolve(() => undefined),
    unpairDevice: (id) => {
      setDevices(devices.filter((d) => d.id !== id));
      return Promise.resolve();
    },
    getPairingState: () => Promise.resolve(pairing),
    startPairing: (method) => {
      setPairing(
        method === "qrCode"
          ? qr
          : { step: "searching", phones, attemptsLeft: 3, lastFailure: null },
      );
      return Promise.resolve();
    },
    pairWith: (target: PhoneTarget) => {
      if (target.kind === "address" && target.address.trim() === "") {
        return Promise.reject(new Error("Enter an address like 192.168.1.20."));
      }
      setPairing(pairing.step === "qrOffer" ? paired : code);
      return Promise.resolve();
    },
    answerCode: (matches) => {
      setPairing(
        matches
          ? paired
          : { step: "searching", phones, attemptsLeft: 2, lastFailure: "verificationFailed" },
      );
      return Promise.resolve();
    },
    cancelPairing: () => {
      setPairing({ step: "idle" });
      return Promise.resolve();
    },
    onPairingChanged: (listener) => listen(pairingListeners, listener),
    onDevicesChanged: (listener) => listen(deviceListeners, listener),
    getSettings: () => Promise.resolve(settings),
    setSettings: (next) => {
      settings = next;
      return Promise.resolve();
    },
  };
}

/** A pattern that only looks like a code. The real image comes from the backend. */
function previewSvg(): string {
  const size = 25;
  const quiet = 4;
  const scale = 10;
  const finder = (x: number, y: number): boolean | null => {
    const inBox = (ox: number, oy: number) => x >= ox && x < ox + 7 && y >= oy && y < oy + 7;
    for (const [ox, oy] of [
      [0, 0],
      [size - 7, 0],
      [0, size - 7],
    ] as const) {
      if (inBox(ox, oy)) {
        const dx = Math.max(Math.abs(x - ox - 3), Math.abs(y - oy - 3));
        return dx !== 2;
      }
    }
    return null;
  };
  const dark = (x: number, y: number) => finder(x, y) ?? (x * 7 + y * 13 + x * y) % 3 === 0;

  let path = "";
  for (let y = 0; y < size; y++) {
    let x = 0;
    while (x < size) {
      if (!dark(x, y)) {
        x++;
        continue;
      }
      const start = x;
      while (x < size && dark(x, y)) x++;
      const run = x - start;
      path += `M${String(start + quiet)} ${String(y + quiet)}h${String(run)}v1h-${String(run)}z`;
    }
  }
  const modules = size + 2 * quiet;
  const px = modules * scale;
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${String(modules)} ${String(modules)}" ` +
    `width="${String(px)}" height="${String(px)}" shape-rendering="crispEdges">` +
    `<rect width="${String(modules)}" height="${String(modules)}" fill="#fff"/>` +
    `<path fill="#000" d="${path}"/></svg>`
  );
}
