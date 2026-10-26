import type { PairingState } from "@/lib/api";

type Step<T extends PairingState["step"]> = Extract<PairingState, { step: T }>;

const svg =
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"><rect width="1" height="1"/></svg>';

export function qrOffer(overrides: Partial<Step<"qrOffer">> = {}): Step<"qrOffer"> {
  return {
    step: "qrOffer",
    uri: "link2linux://pair?v=1&n=Desk",
    svg,
    desktopName: "Desk",
    expiresInMs: 272_000,
    lifetimeMs: 300_000,
    lastFailure: null,
    ...overrides,
  };
}

export function searching(overrides: Partial<Step<"searching">> = {}): Step<"searching"> {
  return { step: "searching", phones: [], attemptsLeft: 3, lastFailure: null, ...overrides };
}

export function sasCode(overrides: Partial<Step<"sasCode">> = {}): Step<"sasCode"> {
  return {
    step: "sasCode",
    code: "482913",
    phoneName: "Pixel 9",
    expiresInMs: 52_000,
    lifetimeMs: 60_000,
    confirmedHere: false,
    ...overrides,
  };
}
