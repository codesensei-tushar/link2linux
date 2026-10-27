import { Bluetooth, BluetoothConnected, Check, X } from "lucide-react";
import { useId } from "react";

import { Badge, Button, Card, Spinner } from "@/components/ui";
import { useDesktopApi, type BluetoothPairing, type PhoneCalls } from "@/lib/api";
import { errorMessage } from "@/lib/utils";

import { requestText } from "./copy";
import { formatDuration, groupCode } from "./format";
import { useClock } from "./use-clock";

type OnError = (message: string | null) => void;

/** Runs a command, and reports its failure instead of throwing. */
function useRun(onError: OnError) {
  return (action: () => Promise<void>) => {
    onError(null);
    action().catch((reason: unknown) => {
      onError(errorMessage(reason));
    });
  };
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
  const run = useRun(onError);
  const now = useClock();
  const titleId = useId();
  const prompt = pairing.prompt;

  return (
    <Card className="calls-setup">
      <section aria-labelledby={titleId} className="calls-setup__body">
        <h2 id={titleId} className="calls-setup__title">
          {`Pair ${phoneName} over Bluetooth`}
        </h2>
        <p className="calls-setup__text">{requestText(pairing.phone, phoneName)}</p>
        <ol className="calls-setup__steps">
          <li>{`On ${phoneName}, open Bluetooth settings and pick “${pairing.adapterName}”.`}</li>
          <li>Check that both devices show the same code, and confirm on each.</li>
        </ol>
        {prompt ? (
          <div className="pairing-prompt" role="alert">
            <p className="calls-setup__text">
              {`${prompt.deviceName || "A device"} wants to pair. Does it show this code?`}
            </p>
            <p className="pairing-prompt__code" aria-label={`Code ${prompt.code}`}>
              {groupCode(prompt.code)}
            </p>
            <div className="calls-setup__actions">
              <Button
                variant="primary"
                icon={Check}
                onClick={() => {
                  run(() => api.confirmBluetoothCode(prompt.id, true));
                }}
              >
                Yes, it matches
              </Button>
              <Button
                icon={X}
                onClick={() => {
                  run(() => api.confirmBluetoothCode(prompt.id, false));
                }}
              >
                No
              </Button>
            </div>
          </div>
        ) : (
          <Spinner label="Waiting for your phone…" />
        )}
        <div className="calls-setup__actions">
          <span className="calls-setup__hint">
            {`Pairing stays open for ${formatDuration(pairing.endsUnixMs - now)}.`}
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
  const run = useRun(onError);
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
            <Button variant="primary" icon={Bluetooth} onClick={pair}>
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
            Several devices paired with this computer could be it. Pick it once; link2linux keeps
            your choice.
          </p>
          <ul className="calls-choices">
            {link.candidates.map((candidate) => (
              <li key={candidate.address}>
                <Button
                  icon={Bluetooth}
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
            <Button variant="ghost" onClick={pair}>
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
              onClick={() => {
                run(() => api.connectCalls(phone.deviceId));
              }}
            >
              Connect
            </Button>
            <Button variant="ghost" onClick={pair}>
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
