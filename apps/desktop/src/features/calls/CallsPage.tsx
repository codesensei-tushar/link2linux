import { Phone, RefreshCw, Smartphone, TriangleAlert } from "lucide-react";
import { useCallback, useState } from "react";

import { Button, Callout, Card, EmptyState, PageHeader, Spinner, StatusDot } from "@/components/ui";
import { useDesktopApi, type CallsStatus, type PhoneCalls } from "@/lib/api";
import { useLive } from "@/lib/use-live";
import { errorMessage } from "@/lib/utils";

import { LinkPanel, PairingCard } from "./BluetoothSetup";
import { CallCard } from "./CallCard";
import { desktopCopy, problemText } from "./copy";
import { DialPad } from "./DialPad";
import { callsMeta } from "./meta";
import "./calls.css";

/** Why this computer can't take calls, with a way to turn calls on when they are off. */
function DesktopCard({ status, onError }: { status: CallsStatus; onError: (m: string) => void }) {
  const api = useDesktopApi();
  if (status.desktop === "ready") return null;
  const copy = desktopCopy[status.desktop];
  const turnOn = () => {
    api
      .getSettings()
      .then((settings) => api.setSettings({ ...settings, useForCalls: true }))
      .catch((reason: unknown) => {
        onError(errorMessage(reason));
      });
  };
  return (
    <Card className="calls-setup">
      <h2 className="calls-setup__title">{copy.title}</h2>
      <p className="calls-setup__text">{copy.body}</p>
      {status.desktop === "disabled" && (
        <div className="calls-setup__actions">
          <Button variant="primary" icon={Phone} onClick={turnOn}>
            Turn on calls
          </Button>
        </div>
      )}
    </Card>
  );
}

/** A choice between paired phones, shown when there are several. */
function PhonePicker({
  phones,
  selected,
  onSelect,
}: {
  phones: PhoneCalls[];
  selected: string;
  onSelect: (id: string) => void;
}) {
  return (
    <fieldset className="calls-picker">
      <legend className="sr-only">Phone</legend>
      {phones.map((phone) => (
        <label key={phone.deviceId} className="calls-picker__option">
          <input
            type="radio"
            name="calls-phone"
            value={phone.deviceId}
            checked={phone.deviceId === selected}
            onChange={() => {
              onSelect(phone.deviceId);
            }}
          />
          <StatusDot tone={phone.calls.length > 0 ? "accent" : "neutral"} />
          {phone.phoneName}
          {phone.calls.length > 0 && <span className="sr-only">, has a call</span>}
        </label>
      ))}
    </fieldset>
  );
}

function CallsView({ status }: { status: CallsStatus }) {
  const [chosen, setChosen] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const phones = status.phones;
  // Unless the user picked one, the phone with a call comes first, then the
  // one being paired.
  const phone =
    phones.find((p) => p.deviceId === chosen) ??
    phones.find((p) => p.calls.length > 0) ??
    phones.find((p) => p.deviceId === status.pairing?.deviceId) ??
    phones[0];

  if (!phone) {
    return (
      <>
        <DesktopCard status={status} onError={setError} />
        <EmptyState
          icon={Smartphone}
          title="No phone paired"
          description="Pair your phone with link2linux to take its calls on this computer."
        />
      </>
    );
  }

  const link = phone.link;
  const mute =
    link.state === "connected"
      ? { can: link.canMute, muted: link.muted }
      : { can: false, muted: false };
  const pairing = status.pairing?.deviceId === phone.deviceId ? status.pairing : null;

  return (
    <>
      <DesktopCard status={status} onError={setError} />
      {phones.length > 1 && (
        <PhonePicker phones={phones} selected={phone.deviceId} onSelect={setChosen} />
      )}
      {phone.problem && (
        <Callout tone="warning">{problemText(phone.problem, phone.phoneName)}</Callout>
      )}
      {error && (
        <Callout tone="warning" role="alert">
          {error}
        </Callout>
      )}
      {pairing ? (
        <PairingCard pairing={pairing} phoneName={phone.phoneName} onError={setError} />
      ) : (
        <LinkPanel key={phone.deviceId} phone={phone} onError={setError} />
      )}
      {phone.calls.map((call) => (
        <CallCard
          key={call.id}
          deviceId={phone.deviceId}
          call={call}
          mute={mute}
          onError={setError}
        />
      ))}
      {link.state === "connected" && (
        <DialPad key={phone.deviceId} deviceId={phone.deviceId} onError={setError} />
      )}
    </>
  );
}

/** The phones' calls: answer, decline, hang up and dial on this computer, over Bluetooth. */
export function CallsPage() {
  const api = useDesktopApi();
  const subscribe = useCallback(
    (listener: (status: CallsStatus) => void) => api.onCallsChanged(listener),
    [api],
  );
  const load = useCallback(() => api.getCallsStatus(), [api]);
  const [status, reload] = useLive(subscribe, load);

  let body;
  if (status.status === "loading") {
    body = <Spinner label="Loading calls…" />;
  } else if (status.status === "error") {
    body = (
      <EmptyState
        icon={TriangleAlert}
        title="Couldn't load calls"
        description={status.error}
        actions={
          <Button icon={RefreshCw} onClick={reload}>
            Try again
          </Button>
        }
      />
    );
  } else {
    body = <CallsView status={status.data} />;
  }

  return (
    <>
      <PageHeader title={callsMeta.label} description={callsMeta.description} />
      <div className="page-body calls">{body}</div>
    </>
  );
}
