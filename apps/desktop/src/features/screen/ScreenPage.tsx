import { ArrowLeft, Bell, Circle, MonitorUp, Smartphone, Square, X } from "lucide-react";
import {
  useEffect,
  useEffectEvent,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
  type WheelEvent,
} from "react";

import { Button, Callout, EmptyState, PageHeader, Spinner, StatusDot } from "@/components/ui";
import { useDevices } from "@/features/pairing/devices-context";
import {
  useDesktopApi,
  type Device,
  type ScreenCodec,
  type ScreenFormat,
  type ScreenInput,
  type ScreenKey,
} from "@/lib/api";

import { GestureRecorder, keyInput, toFramePoint, wheelSwipe, type FramePoint } from "./control";
import { ScreenDecoder, type Picture } from "./decoder";
import { endText, inputProblemText, problemText } from "./messages";
import { screenMeta } from "./meta";
import { useDecoderEnv, useH264Support, useScreenStatus } from "./use-screen";
import "./screen.css";

/** The longest side asked for: H.264 handles a full phone screen, JPEG frames stay small. */
const H264_MAX_SIZE = 1920;
const JPEG_MAX_SIZE = 1280;
/** How long wheel turns add up before they become one swipe, in milliseconds. */
const WHEEL_GATHER_MS = 80;

const NAV_KEYS: { key: ScreenKey; label: string; icon: typeof ArrowLeft }[] = [
  { key: "back", label: "Back", icon: ArrowLeft },
  { key: "home", label: "Home", icon: Circle },
  { key: "recents", label: "Recent apps", icon: Square },
  { key: "notifications", label: "Notifications", icon: Bell },
];

/** Draws `picture` on `canvas`, sized to the frames, and closes it. */
function drawOn(
  canvas: HTMLCanvasElement | null,
  picture: Picture,
  width: number,
  height: number,
): void {
  if (canvas) {
    if (canvas.width !== width) canvas.width = width;
    if (canvas.height !== height) canvas.height = height;
    canvas.getContext("2d")?.drawImage(picture, 0, 0, width, height);
  }
  picture.close();
}

function message(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}

/** The screen of one phone: starting it, the phone's question, the stream and its controls. */
function PhoneScreen({ phone }: { phone: Device }) {
  const api = useDesktopApi();
  const env = useDecoderEnv();
  const status = useScreenStatus(phone.id);
  const h264 = useH264Support();
  const [jpegOnly, setJpegOnly] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const typingRef = useRef<HTMLTextAreaElement>(null);
  const decoderRef = useRef<ScreenDecoder | null>(null);
  const startedRef = useRef(false);
  const fellBackRef = useRef(false);
  const gestureRef = useRef<GestureRecorder | null>(null);
  const inputsRef = useRef<Promise<void>>(Promise.resolve());
  const wheelRef = useRef<{ delta: FramePoint; at: FramePoint; timer: number } | null>(null);

  const start = (codecs: ScreenCodec[]) => {
    decoderRef.current?.close();
    const decoder = new ScreenDecoder({
      env,
      draw: (picture, width, height) => {
        drawOn(canvasRef.current, picture, width, height);
      },
      askKeyFrame: () => {
        void api.requestScreenKeyFrame(phone.id).catch(() => undefined);
      },
      unsupported: () => {
        if (fellBackRef.current) {
          stop();
          setError(`This computer can't show the video ${phone.name} sends.`);
          return;
        }
        // The phone's H.264 is one this webview can't decode: start again in JPEG.
        fellBackRef.current = true;
        setJpegOnly(true);
        start(["jpeg"]);
      },
    });
    decoderRef.current = decoder;
    startedRef.current = true;
    setError(null);
    const maxSize = codecs.includes("h264") ? H264_MAX_SIZE : JPEG_MAX_SIZE;
    api
      .startScreen(phone.id, codecs, maxSize, (packet) => {
        decoder.push(packet);
      })
      .catch((reason: unknown) => {
        setError(message(reason));
      });
  };

  const stop = () => {
    if (!startedRef.current) return;
    startedRef.current = false;
    decoderRef.current?.close();
    decoderRef.current = null;
    void api.stopScreen(phone.id).catch(() => undefined);
  };

  const send = (input: ScreenInput) => {
    // One after another, so the phone carries them out in the order the user made them.
    inputsRef.current = inputsRef.current
      .then(() => api.sendScreenInput(phone.id, input))
      .catch((reason: unknown) => {
        setError(message(reason));
      });
  };

  // The stream stops with the page, and with the window.
  const onPageHide = useEffectEvent(() => {
    stop();
  });
  useEffect(() => {
    const hide = () => {
      onPageHide();
    };
    window.addEventListener("pagehide", hide);
    return () => {
      window.removeEventListener("pagehide", hide);
      hide();
    };
  }, []);

  if (status === null || h264 === null) {
    return <Spinner label="Looking at your phone's screen…" />;
  }

  const { phase, format } = status;
  const decodesH264 = h264 && !jpegOnly;
  const blocked = status.blocked && problemText(status.blocked, phone.name);
  const control = phase === "showing" && format?.input === true ? format : null;

  const framePoint = (event: { clientX: number; clientY: number }, size: ScreenFormat) => {
    const rect = canvasRef.current?.getBoundingClientRect() ?? {
      left: 0,
      top: 0,
      width: 0,
      height: 0,
    };
    return toFramePoint(event.clientX, event.clientY, rect, size);
  };

  const onPointerDown = (event: PointerEvent<HTMLCanvasElement>) => {
    if (!control || event.button !== 0) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    gestureRef.current = new GestureRecorder(
      control.id,
      framePoint(event, control),
      event.timeStamp,
    );
    // Typing goes to the phone from now on.
    typingRef.current?.focus({ preventScroll: true });
  };
  const onPointerMove = (event: PointerEvent<HTMLCanvasElement>) => {
    if (control) gestureRef.current?.move(framePoint(event, control), event.timeStamp);
  };
  const onPointerUp = (event: PointerEvent<HTMLCanvasElement>) => {
    const gesture = gestureRef.current;
    gestureRef.current = null;
    if (control && gesture) send(gesture.up(framePoint(event, control), event.timeStamp));
  };
  const onPointerCancel = () => {
    gestureRef.current = null;
  };
  const onWheel = (event: WheelEvent<HTMLCanvasElement>) => {
    if (!control) return;
    const rect = canvasRef.current?.getBoundingClientRect();
    // Wheel turns, in the frames' pixels.
    const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? (rect?.height ?? 800) : 1;
    const scale = rect && rect.height > 0 ? control.height / rect.height : 1;
    const gathered = wheelRef.current ?? {
      delta: { x: 0, y: 0 },
      at: framePoint(event, control),
      timer: 0,
    };
    gathered.delta = {
      x: gathered.delta.x + event.deltaX * unit * scale,
      y: gathered.delta.y + event.deltaY * unit * scale,
    };
    window.clearTimeout(gathered.timer);
    gathered.timer = window.setTimeout(() => {
      wheelRef.current = null;
      const swipe = wheelSwipe(control.id, control, gathered.at, gathered.delta);
      if (swipe) send(swipe);
    }, WHEEL_GATHER_MS);
    wheelRef.current = gathered;
  };

  // Text goes to the phone as it is typed, and an IME's text once it is composed.
  const flushTyping = (field: HTMLTextAreaElement) => {
    const text = field.value;
    field.value = "";
    if (control && text.length > 0) send({ kind: "text", text });
  };
  const onTypingKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (!control || event.nativeEvent.isComposing) return;
    const key = keyInput(event.key);
    if (key) {
      event.preventDefault();
      send(key);
    }
  };

  return (
    <div className="screen">
      {error && (
        <Callout tone="warning" role="alert">
          <span>{error}</span>
        </Callout>
      )}
      {phase === "idle" && (
        <div className="screen__start">
          {status.ended && <p className="screen__note">{endText(status.ended, phone.name)}</p>}
          {status.problem && status.problem !== status.blocked && (
            <p className="screen__note" role="alert">
              {problemText(status.problem, phone.name)}
            </p>
          )}
          {blocked ? (
            <p className="screen__note">{blocked}</p>
          ) : status.blocked === "waiting" ? (
            <Spinner label={`Waiting for ${phone.name}…`} />
          ) : (
            <Button
              variant="primary"
              icon={MonitorUp}
              onClick={() => {
                start(decodesH264 ? ["h264", "jpeg"] : ["jpeg"]);
              }}
            >
              {`Show ${phone.name}'s screen`}
            </Button>
          )}
          {!decodesH264 && (
            <p className="screen__hint">
              Video is smoother with an H.264 decoder for GStreamer, such as gstreamer1.0-libav.
              Install it, then restart link2linux.
            </p>
          )}
        </div>
      )}
      {(phase === "starting" || phase === "asking") && (
        <div className="screen__asking">
          {phase === "starting" ? (
            <Spinner label={`Asking ${phone.name}…`} />
          ) : (
            <Callout>
              <span>
                {`Check ${phone.name}: it asks whether to share its screen with this computer. ` +
                  "Agree there to see it here."}
              </span>
            </Callout>
          )}
          <Button icon={X} onClick={stop}>
            Cancel
          </Button>
        </div>
      )}
      {phase === "stopping" && <Spinner label="Stopping…" />}
      {phase === "showing" && (
        <div className="screen__bar" role="toolbar" aria-label="Phone controls">
          {NAV_KEYS.map(({ key, label, icon }) => (
            <Button
              key={key}
              size="sm"
              icon={icon}
              disabled={!control}
              onClick={() => {
                send({ kind: "key", key });
              }}
            >
              {label}
            </Button>
          ))}
          <Button size="sm" variant="primary" icon={X} onClick={stop}>
            Stop
          </Button>
        </div>
      )}
      {phase === "showing" && format && !format.input && (
        <p className="screen__note">
          {`Control is off on ${phone.name}. To use the mouse and keyboard here, turn on ` +
            "control in the link2linux app on your phone."}
        </p>
      )}
      {phase === "showing" && status.inputProblem && (
        <p className="screen__note" role="alert">
          {inputProblemText(status.inputProblem, phone.name)}
        </p>
      )}
      <div className="screen__view" hidden={phase !== "showing"}>
        <canvas
          ref={canvasRef}
          className="screen__canvas"
          data-control={control ? "on" : "off"}
          aria-label={`${phone.name}'s screen`}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerCancel}
          onWheel={onWheel}
        />
        <textarea
          ref={typingRef}
          className="screen__typing"
          aria-label={`Type on ${phone.name}`}
          disabled={!control}
          autoComplete="off"
          spellCheck={false}
          onKeyDown={onTypingKeyDown}
          onInput={(event) => {
            if (!event.nativeEvent.isComposing) flushTyping(event.currentTarget);
          }}
          onCompositionEnd={(event) => {
            flushTyping(event.currentTarget);
          }}
        />
      </div>
    </div>
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
            name="screen-phone"
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

export function ScreenPage() {
  const { devices } = useDevices();
  const [chosen, setChosen] = useState<string | null>(null);
  const phones =
    devices.status === "ready" ? devices.data.filter((d) => d.status !== "revoked") : [];
  const phone =
    phones.find((p) => p.id === chosen) ??
    phones.find((p) => p.status === "connected") ??
    phones[0];

  return (
    <>
      <PageHeader title={screenMeta.label} description={screenMeta.description} />
      <div className="page-body">
        {devices.status === "loading" ? (
          <Spinner label="Looking for your phone…" />
        ) : phone ? (
          <>
            {phones.length > 1 && (
              <PhonePicker phones={phones} selected={phone.id} onSelect={setChosen} />
            )}
            <PhoneScreen key={phone.id} phone={phone} />
          </>
        ) : (
          <EmptyState
            icon={Smartphone}
            title="No phone paired"
            description="Pair your phone to see and control its screen here."
          />
        )}
      </div>
    </>
  );
}
