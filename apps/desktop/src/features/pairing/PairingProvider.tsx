import { useCallback, useMemo, useState, type ReactNode } from "react";

import { useDesktopApi, type PairingState } from "@/lib/api";
import { useLive } from "@/lib/use-live";
import { errorMessage } from "@/lib/utils";

import { PairingContext, type PairingValue } from "./pairing-context";

interface Refusal {
  message: string;
  /** The state that was showing when the request was refused. */
  during: PairingState | null;
}

/**
 * Follows the backend's pairing step for the whole app, so leaving the page
 * and coming back finds the same screen. Requests return nothing; the result
 * is the next event.
 */
export function PairingProvider({ children }: { children: ReactNode }) {
  const api = useDesktopApi();
  const subscribe = useCallback(
    (listener: (state: PairingState) => void) => api.onPairingChanged(listener),
    [api],
  );
  const load = useCallback(() => api.getPairingState(), [api]);
  const [state] = useLive(subscribe, load);
  const [refusal, setRefusal] = useState<Refusal | null>(null);

  const current = state.status === "ready" ? state.data : null;
  // A refusal belongs to the step it happened in; any new step clears it.
  const error = refusal?.during === current ? refusal.message : null;

  const run = useCallback(
    async (request: () => Promise<void>) => {
      setRefusal(null);
      try {
        await request();
      } catch (failure) {
        setRefusal({ message: errorMessage(failure), during: current });
      }
    },
    [current],
  );

  const value = useMemo<PairingValue>(
    () => ({
      state,
      error,
      start: (method) => run(() => api.startPairing(method)),
      pairWith: (target) => run(() => api.pairWith(target)),
      answerCode: (matches) => run(() => api.answerCode(matches)),
      cancel: () => run(() => api.cancelPairing()),
      switchTo: (method) =>
        run(async () => {
          await api.cancelPairing();
          await api.startPairing(method);
        }),
    }),
    [api, error, run, state],
  );

  return <PairingContext value={value}>{children}</PairingContext>;
}
