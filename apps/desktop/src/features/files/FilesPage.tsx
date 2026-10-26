import { Files, RefreshCw, Smartphone, TriangleAlert, Upload } from "lucide-react";
import { useEffect, useState } from "react";
import { useLocation } from "react-router";

import { Button, Callout, EmptyState, PageHeader, Spinner, StatusDot } from "@/components/ui";
import { useDevices } from "@/features/pairing/devices-context";
import type { Device } from "@/lib/api";

import { filesMeta } from "./meta";
import { TransferCard } from "./TransferCard";
import { useTransfers } from "./use-transfers";
import "./files.css";

/** Re-renders every `interval` milliseconds, so times and speeds stay current. */
function useNow(interval: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => {
      setNow(Date.now());
    }, interval);
    return () => {
      clearInterval(timer);
    };
  }, [interval]);
  return now;
}

function TransferList({ phone }: { phone: Device }) {
  const {
    state,
    rates,
    announcement,
    error,
    clearError,
    reload,
    choosing,
    send,
    cancel,
    forget,
    open,
    show,
  } = useTransfers(phone.id);
  const running =
    state.status === "ready" && state.transfers.some((t) => t.status.kind === "running");
  // Every second while bytes move, so a speed goes when they stop.
  const now = useNow(running ? 1_000 : 60_000);

  let body;
  if (state.status === "loading") {
    body = <Spinner label="Loading transfers…" />;
  } else if (state.status === "error") {
    body = (
      <EmptyState
        icon={TriangleAlert}
        title="Couldn't load transfers"
        description={state.error}
        actions={
          <Button icon={RefreshCw} onClick={reload}>
            Try again
          </Button>
        }
      />
    );
  } else if (state.transfers.length === 0) {
    body = (
      <EmptyState
        icon={Files}
        title="No transfers yet"
        description={`Files you send to ${phone.name}, and files it sends you, show up here. Received files go to your Downloads folder.`}
      />
    );
  } else {
    body = (
      <ul className="transfer-list" aria-label={`Transfers with ${phone.name}`}>
        {[...state.transfers].reverse().map((transfer) => (
          <li key={transfer.id}>
            <TransferCard
              transfer={transfer}
              phoneName={phone.name}
              rate={rates[transfer.id]}
              now={now}
              onCancel={cancel}
              onForget={forget}
              onOpen={open}
              onShow={show}
            />
          </li>
        ))}
      </ul>
    );
  }

  return (
    <>
      <div role="status" aria-live="polite" className="sr-only">
        {announcement}
      </div>
      <div className="files-toolbar">
        <Button variant="primary" icon={Upload} disabled={choosing} onClick={send}>
          Send files to {phone.name}
        </Button>
        <p className="files-toolbar__hint">
          {phone.status === "connected"
            ? "Or drop files anywhere in this window."
            : `${phone.name} isn't connected: files wait up to an hour for it.`}
        </p>
      </div>
      {error && (
        <Callout tone="warning" role="alert" className="files-toast">
          <span>{error}</span>
          <Button size="sm" variant="ghost" onClick={clearError}>
            Close
          </Button>
        </Callout>
      )}
      {body}
    </>
  );
}

/** A choice between paired phones, shown when there are several. */
function PhonePicker({
  phones,
  selected,
  onSelect,
}: {
  phones: Device[];
  selected: string;
  onSelect: (id: string) => void;
}) {
  return (
    <fieldset className="phone-picker">
      <legend className="sr-only">Phone</legend>
      {phones.map((phone) => (
        <label key={phone.id} className="phone-picker__option">
          <input
            type="radio"
            name="files-phone"
            value={phone.id}
            checked={phone.id === selected}
            onChange={() => {
              onSelect(phone.id);
            }}
          />
          <StatusDot tone={phone.status === "connected" ? "success" : "neutral"} />
          {phone.name}
          <span className="sr-only">
            {phone.status === "connected" ? ", connected" : ", not connected"}
          </span>
        </label>
      ))}
    </fieldset>
  );
}

export function FilesPage() {
  const { devices } = useDevices();
  const location = useLocation();
  const requested = (location.state as { deviceId?: string } | null)?.deviceId ?? null;
  const [chosen, setChosen] = useState<string | null>(requested);
  // Files dropped for a phone open this page on that phone.
  const [visit, setVisit] = useState(location.key);
  if (visit !== location.key) {
    setVisit(location.key);
    if (requested) setChosen(requested);
  }

  const phones =
    devices.status === "ready" ? devices.data.filter((d) => d.status !== "revoked") : [];
  const phone =
    phones.find((p) => p.id === chosen) ??
    phones.find((p) => p.status === "connected") ??
    phones[0];

  return (
    <>
      <PageHeader title={filesMeta.label} description={filesMeta.description} />
      <div className="page-body">
        {devices.status === "loading" ? (
          <Spinner label="Looking for your phone…" />
        ) : phone ? (
          <>
            {phones.length > 1 && (
              <PhonePicker phones={phones} selected={phone.id} onSelect={setChosen} />
            )}
            <TransferList key={phone.id} phone={phone} />
          </>
        ) : (
          <EmptyState
            icon={Smartphone}
            title="No phone paired"
            description="Pair your phone to send files to it and receive files from it."
          />
        )}
      </div>
    </>
  );
}
