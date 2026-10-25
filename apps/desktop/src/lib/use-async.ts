import { useCallback, useEffect, useState } from "react";

import { errorMessage } from "./utils";

export type AsyncState<T> =
  { status: "loading" } | { status: "ready"; data: T } | { status: "error"; error: string };

/**
 * Runs `load` on mount and returns its state plus a `reload` function.
 *
 * `load` must keep the same identity between renders (wrap it in
 * `useCallback`), otherwise it runs again on every render.
 */
export function useAsync<T>(load: () => Promise<T>): [AsyncState<T>, () => void] {
  const [state, setState] = useState<AsyncState<T>>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let current = true;
    load().then(
      (data) => {
        if (current) setState({ status: "ready", data });
      },
      (error: unknown) => {
        if (current) setState({ status: "error", error: errorMessage(error) });
      },
    );
    return () => {
      current = false;
    };
  }, [load, attempt]);

  const reload = useCallback(() => {
    setState({ status: "loading" });
    setAttempt((n) => n + 1);
  }, []);

  return [state, reload];
}
