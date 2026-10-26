import { Plus, RefreshCw, Smartphone, TriangleAlert } from "lucide-react";
import { useEffect, useRef } from "react";

import { Button, EmptyState, PageHeader, Spinner } from "@/components/ui";
import { useDesktopApi } from "@/lib/api";

import { DeviceList } from "./DeviceList";
import { useDevices } from "./devices-context";
import { pairingMeta } from "./meta";
import { PairingFlow } from "./PairingFlow";
import { usePairing } from "./pairing-context";
import "./pairing.css";

export function PairingPage() {
  const api = useDesktopApi();
  const { devices, reload } = useDevices();
  const { state, start, error } = usePairing();
  const startButtonRef = useRef<HTMLButtonElement>(null);
  const wasPairing = useRef(false);
  const deviceCount = useRef(0);

  const step = state.status === "ready" ? state.data : null;
  const pairing = step !== null && step.step !== "idle";
  const count = devices.status === "ready" ? devices.data.length : 0;

  useEffect(() => {
    // After a finished or cancelled attempt, put focus back on the button that started it.
    if (wasPairing.current && !pairing) startButtonRef.current?.focus();
    wasPairing.current = pairing;
  }, [pairing]);

  useEffect(() => {
    // The last phone was unpaired: its list is gone, so move to the start button.
    if (deviceCount.current > 0 && count === 0 && !pairing) startButtonRef.current?.focus();
    deviceCount.current = count;
  }, [count, pairing]);

  const startButton = (label: string) => (
    <Button
      ref={startButtonRef}
      variant="primary"
      icon={Plus}
      onClick={() => {
        void start("qrCode");
      }}
    >
      {label}
    </Button>
  );

  let body;
  if (step && step.step !== "idle") {
    body = <PairingFlow step={step} />;
  } else if (devices.status === "loading") {
    body = <Spinner label="Looking for paired phones…" />;
  } else if (devices.status === "error") {
    body = (
      <EmptyState
        icon={TriangleAlert}
        title="Couldn't load paired phones"
        description={devices.error}
        actions={
          <Button icon={RefreshCw} onClick={reload}>
            Try again
          </Button>
        }
      />
    );
  } else if (devices.data.length === 0) {
    body = (
      <EmptyState
        icon={Smartphone}
        title="No phone paired yet"
        description="Pair your Android phone to see its notifications, messages, calls and photos on this computer. Both devices need to be on the same network."
        actions={startButton("Pair a phone")}
      />
    );
  } else {
    body = <DeviceList devices={devices.data} onUnpair={(id) => api.unpairDevice(id)} />;
  }

  return (
    <>
      <PageHeader
        title={pairingMeta.label}
        description={pairingMeta.description}
        actions={count > 0 && !pairing ? startButton("Pair another phone") : undefined}
      />
      <div className="page-body">
        {error && !pairing && (
          <p role="alert" className="page-error">
            {error}
          </p>
        )}
        {body}
      </div>
    </>
  );
}
