import { useCallback, type ReactNode } from "react";

import { useDesktopApi } from "@/lib/api";
import { useAsync } from "@/lib/use-async";

import { StartupErrorPage } from "./StartupErrorPage";

/** Renders the app, or the startup error page when the backend did not start. */
export function StartupGate({ children }: { children: ReactNode }) {
  const api = useDesktopApi();
  const load = useCallback(() => api.startupError(), [api]);
  const [state] = useAsync(load);

  if (state.status === "loading") return null;
  if (state.status === "ready" && state.data) return <StartupErrorPage error={state.data} />;
  // Without an answer the app is shown: its own pages report what does not work.
  return children;
}
