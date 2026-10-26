import { Bell, RefreshCw, Smartphone, TriangleAlert } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { Button, Callout, EmptyState, PageHeader, Spinner } from "@/components/ui";
import { useDevices } from "@/features/pairing/devices-context";

import { groupNotifications } from "./list";
import { NotificationCard } from "./NotificationCard";
import { notificationsMeta } from "./meta";
import type { MirroredNotification } from "./types";
import { useNotifications } from "./use-notifications";
import "./notifications.css";

/** Re-renders every minute so relative times stay current. */
function useNow(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => {
      setNow(Date.now());
    }, 60_000);
    return () => {
      clearInterval(timer);
    };
  }, []);
  return now;
}

function NotificationList({ deviceId }: { deviceId: string }) {
  const { state, announcement, error, clearError, dismiss, runAction, reload } =
    useNotifications(deviceId);
  const now = useNow();
  const listRef = useRef<HTMLDivElement>(null);
  const handleDismiss = (notification: MirroredNotification) => {
    dismiss(notification);
    // The dismissed item is gone, so keep keyboard users in the list.
    listRef.current?.focus();
  };
  const groups = useMemo(
    () => (state.status === "ready" ? groupNotifications(state.notifications) : []),
    [state],
  );

  let body;
  if (state.status === "loading") {
    body = <Spinner label="Loading notifications…" />;
  } else if (state.status === "error") {
    body = (
      <EmptyState
        icon={TriangleAlert}
        title="Couldn't load notifications"
        description={state.error}
        actions={
          <Button icon={RefreshCw} onClick={reload}>
            Try again
          </Button>
        }
      />
    );
  } else if (groups.length === 0) {
    body = (
      <EmptyState
        icon={Bell}
        title="No notifications"
        description="Notifications from your phone will show up here as they arrive."
      />
    );
  } else {
    body = (
      <ul className="notification-list" aria-label="Notifications">
        {groups.map((group) => (
          <li key={group.id}>
            {group.items.length === 1 ? (
              <NotificationCard
                deviceId={deviceId}
                notification={group.first}
                now={now}
                onDismiss={handleDismiss}
                onRunAction={runAction}
              />
            ) : (
              <section
                className="notification-group"
                aria-label={`${group.first.appName}, ${group.items.length} notifications`}
              >
                {group.items.map((n) => (
                  <NotificationCard
                    key={n.key}
                    deviceId={deviceId}
                    notification={n}
                    now={now}
                    onDismiss={handleDismiss}
                    onRunAction={runAction}
                  />
                ))}
              </section>
            )}
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
      {error && (
        <Callout tone="warning" role="alert" className="notification-toast">
          <span>{error}</span>
          <Button size="sm" variant="ghost" onClick={clearError}>
            Close
          </Button>
        </Callout>
      )}
      <div ref={listRef} tabIndex={-1} className="notification-list-wrap">
        {body}
      </div>
    </>
  );
}

export function NotificationsPage() {
  const { devices } = useDevices();
  const device =
    devices.status === "ready" ? devices.data.find((d) => d.status === "connected") : undefined;

  return (
    <>
      <PageHeader title={notificationsMeta.label} description={notificationsMeta.description} />
      <div className="page-body">
        {devices.status === "loading" ? (
          <Spinner label="Looking for your phone…" />
        ) : device ? (
          <NotificationList key={device.id} deviceId={device.id} />
        ) : (
          <EmptyState
            icon={Smartphone}
            title="No phone connected"
            description="Connect your phone to see its notifications here."
          />
        )}
      </div>
    </>
  );
}
