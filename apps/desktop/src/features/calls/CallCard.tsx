import { Grid3x3, Mic, MicOff, Phone, PhoneOff } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { Button, Card } from "@/components/ui";
import { useDesktopApi, type CallView } from "@/lib/api";
import { cn, errorMessage } from "@/lib/utils";

import { callStateLabel } from "./copy";
import { formatDuration } from "./format";
import { Keypad } from "./Keypad";
import { useClock } from "./use-clock";
import { useRun } from "./use-run";

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
  phoneName,
  call,
  mute,
  onError,
}: {
  deviceId: string;
  /** Names the phone when the page shows the calls of several. */
  phoneName?: string;
  call: CallView;
  mute: Mute;
  onError: (message: string | null) => void;
}) {
  const api = useDesktopApi();
  const [keypad, setKeypad] = useState(false);
  const caller = call.name || call.number || "Unknown caller";
  const detail = call.name && call.number ? call.number : null;
  const ringing = call.state === "incoming" || call.state === "waiting";
  const { run, busy } = useRun(onError);
  const section = useRef<HTMLElement>(null);
  const hadFocus = useRef(false);
  // Tones are sent one after the other, so fast taps arrive in order.
  const tones = useRef<Promise<void>>(Promise.resolve());
  const sendTone = (key: string) => {
    onError(null);
    tones.current = tones.current
      .then(() => api.sendCallTones(deviceId, key))
      .catch((reason: unknown) => {
        onError(errorMessage(reason));
      });
  };
  // A button the call no longer allows disappears when it is used. Keep
  // keyboard focus in the card instead of dropping it to the page.
  const buttons = [call.canAnswer, call.canDecline, call.canHangUp, mute.can].join();
  useEffect(() => {
    const card = section.current;
    if (hadFocus.current && card && !card.contains(document.activeElement)) {
      card.focus();
    }
    hadFocus.current = false;
  }, [buttons]);
  const act = (action: () => Promise<void>) => {
    hadFocus.current = section.current?.contains(document.activeElement) ?? false;
    run(action);
  };

  return (
    <Card className={cn("call", ringing && "call--ringing")}>
      <section
        ref={section}
        tabIndex={-1}
        aria-label={`Call with ${caller}`}
        className="call__body"
      >
        <div className="call__who">
          <h2 className="call__caller">{caller}</h2>
          {detail && <span className="call__detail">{detail}</span>}
          {phoneName && <span className="call__detail">{`On ${phoneName}`}</span>}
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
              disabled={busy}
              onClick={() => {
                act(() => api.answerCall(deviceId, call.id));
              }}
            >
              Answer
            </Button>
          )}
          {call.canDecline && (
            <Button
              icon={PhoneOff}
              className="call__end"
              disabled={busy}
              onClick={() => {
                act(() => api.declineCall(deviceId, call.id));
              }}
            >
              Decline
            </Button>
          )}
          {call.canHangUp && (
            <Button
              icon={PhoneOff}
              className="call__end"
              disabled={busy}
              onClick={() => {
                act(() => api.hangUpCall(deviceId, call.id));
              }}
            >
              Hang up
            </Button>
          )}
          {call.audioHere && call.state === "active" && mute.can && (
            <Button
              icon={mute.muted ? MicOff : Mic}
              aria-pressed={mute.muted}
              disabled={busy}
              onClick={() => {
                act(() => api.setCallMuted(deviceId, !mute.muted));
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
              sendTone(key);
            }}
          />
        )}
      </section>
    </Card>
  );
}
