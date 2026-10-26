import { createContext, use } from "react";

import type { PairingMethod, PairingState, PhoneTarget } from "@/lib/api";
import type { AsyncState } from "@/lib/use-async";

export interface PairingValue {
  /** The backend's pairing step, followed live. */
  state: AsyncState<PairingState>;
  /** Why the last request was refused. Cleared as soon as the step changes. */
  error: string | null;
  start: (method: PairingMethod) => Promise<void>;
  pairWith: (target: PhoneTarget) => Promise<void>;
  answerCode: (matches: boolean) => Promise<void>;
  /** Stops pairing, or dismisses a finished attempt. */
  cancel: () => Promise<void>;
  /** Stops the current attempt and starts again with another method. */
  switchTo: (method: PairingMethod) => Promise<void>;
}

export const PairingContext = createContext<PairingValue | null>(null);

export function usePairing(): PairingValue {
  const value = use(PairingContext);
  if (!value) {
    throw new Error("usePairing() must be called inside <PairingProvider>");
  }
  return value;
}
