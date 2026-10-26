import { useCallback, useEffect, useState } from "react";

import type { Unsubscribe } from "./api";
import type { AsyncState } from "./use-async";
import { errorMessage } from "./utils";

/**
 * Follows a value the backend pushes as events and can also report on demand.
 *
 * It subscribes first and then reads a snapshot, and applies the snapshot only
 * if no event arrived meanwhile, so a late reply never overwrites newer data.
 * `subscribe` and `load` must keep the same identity between renders.
 */
export function useLive<T>(
  subscribe: (listener: (value: T) => void) => Promise<Unsubscribe>,
  load: () => Promise<T>,
): [AsyncState<T>, () => void] {
  const [state, setState] = useState<AsyncState<T>>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let active = true;
    let sawEvent = false;
    let unsubscribe: Unsubscribe | undefined;

    subscribe((value) => {
      if (!active) return;
      sawEvent = true;
      setState({ status: "ready", data: value });
    })
      .then((stop) => {
        if (!active) {
          stop();
          return undefined;
        }
        unsubscribe = stop;
        return load().then((data) => {
          if (active && !sawEvent) setState({ status: "ready", data });
        });
      })
      .catch((error: unknown) => {
        if (active && !sawEvent) setState({ status: "error", error: errorMessage(error) });
      });

    return () => {
      active = false;
      unsubscribe?.();
    };
  }, [subscribe, load, attempt]);

  const reload = useCallback(() => {
    setState({ status: "loading" });
    setAttempt((n) => n + 1);
  }, []);

  return [state, reload];
}
