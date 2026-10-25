import { Plus, RefreshCw, Smartphone, TriangleAlert } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { Button, EmptyState, PageHeader, Spinner } from "@/components/ui";

import { DeviceList } from "./DeviceList";
import { useDevices } from "./devices-context";
import { pairingMeta } from "./meta";
import { PairingSteps } from "./PairingSteps";
import "./pairing.css";

export function PairingPage() {
  const { devices, reload } = useDevices();
  const [pairing, setPairing] = useState(false);
  const startButtonRef = useRef<HTMLButtonElement>(null);
  const returnFocus = useRef(false);

  // After cancelling, put focus back on the button that started pairing.
  useEffect(() => {
    if (!pairing && returnFocus.current) {
      returnFocus.current = false;
      startButtonRef.current?.focus();
    }
  }, [pairing]);

  // TODO(core): start a real pairing session once crates/core exposes one.
  const startPairing = () => {
    setPairing(true);
  };
  const cancelPairing = () => {
    returnFocus.current = true;
    setPairing(false);
  };

  const startButton = (label: string) => (
    <Button ref={startButtonRef} variant="primary" icon={Plus} onClick={startPairing}>
      {label}
    </Button>
  );

  let body;
  if (pairing) {
    body = <PairingSteps onCancel={cancelPairing} />;
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
    body = <DeviceList devices={devices.data} />;
  }

  const hasDevices = devices.status === "ready" && devices.data.length > 0;

  return (
    <>
      <PageHeader
        title={pairingMeta.label}
        description={pairingMeta.description}
        actions={hasDevices && !pairing ? startButton("Pair another phone") : undefined}
      />
      <div className="page-body">{body}</div>
    </>
  );
}
