import type { ReactNode } from "react";

import { DevicesProvider } from "@/features/pairing/DevicesProvider";
import { DesktopApiContext, tauriApi, type DesktopApi } from "@/lib/api";

export interface AppProvidersProps {
  /** The backend to talk to. Tests pass a fake. */
  api?: DesktopApi;
  children: ReactNode;
}

/** App-wide context: the backend client and the shared paired-device state. */
export function AppProviders({ api = tauriApi, children }: AppProvidersProps) {
  return (
    <DesktopApiContext value={api}>
      <DevicesProvider>{children}</DevicesProvider>
    </DesktopApiContext>
  );
}
