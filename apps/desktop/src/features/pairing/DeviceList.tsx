import { Smartphone, Unlink } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { Badge, Button, Card, ConfirmDialog, type Tone } from "@/components/ui";
import type { Device, DeviceStatus } from "@/lib/api";
import { errorMessage } from "@/lib/utils";

const statusBadge: Record<DeviceStatus, { tone: Tone; label: string }> = {
  connected: { tone: "success", label: "Connected" },
  reconnecting: { tone: "warning", label: "Reconnecting…" },
  disconnected: { tone: "neutral", label: "Not connected" },
  revoked: { tone: "danger", label: "Removed on the phone" },
};

const dateOnly = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });
const dateAndTime = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

function history(device: Device): string {
  const paired = `Paired ${dateOnly.format(device.pairedAtUnixMs)}`;
  if (device.status === "connected" || device.lastConnectedUnixMs === null) return paired;
  return `${paired} · Last connected ${dateAndTime.format(device.lastConnectedUnixMs)}`;
}

export interface DeviceListProps {
  devices: Device[];
  /** Forgets a phone. Rejects with a message fit to show. */
  onUnpair: (deviceId: string) => Promise<void>;
}

/** The phones paired with this computer, each with its live status and an unpair action. */
export function DeviceList({ devices, onUnpair }: DeviceListProps) {
  const [pending, setPending] = useState<Device | null>(null);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const count = useRef(devices.length);

  // When a phone disappears its Unpair button goes with it; keep focus in the list.
  useEffect(() => {
    if (devices.length < count.current && document.activeElement === document.body) {
      headingRef.current?.focus();
    }
    count.current = devices.length;
  }, [devices.length]);

  const close = () => {
    setPending(null);
    setFailure(null);
  };

  const confirm = () => {
    if (!pending) return;
    setBusy(true);
    setFailure(null);
    onUnpair(pending.id).then(
      () => {
        setBusy(false);
        setPending(null);
      },
      (error: unknown) => {
        setBusy(false);
        setFailure(errorMessage(error));
      },
    );
  };

  return (
    <>
      <h2 ref={headingRef} tabIndex={-1} className="section-title">
        Paired phones
      </h2>
      <ul className="device-list" aria-label="Paired phones">
        {devices.map((device) => {
          const badge = statusBadge[device.status];
          return (
            <li key={device.id}>
              <Card className="device-list__item">
                <Smartphone className="device-list__icon" aria-hidden="true" />
                <div className="device-list__text">
                  <p className="device-list__name">{device.name}</p>
                  {device.model && <p className="device-list__model">{device.model}</p>}
                  <p className="device-list__model">{history(device)}</p>
                </div>
                <Badge tone={badge.tone} dot>
                  {badge.label}
                </Badge>
                <Button
                  variant="ghost"
                  size="sm"
                  icon={Unlink}
                  aria-label={`Unpair ${device.name}`}
                  onClick={() => {
                    setPending(device);
                  }}
                >
                  Unpair
                </Button>
              </Card>
            </li>
          );
        })}
      </ul>
      {pending && (
        <ConfirmDialog
          title={`Unpair ${pending.name}?`}
          confirmLabel="Unpair"
          busy={busy}
          error={failure}
          onConfirm={confirm}
          onCancel={close}
        >
          <p>
            This computer will forget {pending.name}. To use it again, you pair it again from
            scratch.
          </p>
        </ConfirmDialog>
      )}
    </>
  );
}
