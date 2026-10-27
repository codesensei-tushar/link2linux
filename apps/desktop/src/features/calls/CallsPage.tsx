import { Phone, RefreshCw, Smartphone, TriangleAlert } from "lucide-react";
import { useCallback, useState } from "react";

import { Button, Callout, Card, EmptyState, PageHeader, Spinner, StatusDot } from "@/components/ui";
import { useDesktopApi, type CallsStatus, type CallView, type PhoneCalls } from "@/lib/api";
import { useLive } from "@/lib/use-live";
import { errorMessage } from "@/lib/utils";

import { LinkPanel, PairingCard } from "./BluetoothSetup";
import { CallCard, type Mute } from "./CallCard";
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

/** What the microphone can do in a phone's calls. */
function muteOf(phone: PhoneCalls): Mute {
  return phone.link.state === "connected"
    ? { can: phone.link.canMute, muted: phone.link.muted }
    : { can: false, muted: false };
}

const isRinging = (call: CallView) => call.state === "incoming" || call.state === "waiting";

function CallsView({ status }: { status: CallsStatus }) {
  const [chosen, setChosen] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const phones = status.phones;
  // Unless the user picked one, the phone being paired comes first. Calls show
  // for every phone, so one arriving must not move the selection.
  const phone =
    phones.find((p) => p.deviceId === chosen) ??
    phones.find((p) => p.deviceId === status.pairing?.deviceId) ??
    phones[0];

  // Drop an error when the user picks another phone. It is not tied to the
  // selected phone, since call cards of every phone report errors here.
  const [errorChosen, setErrorChosen] = useState(chosen);
  if (errorChosen !== chosen) {
    setErrorChosen(chosen);
    setError(null);
  }

  // Cards keep the place they were first shown at, so answering a call doesn't
  // move it. A call seen for the first time goes on top when it is ringing and
  // to the end otherwise.
  const [ranks, setRanks] = useState({ byKey: new Map<string, number>(), lo: 0, hi: 0 });
  const entries = phones.flatMap((p) =>
    p.calls.map((call) => ({ key: `${p.deviceId}:${call.id}`, phone: p, call })),
  );
  const fresh = entries.filter((e) => !ranks.byKey.has(e.key));
  if (fresh.length > 0) {
    const byKey = new Map(ranks.byKey);
    const ringing = fresh.filter((e) => isRinging(e.call));
    const rest = fresh.filter((e) => !isRinging(e.call));
    ringing.forEach((e, i) => byKey.set(e.key, ranks.lo - ringing.length + i));
    rest.forEach((e, i) => byKey.set(e.key, ranks.hi + 1 + i));
    setRanks({ byKey, lo: ranks.lo - ringing.length, hi: ranks.hi + rest.length });
  }

  const errorCallout = error && (
    <Callout tone="warning" role="alert">
      {error}
    </Callout>
  );

  if (!phone) {
    return (
      <>
        <DesktopCard status={status} onError={setError} />
        {errorCallout}
        <EmptyState
          icon={Smartphone}
          title="No phone paired"
          description="Pair your phone with link2linux to take its calls on this computer."
        />
      </>
    );
  }

  const pairing = status.pairing?.deviceId === phone.deviceId ? status.pairing : null;
  // Every phone's calls stay on top of the page, so a call can be answered
  // whichever phone is selected.
  const named = phones.length > 1;
  const rank = (key: string) => ranks.byKey.get(key) ?? Infinity;
  const calls = [...entries].sort((a, b) => rank(a.key) - rank(b.key));

  return (
    <>
      <div className="sr-only" aria-live="polite">
        {calls
          .filter(({ call }) => isRinging(call))
          .map(({ key, phone: p, call }) => (
            <p key={key}>
              {`Incoming call from ${call.name || call.number || "an unknown caller"}${named ? ` on ${p.phoneName}` : ""}`}
            </p>
          ))}
      </div>
      {calls.map(({ key, phone: p, call }) => (
        <CallCard
          key={key}
          deviceId={p.deviceId}
          phoneName={named ? p.phoneName : undefined}
          call={call}
          mute={muteOf(p)}
          onError={setError}
        />
      ))}
      <DesktopCard status={status} onError={setError} />
      {named && <PhonePicker phones={phones} selected={phone.deviceId} onSelect={setChosen} />}
      {phone.problem && (
        <Callout tone="warning">{problemText(phone.problem, phone.phoneName)}</Callout>
      )}
      {errorCallout}
      {pairing ? (
        <PairingCard pairing={pairing} phoneName={phone.phoneName} onError={setError} />
      ) : (
        <LinkPanel key={phone.deviceId} phone={phone} onError={setError} />
      )}
      {phone.link.state === "connected" && (
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
