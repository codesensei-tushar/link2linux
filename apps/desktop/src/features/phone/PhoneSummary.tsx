import { BellOff, BellRing } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui";
import { useDesktopApi, type Device, type PhoneStatus } from "@/lib/api";
import { errorMessage } from "@/lib/utils";

import { ringEndText, ringProblemText, statusItems, statusNote } from "./format";
import { usePhoneStatus, useRingStatus } from "./use-phone";
import "./phone.css";

/** The phone's battery, signal and sound mode, or why they aren't shown. */
function StatusList({ status }: { status: PhoneStatus }) {
  if (status.state !== "current") {
    const note = statusNote(status);
    return note && <p className="phone-status__note">{note}</p>;
  }
  const items = statusItems(status);
  if (items.length === 0) return null;
  return (
    <ul className="phone-status" aria-label="Phone status">
      {items.map(({ key, icon: Icon, text, label }) => (
        <li key={key} aria-label={label} title={label}>
          <Icon aria-hidden="true" />
          {text && <span aria-hidden="true">{text}</span>}
        </li>
      ))}
    </ul>
  );
}

/** Makes the phone ring so the user finds it, and stops it again. */
function RingControl({ device }: { device: Device }) {
  const api = useDesktopApi();
  const ring = useRingStatus(device.id);
  const [failure, setFailure] = useState<string | null>(null);
  if (!ring) return null;

  const run = (action: () => Promise<void>) => {
    setFailure(null);
    action().catch((error: unknown) => {
      setFailure(errorMessage(error));
    });
  };
  const ringing = ring.phase === "ringing" || ring.phase === "stopping";
  const problem = failure ?? (ring.problem && ringProblemText(ring.problem));
  let note = null;
  if (ring.blocked) note = ringProblemText(ring.blocked);
  else if (ring.ended) note = ringEndText(ring.ended);

  return (
    <div className="ring">
      <p className="ring__state" aria-live="polite">
        {ringing ? `${device.name} is ringing` : ""}
      </p>
      {ringing ? (
        <Button
          size="sm"
          icon={BellOff}
          disabled={ring.phase === "stopping"}
          onClick={() => {
            run(() => api.stopRinging(device.id));
          }}
        >
          Stop ringing
        </Button>
      ) : (
        <Button
          size="sm"
          icon={BellRing}
          disabled={ring.phase === "starting" || ring.blocked !== null}
          onClick={() => {
            run(() => api.ringPhone(device.id));
          }}
        >
          Ring phone
        </Button>
      )}
      {problem ? (
        <p role="alert" className="ring__problem">
          {problem}
        </p>
      ) : (
        note && <p className="ring__note">{note}</p>
      )}
    </div>
  );
}

/** The connected phone's status and a button that makes it ring, for the sidebar. */
export function PhoneSummary({ device }: { device: Device }) {
  const status = usePhoneStatus(device.id);
  return (
    <div className="phone-summary">
      {status && <StatusList status={status} />}
      <RingControl device={device} />
    </div>
  );
}
