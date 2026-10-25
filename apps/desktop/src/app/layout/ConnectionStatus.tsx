import { useCallback } from "react";

import { StatusDot, type Tone } from "@/components/ui";
import { useDevices } from "@/features/pairing/devices-context";
import { useDesktopApi, type Device } from "@/lib/api";
import { useAsync, type AsyncState } from "@/lib/use-async";

function describe(devices: AsyncState<Device[]>): { tone: Tone; label: string; detail: string } {
  if (devices.status === "loading") {
    return { tone: "neutral", label: "Checking…", detail: "Looking for paired phones" };
  }
  if (devices.status === "error") {
    return { tone: "danger", label: "Unavailable", detail: "The desktop service didn't respond" };
  }
  const connected = devices.data.find((d) => d.status === "connected");
  if (connected) {
    return { tone: "success", label: "Connected", detail: connected.name };
  }
  const [first] = devices.data;
  if (first) {
    return { tone: "warning", label: "Not connected", detail: `${first.name} is out of reach` };
  }
  return { tone: "neutral", label: "No phone paired", detail: "Pair a phone to get started" };
}

/** Connection summary and app version, pinned to the bottom of the sidebar. */
export function ConnectionStatus() {
  const api = useDesktopApi();
  const { devices } = useDevices();
  const loadInfo = useCallback(() => api.appInfo(), [api]);
  const [info] = useAsync(loadInfo);
  const { tone, label, detail } = describe(devices);

  return (
    <section className="connection" aria-label="Connection">
      <div role="status" className="connection__status">
        <StatusDot tone={tone} />
        <div>
          <p className="connection__label">{label}</p>
          <p className="connection__detail">{detail}</p>
        </div>
      </div>
      {info.status === "ready" && (
        <p className="connection__version">
          {info.data.name} {info.data.version}
        </p>
      )}
    </section>
  );
}
