import { createContext, use, useCallback, useEffect, useState } from "react";

import { useDesktopApi, type ScreenStatus } from "@/lib/api";
import { useLive } from "@/lib/use-live";

import { h264Supported, type DecoderEnv } from "./decoder";

/** Follows a phone's screen, or null until it is known. */
export function useScreenStatus(deviceId: string): ScreenStatus | null {
  const api = useDesktopApi();
  const subscribe = useCallback(
    (listener: (screen: ScreenStatus) => void) => api.onScreenChanged(deviceId, listener),
    [api, deviceId],
  );
  const load = useCallback(() => api.getScreenStatus(deviceId), [api, deviceId]);
  const [state] = useLive(subscribe, load);
  return state.status === "ready" ? state.data : null;
}

/** What the screen's decoder uses of the browser: tests provide a stand-in. */
export const DecoderEnvContext = createContext<DecoderEnv | undefined>(undefined);

/** The browser parts the decoder uses, or undefined for the real ones. */
export function useDecoderEnv(): DecoderEnv | undefined {
  return use(DecoderEnvContext);
}

/** Whether the webview decodes H.264, or null while it is being asked. */
export function useH264Support(): boolean | null {
  const env = useDecoderEnv();
  const [supported, setSupported] = useState<boolean | null>(null);
  useEffect(() => {
    let active = true;
    void h264Supported(env).then((result) => {
      if (active) setSupported(result);
    });
    return () => {
      active = false;
    };
  }, [env]);
  return supported;
}
