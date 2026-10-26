import { ArrowDownLeft, ArrowUpRight, RefreshCw, Send, TriangleAlert } from "lucide-react";
import { useCallback, useId, useState } from "react";
import { Link } from "react-router";

import { Badge, Button, Callout, Card, EmptyState, PageHeader, Spinner } from "@/components/ui";
import { relativeTime } from "@/features/notifications/relative-time";
import { useDesktopApi, type ClipboardActivity, type ClipboardStatus } from "@/lib/api";
import { useAsync } from "@/lib/use-async";
import { useLive } from "@/lib/use-live";
import { useNow } from "@/lib/use-now";
import { errorMessage } from "@/lib/utils";

import { modeCopy, phoneStateCopy, problemText } from "./copy";
import { formatBytes } from "./format";
import { clipboardMeta } from "./meta";
import "./clipboard.css";

function ActivityRow({ entry, now }: { entry: ClipboardActivity; now: number }) {
  const sent = entry.direction === "sent";
  const Icon = sent ? ArrowUpRight : ArrowDownLeft;
  return (
    <li className="clipboard-activity__item">
      <Icon className="clipboard-activity__icon" aria-hidden="true" />
      <span className="clipboard-activity__what">
        {sent ? `Sent to ${entry.phoneName}` : `Received from ${entry.phoneName}`}
      </span>
      <span className="clipboard-activity__size">
        {formatBytes(entry.bytes)}
        {entry.sensitive && " · password"}
      </span>
      <time className="clipboard-activity__time" dateTime={new Date(entry.atUnixMs).toISOString()}>
        {relativeTime(entry.atUnixMs, now)}
      </time>
    </li>
  );
}

function ClipboardView({ status, sharing }: { status: ClipboardStatus; sharing: boolean }) {
  const api = useDesktopApi();
  const now = useNow();
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const hintId = useId();
  const phonesId = useId();
  const activityId = useId();

  const anyReady = status.phones.some((phone) => phone.state === "ready");
  let hint = null;
  if (!sharing) hint = "Clipboard sharing is off.";
  else if (!anyReady) hint = "No phone can take the clipboard right now.";
  const mode = modeCopy[status.mode];

  const send = () => {
    setSending(true);
    setError(null);
    api
      .sendClipboard()
      .catch((reason: unknown) => {
        setError(errorMessage(reason));
      })
      .finally(() => {
        setSending(false);
      });
  };

  return (
    <>
      <Card className="clipboard-mode">
        {sharing ? (
          <>
            <h2 className="clipboard-mode__title">{mode.title}</h2>
            <p className="clipboard-mode__body">{mode.body}</p>
          </>
        ) : (
          <>
            <h2 className="clipboard-mode__title">Clipboard sharing is off</h2>
            <p className="clipboard-mode__body">
              Nothing is shared with your phones. <Link to="/settings">Turn it on in Settings</Link>
              .
            </p>
          </>
        )}
        {status.mode !== "unavailable" && (
          <div className="clipboard-mode__send">
            <Button
              variant="primary"
              icon={Send}
              disabled={hint !== null || sending}
              aria-describedby={hint ? hintId : undefined}
              onClick={send}
            >
              {sending ? "Sending…" : "Send clipboard to phone"}
            </Button>
            {hint && (
              <span id={hintId} className="clipboard-mode__hint">
                {hint}
              </span>
            )}
          </div>
        )}
      </Card>

      {status.held && (
        <Callout>
          Text from {status.held.phoneName} ({formatBytes(status.held.bytes)}) is waiting. It goes
          on this computer's clipboard when this window is focused.
        </Callout>
      )}
      {status.problem && (
        <Callout tone="warning">
          {problemText(status.problem)}{" "}
          <time
            className="clipboard__when"
            dateTime={new Date(status.problem.atUnixMs).toISOString()}
          >
            {relativeTime(status.problem.atUnixMs, now)}
          </time>
        </Callout>
      )}
      {error && (
        <Callout tone="warning" role="alert">
          {error}
        </Callout>
      )}

      {status.phones.length > 0 && (
        <section className="clipboard__section" aria-labelledby={phonesId}>
          <h2 id={phonesId} className="clipboard__heading">
            Phones
          </h2>
          <ul className="clipboard-phones">
            {status.phones.map((phone) => {
              const state = phoneStateCopy[phone.state];
              return (
                <li key={phone.deviceId} className="clipboard-phones__item">
                  <span>{phone.name}</span>
                  <Badge tone={state.tone} dot>
                    {state.label}
                  </Badge>
                </li>
              );
            })}
          </ul>
        </section>
      )}

      <section className="clipboard__section" aria-labelledby={activityId}>
        <h2 id={activityId} className="clipboard__heading">
          Recent activity
        </h2>
        {status.activity.length === 0 ? (
          <p className="clipboard__empty">
            Nothing shared yet. What you send and receive shows up here, without the text itself.
          </p>
        ) : (
          <ul className="clipboard-activity" aria-labelledby={activityId}>
            {status.activity.map((entry, index) => (
              <ActivityRow
                key={`${entry.atUnixMs}-${entry.deviceId}-${index}`}
                entry={entry}
                now={now}
              />
            ))}
          </ul>
        )}
      </section>
    </>
  );
}

/** How the clipboard is shared, a button to send it, and what went where lately. */
export function ClipboardPage() {
  const api = useDesktopApi();
  const subscribe = useCallback(
    (listener: (status: ClipboardStatus) => void) => api.onClipboardChanged(listener),
    [api],
  );
  const load = useCallback(() => api.getClipboardStatus(), [api]);
  const [status, reload] = useLive(subscribe, load);
  const loadSettings = useCallback(() => api.getSettings(), [api]);
  const [settings] = useAsync(loadSettings);
  // Until the settings say otherwise, sharing is on, as it is by default.
  const sharing = settings.status !== "ready" || (settings.data.shareClipboard ?? true);

  let body;
  if (status.status === "loading") {
    body = <Spinner label="Loading the clipboard…" />;
  } else if (status.status === "error") {
    body = (
      <EmptyState
        icon={TriangleAlert}
        title="Couldn't load the clipboard"
        description={status.error}
        actions={
          <Button icon={RefreshCw} onClick={reload}>
            Try again
          </Button>
        }
      />
    );
  } else {
    body = <ClipboardView status={status.data} sharing={sharing} />;
  }

  return (
    <>
      <PageHeader title={clipboardMeta.label} description={clipboardMeta.description} />
      <div className="page-body clipboard">{body}</div>
    </>
  );
}
