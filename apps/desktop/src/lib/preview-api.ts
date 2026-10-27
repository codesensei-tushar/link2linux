import { demoCall, demoCallsStatus } from "@/features/calls/demo";
import { demoTransfers } from "@/features/files/demo";
import { demoNotifications } from "@/features/notifications/demo";
import { demoScreenFormat, demoScreenPacket } from "@/features/screen/demo";

import type {
  CallsStatus,
  CallView,
  ClipboardStatus,
  DesktopApi,
  Device,
  PairingState,
  PhoneStatus,
  PhoneTarget,
  RingStatus,
  ScreenStatus,
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
  const statusListeners = new Set<(change: { deviceId: string; status: PhoneStatus }) => void>();
  const ringListeners = new Set<(change: { deviceId: string; ring: RingStatus }) => void>();
  const rings = new Map<string, RingStatus>();
  const ringTimers = new Map<string, ReturnType<typeof setTimeout>>();
  const callsListeners = new Set<(status: CallsStatus) => void>();
  let calls: CallsStatus = demoCallsStatus();

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
  const setCalls = (next: CallsStatus) => {
    calls = next;
    callsListeners.forEach((listener) => {
      listener(next);
    });
  };
  /** Changes phone `deviceId`'s calls. */
  const changeCalls = (deviceId: string, change: (calls: CallView[]) => CallView[]) => {
    setCalls({
      ...calls,
      phones: calls.phones.map((phone) =>
        phone.deviceId === deviceId ? { ...phone, calls: change(phone.calls) } : phone,
      ),
    });
    return Promise.resolve();
  };
  const listen = <T>(set: Set<(value: T) => void>, listener: (value: T) => void) => {
    set.add(listener);
    const stop: Unsubscribe = () => set.delete(listener);
    return Promise.resolve(stop);
  };
  const isConnected = (deviceId: string) =>
    devices.some((device) => device.id === deviceId && device.status === "connected");
  const statusOf = (deviceId: string): PhoneStatus => {
    if (!isConnected(deviceId)) return { state: "disconnected" };
    if (settings.showPhoneStatus === false) return { state: "hidden" };
    return {
      state: "current",
      battery: { percent: 76, pluggedIn: false, powerSaver: false },
      cellular: { level: 3, network: "5g", operatorName: "Carrier" },
      wifi: { level: 4 },
      ringer: "vibrate",
      doNotDisturb: false,
    };
  };
  const ringOf = (deviceId: string): RingStatus => {
    const blocked = isConnected(deviceId) ? null : "notConnected";
    const ring = rings.get(deviceId) ?? { phase: "idle", blocked, problem: null, ended: null };
    return { ...ring, blocked };
  };
  const setRing = (deviceId: string, ring: RingStatus) => {
    rings.set(deviceId, ring);
    ringListeners.forEach((listener) => {
      listener({ deviceId, ring });
    });
  };
  const screenListeners = new Set<(change: { deviceId: string; screen: ScreenStatus }) => void>();
  const screens = new Map<string, ScreenStatus>();
  const screenTimers = new Map<string, ReturnType<typeof setTimeout>>();
  const screenOf = (deviceId: string): ScreenStatus => {
    const blocked = isConnected(deviceId) ? null : "notConnected";
    const screen = screens.get(deviceId) ?? {
      phase: "idle",
      blocked,
      problem: null,
      ended: null,
      format: null,
      inputProblem: null,
    };
    return { ...screen, blocked };
  };
  const setScreen = (deviceId: string, screen: ScreenStatus) => {
    screens.set(deviceId, screen);
    screenListeners.forEach((listener) => {
      listener({ deviceId, screen });
    });
  };
  const stopScreen = (deviceId: string) => {
    clearTimeout(screenTimers.get(deviceId));
    screenTimers.delete(deviceId);
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
    getPhoneStatus: (deviceId) => Promise.resolve(statusOf(deviceId)),
    onPhoneStatusChanged: (deviceId, listener) =>
      listen(statusListeners, (change) => {
        if (change.deviceId === deviceId) listener(change.status);
      }),
    getRingStatus: (deviceId) => Promise.resolve(ringOf(deviceId)),
    onRingChanged: (deviceId, listener) =>
      listen(ringListeners, (change) => {
        if (change.deviceId === deviceId) listener(change.ring);
      }),
    ringPhone: (deviceId) => {
      const ring = ringOf(deviceId);
      clearTimeout(ringTimers.get(deviceId));
      if (ring.blocked) {
        setRing(deviceId, { ...ring, problem: ring.blocked, ended: null });
        return Promise.resolve();
      }
      setRing(deviceId, { ...ring, phase: "ringing", problem: null, ended: null });
      ringTimers.set(
        deviceId,
        setTimeout(() => {
          setRing(deviceId, { ...ringOf(deviceId), phase: "idle", ended: "timedOut" });
        }, 60_000),
      );
      return Promise.resolve();
    },
    stopRinging: (deviceId) => {
      clearTimeout(ringTimers.get(deviceId));
      setRing(deviceId, { ...ringOf(deviceId), phase: "idle", problem: null });
      return Promise.resolve();
    },
    getScreenStatus: (deviceId) => Promise.resolve(screenOf(deviceId)),
    onScreenChanged: (deviceId, listener) =>
      listen(screenListeners, (change) => {
        if (change.deviceId === deviceId) listener(change.screen);
      }),
    startScreen: (deviceId, _codecs, _maxSize, onPacket) => {
      stopScreen(deviceId);
      const screen = screenOf(deviceId);
      if (screen.blocked) {
        setScreen(deviceId, { ...screen, problem: screen.blocked, ended: null });
        return Promise.resolve();
      }
      // The pretend user agrees after a moment, then the screen plays.
      setScreen(deviceId, { ...screen, phase: "asking", problem: null, ended: null });
      let index = 0;
      const play = () => {
        void demoScreenPacket(index).then((packet) => {
          if (packet && screenTimers.has(deviceId)) onPacket(packet);
        });
        index += 1;
        screenTimers.set(deviceId, setTimeout(play, 100));
      };
      screenTimers.set(
        deviceId,
        setTimeout(() => {
          setScreen(deviceId, { ...screenOf(deviceId), phase: "showing", format: demoScreenFormat });
          play();
        }, 1500),
      );
      return Promise.resolve();
    },
    stopScreen: (deviceId) => {
      stopScreen(deviceId);
      setScreen(deviceId, { ...screenOf(deviceId), phase: "idle", format: null, problem: null });
      return Promise.resolve();
    },
    sendScreenInput: () => Promise.resolve(),
    requestScreenKeyFrame: () => Promise.resolve(),
    getCallsStatus: () => Promise.resolve(calls),
    onCallsChanged: (listener) => listen(callsListeners, listener),
    startBluetoothPairing: (deviceId) => {
      setCalls({
        ...calls,
        pairing: {
          deviceId,
          adapterName: "This computer",
          adapterAddress: "A0:B1:C2:D3:E4:F5",
          endsUnixMs: Date.now() + 120_000,
          phone: "asked",
          prompt: { id: 1, deviceName: "Old Galaxy", code: "482913" },
        },
      });
      return Promise.resolve();
    },
    cancelBluetoothPairing: () => {
      setCalls({ ...calls, pairing: null });
      return Promise.resolve();
    },
    confirmBluetoothCode: (_promptId, matches) => {
      const deviceId = calls.pairing?.deviceId;
      setCalls({
        ...calls,
        pairing: null,
        phones: calls.phones.map((phone) =>
          matches && phone.deviceId === deviceId
            ? {
                ...phone,
                link: {
                  state: "connected",
                  device: { address: "00:11:22:33:44:55", name: phone.phoneName },
                  canMute: true,
                  muted: false,
                },
              }
            : phone,
        ),
      });
      return Promise.resolve();
    },
    chooseBluetoothDevice: () => Promise.resolve(),
    connectCalls: () => Promise.resolve(),
    answerCall: (deviceId, callId) =>
      changeCalls(deviceId, (list) =>
        list.map((call) =>
          call.id === callId
            ? demoCall({
                ...call,
                state: "active",
                canAnswer: false,
                canDecline: false,
                canHangUp: true,
                canSendTones: true,
                activeSinceUnixMs: Date.now(),
              })
            : call,
        ),
      ),
    declineCall: (deviceId, callId) =>
      changeCalls(deviceId, (list) => list.filter((call) => call.id !== callId)),
    hangUpCall: (deviceId, callId) =>
      changeCalls(deviceId, (list) => list.filter((call) => call.id !== callId)),
    dialNumber: (deviceId, number) =>
      changeCalls(deviceId, (list) => [
        ...list,
        demoCall({
          id: `bt:/ag0/call${String(list.length + 2)}`,
          number,
          name: "",
          state: "alerting",
          canAnswer: false,
          canDecline: false,
          canHangUp: true,
        }),
      ]),
    sendCallTones: () => Promise.resolve(),
    setCallMuted: (deviceId, muted) => {
      setCalls({
        ...calls,
        phones: calls.phones.map((phone) =>
          phone.deviceId === deviceId && phone.link.state === "connected"
            ? { ...phone, link: { ...phone.link, muted } }
            : phone,
        ),
      });
      return Promise.resolve();
    },
    getSettings: () => Promise.resolve(settings),
    setSettings: (next) => {
      settings = next;
      devices.forEach((device) => {
        const status = statusOf(device.id);
        statusListeners.forEach((listener) => {
          listener({ deviceId: device.id, status });
        });
      });
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
