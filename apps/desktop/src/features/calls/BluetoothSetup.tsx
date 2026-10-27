import { Bluetooth, BluetoothConnected, Check, X } from "lucide-react";
import { useEffect, useId, useRef } from "react";

import { Badge, Button, Card, Spinner } from "@/components/ui";
import { useDesktopApi, type BluetoothPairing, type PhoneCalls } from "@/lib/api";

import { requestText } from "./copy";
import { formatDuration, groupCode } from "./format";
import { useClock } from "./use-clock";
import { useRun, type OnError } from "./use-run";

/** The code to compare with the other device's. Its digits are spoken one by one. */
function PairingPrompt({
  prompt,
  closed,
  busy,
  onConfirm,
}: {
  prompt: NonNullable<BluetoothPairing["prompt"]>;
  closed: boolean;
  busy: boolean;
  onConfirm: (matches: boolean) => void;
}) {
  const yes = useRef<HTMLButtonElement>(null);
  // The buttons use aria-disabled, so they keep keyboard focus while busy.
  const idle = !closed && !busy;
  // The prompt appears on its own, so put keyboard users on the decision.
  useEffect(() => {
    yes.current?.focus();
  }, []);
  return (
    <div className="pairing-prompt" role="alert">
      <p className="calls-setup__text">
        {`${prompt.deviceName || "A device"} wants to pair. Does it show this code?`}
      </p>
      <p
        className="pairing-prompt__code"
        role="img"
        aria-label={`Code ${prompt.code.split("").join(" ")}`}
      >
        {groupCode(prompt.code)}
      </p>
      <div className="calls-setup__actions">
        <Button
          ref={yes}
          variant="primary"
          icon={Check}
          aria-disabled={!idle}
          onClick={() => {
            if (idle) onConfirm(true);
          }}
        >
          Yes, it matches
        </Button>
        <Button
          icon={X}
          aria-disabled={!idle}
          onClick={() => {
            if (idle) onConfirm(false);
          }}
        >
          No
        </Button>
      </div>
    </div>
  );
}

/** The pairing window the user opened: what to do on the phone, and the code to confirm. */
export function PairingCard({
  pairing,
  phoneName,
  onError,
}: {
  pairing: BluetoothPairing;
  phoneName: string;
  onError: OnError;
}) {
  const api = useDesktopApi();
  const { run, busy } = useRun(onError);
  const now = useClock();
  const titleId = useId();
  const prompt = pairing.prompt;
  const remaining = pairing.endsUnixMs - now;
  // The desktop only accepts a code inside the window, so stop offering it
  // once the window is over, even before the backend says so.
  const closed = remaining <= 0;

  return (
    <Card className="calls-setup">
      <section aria-labelledby={titleId} className="calls-setup__body">
        <h2 id={titleId} className="calls-setup__title">
          {`Pair ${phoneName} over Bluetooth`}
        </h2>
        {!prompt && (
          <p className="calls-setup__text">
            {requestText(pairing.phone, phoneName, pairing.adapterName)}
          </p>
        )}
        <ol className="calls-setup__steps">
          <li>{`On ${phoneName}, open Bluetooth settings and pick “${pairing.adapterName}”.`}</li>
          <li>Check that both devices show the same code, and confirm on each.</li>
        </ol>
        {prompt ? (
          <PairingPrompt
            prompt={prompt}
            closed={closed}
            busy={busy}
            onConfirm={(matches) => {
              run(() => api.confirmBluetoothCode(prompt.id, matches));
            }}
          />
        ) : (
          <Spinner label="Waiting for a device to start pairing…" />
        )}
        <div className="calls-setup__actions">
          <span className="calls-setup__hint">
            {closed
              ? "The pairing window has closed."
              : `Pairing stays open for ${formatDuration(remaining)}.`}
          </span>
          <Button
            variant="ghost"
            onClick={() => {
              void api.cancelBluetoothPairing();
            }}
          >
            Cancel
          </Button>
        </div>
      </section>
    </Card>
  );
}

/** A phone's Bluetooth link with this computer, and the way to set it up. */
export function LinkPanel({ phone, onError }: { phone: PhoneCalls; onError: OnError }) {
  const api = useDesktopApi();
  const { run, busy } = useRun(onError);
  const titleId = useId();
  const name = phone.phoneName;
  const pair = () => {
    run(() => api.startBluetoothPairing(phone.deviceId));
  };
  const link = phone.link;

  let title;
  let body;
  switch (link.state) {
    case "off":
      return null;
    case "notPaired":
      title = `Pair ${name} over Bluetooth`;
      body = (
        <>
          <p className="calls-setup__text">
            Its calls then ring on this computer, and you talk with this computer's microphone and
            speakers. Music and videos keep playing on the phone.
          </p>
          <div className="calls-setup__actions">
            <Button variant="primary" icon={Bluetooth} disabled={busy} onClick={pair}>
              Pair over Bluetooth
            </Button>
          </div>
        </>
      );
      break;
    case "choose":
      title = `Which Bluetooth device is ${name}?`;
      body = (
        <>
          <p className="calls-setup__text">
            {link.candidates.length === 1
              ? "One device paired with this computer could be it. Pick it only if it's this phone; link2linux keeps your choice."
              : "Several devices paired with this computer could be it. Pick it once; link2linux keeps your choice."}
          </p>
          <ul className="calls-choices">
            {link.candidates.map((candidate) => (
              <li key={candidate.address}>
                <Button
                  icon={Bluetooth}
                  disabled={busy}
                  aria-label={`${candidate.name || candidate.address}, ${candidate.address}`}
                  onClick={() => {
                    run(() => api.chooseBluetoothDevice(phone.deviceId, candidate.address));
                  }}
                >
                  {candidate.name || candidate.address}
                </Button>
              </li>
            ))}
          </ul>
          <div className="calls-setup__actions">
            <Button variant="ghost" disabled={busy} onClick={pair}>
              It isn't listed: pair it
            </Button>
          </div>
        </>
      );
      break;
    case "disconnected":
      title = `${name} isn't connected for calls`;
      body = (
        <>
          <p className="calls-setup__text">
            {`It's paired with this computer over Bluetooth as “${link.device.name}”.`}
          </p>
          <div className="calls-setup__actions">
            <Button
              variant="primary"
              icon={BluetoothConnected}
              disabled={busy}
              onClick={() => {
                run(() => api.connectCalls(phone.deviceId));
              }}
            >
              Connect
            </Button>
            <Button variant="ghost" disabled={busy} onClick={pair}>
              Pair again
            </Button>
          </div>
        </>
      );
      break;
    case "connecting":
      title = `Connecting to ${name}`;
      body = <Spinner label={`Connecting to “${link.device.name}” over Bluetooth…`} />;
      break;
    case "connected":
      return (
        <div className="calls-link">
          <Badge tone="success" dot>
            Connected over Bluetooth
          </Badge>
          <span className="calls-link__device">
            {`Calls to ${name} ring here (“${link.device.name}”).`}
          </span>
        </div>
      );
  }

  return (
    <Card className="calls-setup">
      <section aria-labelledby={titleId} className="calls-setup__body">
        <h2 id={titleId} className="calls-setup__title">
          {title}
        </h2>
        {body}
      </section>
    </Card>
  );
}
