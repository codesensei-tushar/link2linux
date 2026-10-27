import { useRef, useState } from "react";

import { errorMessage } from "@/lib/utils";

export type OnError = (message: string | null) => void;

/**
 * Runs a command and reports its failure instead of throwing. While one is in
 * flight `busy` is true and further runs are ignored, so a double click can't
 * send the command twice.
 */
export function useRun(onError: OnError) {
  const [busy, setBusy] = useState(false);
  const running = useRef(false);
  const run = (action: () => Promise<void>) => {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    onError(null);
    action()
      .catch((reason: unknown) => {
        onError(errorMessage(reason));
      })
      .finally(() => {
        running.current = false;
        setBusy(false);
      });
  };
  return { run, busy };
}
