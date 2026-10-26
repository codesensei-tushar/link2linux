import { useCallback, useEffect, useState } from "react";

import { useDesktopApi } from "@/lib/api";
import { errorMessage } from "@/lib/utils";

import { applyEvent } from "./list";
import type { MirroredNotification, NotificationActionInfo, NotificationEvent } from "./types";

export type NotificationsState =
  | { status: "loading" }
  | { status: "ready"; notifications: MirroredNotification[] }
  | { status: "error"; error: string };

/** What a screen reader hears when a notification arrives. Never reads redacted content. */
function announcement(n: MirroredNotification): string {
  if (n.content !== "full" || !n.title) return `New notification from ${n.appName}`;
  return `New notification from ${n.appName}: ${n.title}`;
}

/**
 * Follows a phone's mirrored notifications: the current list plus live changes.
 * `announcement` is text for an ARIA live region, set when the phone asks to alert.
 *
 * Render it under `key={deviceId}` so that switching phones starts afresh.
 */
export function useNotifications(deviceId: string) {
  const api = useDesktopApi();
  const [state, setState] = useState<NotificationsState>({ status: "loading" });
  const [live, setLive] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    // Events can arrive before the first list does, so they wait for it.
    let loaded = false;
    const pending: NotificationEvent[] = [];

    const apply = (event: NotificationEvent) => {
      setState((s) =>
        s.status === "ready"
          ? { status: "ready", notifications: applyEvent(s.notifications, event) }
          : s,
      );
      if (event.type === "posted" && event.alert) setLive(announcement(event.notification));
    };
    const unsubscribe = api.subscribeNotifications(deviceId, (event) => {
      if (!current) return;
      if (loaded) apply(event);
      else pending.push(event);
    });

    api.listNotifications(deviceId).then(
      (notifications) => {
        if (!current) return;
        loaded = true;
        setState({ status: "ready", notifications });
        pending.splice(0).forEach(apply);
      },
      (error: unknown) => {
        if (current) setState({ status: "error", error: errorMessage(error) });
      },
    );

    return () => {
      current = false;
      unsubscribe();
    };
  }, [api, deviceId, attempt]);

  /** Loads the list again, for example after it failed to load. */
  const reload = useCallback(() => {
    setState({ status: "loading" });
    setAttempt((n) => n + 1);
  }, []);

  /** Removes the notification at once, and puts it back with an error if the phone refuses. */
  const dismiss = useCallback(
    (notification: MirroredNotification) => {
      setState((s) =>
        s.status === "ready"
          ? {
              status: "ready",
              notifications: applyEvent(s.notifications, {
                type: "removed",
                key: notification.key,
              }),
            }
          : s,
      );
      api.dismissNotification(deviceId, notification.key).catch((cause: unknown) => {
        setState((s) =>
          s.status === "ready"
            ? {
                status: "ready",
                notifications: applyEvent(s.notifications, {
                  type: "posted",
                  notification,
                  alert: false,
                }),
              }
            : s,
        );
        setError(`Couldn't dismiss the notification: ${errorMessage(cause)}`);
      });
    },
    [api, deviceId],
  );

  const runAction = useCallback(
    (notification: MirroredNotification, action: NotificationActionInfo, replyText?: string) => {
      api
        .runNotificationAction(deviceId, notification.key, action.id, replyText)
        .catch((cause: unknown) => {
          setError(`Couldn't run "${action.label}": ${errorMessage(cause)}`);
        });
    },
    [api, deviceId],
  );

  const clearError = useCallback(() => {
    setError(null);
  }, []);

  return { state, announcement: live, error, clearError, dismiss, runAction, reload };
}
