import type { CallsStatus, CallView } from "@/lib/api";

/** A call as the demo shows it: on the hands-free link, ringing. */
export function demoCall(overrides: Partial<CallView> = {}): CallView {
  return {
    id: "bt:/ag0/call1",
    number: "+49 151 2345678",
    name: "Anna Müller",
    state: "incoming",
    appName: null,
    audioHere: true,
    canAnswer: true,
    canDecline: true,
    canHangUp: false,
    canSendTones: false,
    activeSinceUnixMs: null,
    ...overrides,
  };
}

/** The calls of the browser preview's two phones: one connected over Bluetooth with a call ringing, one not paired yet. */
export function demoCallsStatus(): CallsStatus {
  return {
    desktop: "ready",
    enabled: true,
    pairing: null,
    phones: [
      {
        deviceId: "preview-1",
        phoneName: "Pixel 9",
        link: {
          state: "connected",
          device: { address: "0C:1A:2B:3C:4D:5E", name: "Pixel 9" },
          canMute: true,
          muted: false,
        },
        calls: [demoCall()],
        problem: null,
      },
      {
        deviceId: "preview-2",
        phoneName: "Old Galaxy",
        link: { state: "notPaired" },
        calls: [],
        problem: null,
      },
    ],
  };
}
