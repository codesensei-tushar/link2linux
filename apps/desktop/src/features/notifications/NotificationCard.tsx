import { useEffect, useState } from "react";

import { useDesktopApi } from "@/lib/api";
import { EyeOff, X } from "lucide-react";

import { Button } from "@/components/ui";
import { NotificationActions } from "./NotificationActions";
import type { ImageRef, MirroredNotification, NotificationActionInfo } from "./types";
import { relativeTime } from "./relative-time";

/** The app's icon when the phone shared one, else its first letter. */
function AppIcon({
  deviceId,
  name,
  image,
}: {
  deviceId: string;
  name: string;
  image: ImageRef | null;
}) {
  const api = useDesktopApi();
  const [url, setUrl] = useState<string | null>(null);
  const sha256 = image?.sha256;

  useEffect(() => {
    if (!sha256) return;
    let current = true;
    api.notificationImage(deviceId, sha256).then(
      (found) => {
        if (current) setUrl(found);
      },
      () => {
        if (current) setUrl(null);
      },
    );
    return () => {
      current = false;
    };
  }, [api, deviceId, sha256]);

  return (
    <span className="notification__icon" aria-hidden="true">
      {url ? <img src={url} alt="" /> : name.trim().charAt(0).toUpperCase() || "?"}
    </span>
  );
}

/** Why a notification has no content to show, for each way the phone can withhold it. */
const withheldExplanation = {
  redactedBySystem: "Content hidden by Android. Check your phone",
  redactedAppLocked: "Hidden because the app is locked on your phone",
  hiddenByUser: "You chose not to share this app's content",
  oneTimeCode: "One-time code hidden. Check your phone",
  withheld: "Content hidden on your phone",
} as const;

function Conversation({ notification }: { notification: MirroredNotification }) {
  return (
    <ul className="notification__messages" aria-label="Recent messages">
      {notification.messages.map((message, i) => (
        <li
          key={i}
          className={
            message.outgoing
              ? "notification__message notification__message--out"
              : "notification__message"
          }
        >
          <span className="notification__sender">{message.outgoing ? "You" : message.sender}</span>{" "}
          {message.text}
        </li>
      ))}
    </ul>
  );
}

export interface NotificationCardProps {
  deviceId: string;
  notification: MirroredNotification;
  /** The time the page shows relative times against. */
  now: number;
  onDismiss: (notification: MirroredNotification) => void;
  onRunAction: (
    notification: MirroredNotification,
    action: NotificationActionInfo,
    replyText?: string,
  ) => void;
}

/** One mirrored notification. */
export function NotificationCard({
  deviceId,
  notification: n,
  now,
  onDismiss,
  onRunAction,
}: NotificationCardProps) {
  const withheld = n.content === "full" ? null : withheldExplanation[n.content];
  const conversation = n.messages.length > 0;
  const { progress } = n;

  return (
    <article
      className="notification"
      aria-label={`${n.appName}: ${withheld ? "content hidden" : n.title || "notification"}`}
    >
      <AppIcon deviceId={deviceId} name={n.appName} image={n.appIcon} />
      <div className="notification__main">
        <header className="notification__header">
          <span className="notification__app">{n.appName}</span>
          <time dateTime={new Date(n.whenUnixMs).toISOString()}>
            {relativeTime(n.whenUnixMs, now)}
          </time>
          {n.dismissible && (
            <Button
              variant="ghost"
              size="sm"
              icon={X}
              className="notification__dismiss"
              aria-label={`Dismiss ${n.title || n.appName}`}
              onClick={() => {
                onDismiss(n);
              }}
            />
          )}
        </header>
        {withheld ? (
          <p className="notification__withheld">
            <EyeOff aria-hidden="true" />
            {withheld}
          </p>
        ) : (
          <>
            {n.title && <p className="notification__title">{n.title}</p>}
            {conversation ? (
              <Conversation notification={n} />
            ) : (
              n.text && <p className="notification__text">{n.text}</p>
            )}
            {n.subText && <p className="notification__sub">{n.subText}</p>}
            {n.actions.length > 0 && (
              <NotificationActions
                actions={n.actions}
                onRun={(action, replyText) => {
                  onRunAction(n, action, replyText);
                }}
              />
            )}
            {progress && (
              <progress
                className="notification__progress"
                aria-label={`${n.title || n.appName} progress`}
                max={progress.indeterminate ? undefined : progress.max}
                value={progress.indeterminate ? undefined : progress.current}
              />
            )}
          </>
        )}
      </div>
    </article>
  );
}
