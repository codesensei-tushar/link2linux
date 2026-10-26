import { CircleCheck, TriangleAlert } from "lucide-react";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";

import { Button, Card, Spinner } from "@/components/ui";
import type { PairingMethod, PairingState } from "@/lib/api";
import { cn } from "@/lib/utils";

import { AddressForm } from "./AddressForm";
import { Countdown } from "./Countdown";
import { failureCopy } from "./failure-copy";
import { usePairing } from "./pairing-context";

type Step = Exclude<PairingState, { step: "idle" }>;

/** The screen for every pairing step except idle. */
export function PairingFlow({ step }: { step: Step }) {
  switch (step.step) {
    case "qrOffer":
      return <QrPanel step={step} />;
    case "searching":
      return <SearchPanel step={step} />;
    case "connecting":
    case "verifying":
      return <ProgressPanel step={step} />;
    case "sasCode":
      return <CodePanel step={step} />;
    case "paired":
      return <DonePanel step={step} />;
    case "failed":
      return <FailedPanel step={step} />;
  }
}

/**
 * One card per step. Its title takes focus when it appears, because the
 * control that was focused a moment ago is gone.
 */
function Panel({
  title,
  className,
  ownsError = false,
  children,
}: {
  title: string;
  className?: string;
  /** The content shows refused requests itself (the address form does). */
  ownsError?: boolean;
  children: ReactNode;
}) {
  const titleId = useId();
  const titleRef = useRef<HTMLHeadingElement>(null);
  const { error } = usePairing();

  useEffect(() => {
    titleRef.current?.focus();
  }, []);

  return (
    <Card className={cn("pairing-panel", className)}>
      <section aria-labelledby={titleId}>
        <h2 id={titleId} ref={titleRef} tabIndex={-1} className="pairing-panel__title">
          {title}
        </h2>
        {children}
        {error && !ownsError && (
          <p role="alert" className="pairing-panel__error">
            {error}
          </p>
        )}
      </section>
    </Card>
  );
}

function Actions({ children }: { children: ReactNode }) {
  return <div className="pairing-panel__actions">{children}</div>;
}

function CancelButton({ label = "Cancel" }: { label?: string }) {
  const { cancel } = usePairing();
  return (
    <Button
      onClick={() => {
        void cancel();
      }}
    >
      {label}
    </Button>
  );
}

function otherMethod(method: PairingMethod): { method: PairingMethod; label: string } {
  return method === "qrCode"
    ? { method: "sas", label: "Pair with a code instead" }
    : { method: "qrCode", label: "Scan a QR code instead" };
}

function QrPanel({ step }: { step: Extract<Step, { step: "qrOffer" }> }) {
  const { switchTo } = usePairing();
  const image = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(step.svg)}`;
  const failure = step.lastFailure ? failureCopy[step.lastFailure] : null;
  // Announce a renewed code, but not the first one (the title covers that).
  const [firstUri] = useState(step.uri);
  const renewed = firstUri !== step.uri;

  return (
    <Panel title="Scan with your phone" className="pairing-panel--qr" ownsError>
      <div className="qr-layout">
        <div className="qr-layout__code">
          <img
            className="qr-image"
            src={image}
            alt={`QR code for pairing with ${step.desktopName}`}
          />
          <Countdown
            key={step.uri}
            expiresInMs={step.expiresInMs}
            lifetimeMs={step.lifetimeMs}
            label="New code in"
          />
          <p role="status" className="sr-only">
            {renewed ? "A new code is showing." : ""}
          </p>
        </div>
        <div className="qr-layout__help">
          <ol className="pairing-panel__list">
            <li>Connect the phone to the same network as this computer.</li>
            <li>Open link2linux on the phone and choose to add this computer.</li>
            <li>Point the phone&apos;s camera at the code.</li>
          </ol>
          <p className="pairing-panel__note">
            The phone will see this computer as <strong>{step.desktopName}</strong>.
          </p>
          {failure && (
            <div className="pairing-panel__problem" role="alert">
              <TriangleAlert aria-hidden="true" />
              <p>
                <strong>{failure.title}.</strong> {failure.body}
              </p>
            </div>
          )}
          <AddressForm />
        </div>
      </div>
      <Actions>
        <Button
          variant="ghost"
          onClick={() => {
            void switchTo("sas");
          }}
        >
          Can&apos;t scan? Pair with a code
        </Button>
        <CancelButton />
      </Actions>
    </Panel>
  );
}

function SearchPanel({ step }: { step: Extract<Step, { step: "searching" }> }) {
  const { pairWith, switchTo } = usePairing();
  const failure = step.lastFailure ? failureCopy[step.lastFailure] : null;

  return (
    <Panel title="Choose your phone" ownsError>
      <ol className="pairing-panel__list">
        <li>Connect the phone to the same network as this computer.</li>
        <li>Open link2linux on the phone and choose to pair with a code.</li>
        <li>Pick the phone below, then compare the codes.</li>
      </ol>

      {failure && (
        <div className="pairing-panel__problem" role="alert">
          <TriangleAlert aria-hidden="true" />
          <p>
            <strong>{failure.title}.</strong> {failure.body}
          </p>
        </div>
      )}
      {step.attemptsLeft < 3 && (
        <p className="pairing-panel__note">
          {step.attemptsLeft === 1
            ? "You have 1 try left."
            : `You have ${String(step.attemptsLeft)} tries left.`}
        </p>
      )}

      {step.phones.length === 0 ? (
        <Spinner label="Looking for phones on this network…" />
      ) : (
        <ul className="phone-list" aria-label="Phones ready to pair">
          {step.phones.map((phone) => (
            <li key={phone.id}>
              <button
                type="button"
                className="phone-list__item"
                onClick={() => {
                  void pairWith({ kind: "discovered", id: phone.id });
                }}
              >
                <span className="phone-list__name">{phone.name}</span>
                <span className="phone-list__address">{phone.address}</span>
              </button>
            </li>
          ))}
        </ul>
      )}

      <AddressForm />
      <Actions>
        <Button
          variant="ghost"
          onClick={() => {
            void switchTo("qrCode");
          }}
        >
          Scan a QR code instead
        </Button>
        <CancelButton />
      </Actions>
    </Panel>
  );
}

function ProgressPanel({ step }: { step: Extract<Step, { step: "connecting" | "verifying" }> }) {
  const who = step.phoneName ?? "your phone";
  const connecting = step.step === "connecting";
  return (
    <Panel title={connecting ? `Connecting to ${who}` : "Checking the connection"}>
      <Spinner
        label={
          connecting
            ? `Opening a secure connection to ${who}…`
            : `Making sure ${who} is the phone you meant to pair…`
        }
      />
      <Actions>
        <CancelButton />
      </Actions>
    </Panel>
  );
}

function groupCode(code: string): string {
  return code.length === 6 ? `${code.slice(0, 3)} ${code.slice(3)}` : code;
}

function CodePanel({ step }: { step: Extract<Step, { step: "sasCode" }> }) {
  const { answerCode } = usePairing();
  const spoken = step.code.split("").join(" ");

  return (
    <Panel title={`Compare the codes with ${step.phoneName}`}>
      <p className="pairing-panel__note">
        Your phone should show the same six digits. Only confirm if they are identical.
      </p>
      <p className="sas-code" aria-label={`Code: ${spoken}`}>
        <span aria-hidden="true">{groupCode(step.code)}</span>
      </p>
      <Countdown
        key={`${step.code}-${String(step.expiresInMs)}`}
        expiresInMs={step.expiresInMs}
        lifetimeMs={step.lifetimeMs}
        label="Code valid for"
      />
      {step.confirmedHere ? (
        <Spinner label={`Waiting for ${step.phoneName} to confirm…`} />
      ) : (
        <Actions>
          <Button
            onClick={() => {
              void answerCode(false);
            }}
          >
            Codes don&apos;t match
          </Button>
          <Button
            variant="primary"
            onClick={() => {
              void answerCode(true);
            }}
          >
            Codes match
          </Button>
        </Actions>
      )}
    </Panel>
  );
}

function DonePanel({ step }: { step: Extract<Step, { step: "paired" }> }) {
  const { cancel } = usePairing();
  return (
    <Panel title={`Paired with ${step.device.name}`} className="pairing-panel--done">
      <p className="pairing-panel__done">
        <CircleCheck aria-hidden="true" />
        <span>This computer and {step.device.name} now trust each other.</span>
      </p>
      <Actions>
        <Button
          variant="primary"
          onClick={() => {
            void cancel();
          }}
        >
          Done
        </Button>
      </Actions>
    </Panel>
  );
}

function FailedPanel({ step }: { step: Extract<Step, { step: "failed" }> }) {
  const { cancel, switchTo } = usePairing();
  const copy = failureCopy[step.reason];
  const other = otherMethod(step.method);
  const label = step.reason === "tooManyAttempts" ? "Start again" : "Try again";

  return (
    <Panel title={copy.title} className="pairing-panel--failed">
      <p className="pairing-panel__problem" role="alert">
        <TriangleAlert aria-hidden="true" />
        <span>{copy.body}</span>
      </p>
      <Actions>
        <Button
          variant="ghost"
          onClick={() => {
            void switchTo(other.method);
          }}
        >
          {other.label}
        </Button>
        <Button
          onClick={() => {
            void cancel();
          }}
        >
          Close
        </Button>
        <Button
          variant="primary"
          onClick={() => {
            void switchTo(step.method);
          }}
        >
          {label}
        </Button>
      </Actions>
    </Panel>
  );
}
