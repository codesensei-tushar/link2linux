import { Grid3x3, Mic, MicOff, Phone, PhoneOff } from "lucide-react";
import { useState } from "react";

import { Button, Card } from "@/components/ui";
import { useDesktopApi, type CallView } from "@/lib/api";
import { cn, errorMessage } from "@/lib/utils";

import { callStateLabel } from "./copy";
import { formatDuration } from "./format";
import { Keypad } from "./Keypad";
import { useClock } from "./use-clock";

/** Whether the microphone can be muted in a phone's calls, and is. */
export interface Mute {
  can: boolean;
  muted: boolean;
}

function CallTimer({ since }: { since: number }) {
  const now = useClock();
  return <span className="call__status">{formatDuration(now - since)}</span>;
}

/** One call of a phone, with what can be done with it. */
export function CallCard({
  deviceId,
  call,
  mute,
  onError,
}: {
  deviceId: string;
  call: CallView;
  mute: Mute;
  onError: (message: string | null) => void;
}) {
  const api = useDesktopApi();
  const [keypad, setKeypad] = useState(false);
  const caller = call.name || call.number || "Unknown caller";
  const detail = call.name && call.number ? call.number : null;
  const ringing = call.state === "incoming" || call.state === "waiting";
  const run = (action: () => Promise<void>) => {
    onError(null);
    action().catch((reason: unknown) => {
      onError(errorMessage(reason));
    });
  };

  return (
    <Card className={cn("call", ringing && "call--ringing")}>
      <section aria-label={`Call with ${caller}`} className="call__body">
        <div className="call__who">
          <h2 className="call__caller">{caller}</h2>
          {detail && <span className="call__detail">{detail}</span>}
          {call.appName && <span className="call__detail">{`${call.appName} call`}</span>}
        </div>
        {call.state === "active" && call.activeSinceUnixMs !== null ? (
          <CallTimer since={call.activeSinceUnixMs} />
        ) : (
          <span className="call__status">{callStateLabel[call.state]}</span>
        )}
        {!call.audioHere && <p className="call__note">The call's sound stays on your phone.</p>}
        <div className="call__actions">
          {call.canAnswer && (
            <Button
              variant="primary"
              icon={Phone}
              onClick={() => {
                run(() => api.answerCall(deviceId, call.id));
              }}
            >
              Answer
            </Button>
          )}
          {call.canDecline && (
            <Button
              icon={PhoneOff}
              className="call__end"
              onClick={() => {
                run(() => api.declineCall(deviceId, call.id));
              }}
            >
              Decline
            </Button>
          )}
          {call.canHangUp && (
            <Button
              icon={PhoneOff}
              className="call__end"
              onClick={() => {
                run(() => api.hangUpCall(deviceId, call.id));
              }}
            >
              Hang up
            </Button>
          )}
          {call.audioHere && call.state === "active" && mute.can && (
            <Button
              icon={mute.muted ? MicOff : Mic}
              aria-pressed={mute.muted}
              onClick={() => {
                run(() => api.setCallMuted(deviceId, !mute.muted));
              }}
            >
              Mute
            </Button>
          )}
          {call.canSendTones && (
            <Button
              icon={Grid3x3}
              aria-pressed={keypad}
              onClick={() => {
                setKeypad(!keypad);
              }}
            >
              Keypad
            </Button>
          )}
        </div>
        {call.canSendTones && keypad && (
          <Keypad
            label="Keypad"
            onKey={(key) => {
              run(() => api.sendCallTones(deviceId, key));
            }}
          />
        )}
      </section>
    </Card>
  );
}
