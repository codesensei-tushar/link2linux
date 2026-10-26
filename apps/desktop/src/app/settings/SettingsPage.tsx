import { RefreshCw, TriangleAlert } from "lucide-react";
import { useCallback, useId, useState } from "react";

import { Button, Callout, Card, EmptyState, PageHeader, Spinner } from "@/components/ui";
import { useDesktopApi, type Settings } from "@/lib/api";
import { useAsync } from "@/lib/use-async";
import { errorMessage } from "@/lib/utils";

import "./settings.css";

/** The desktop app's own settings. */
export function SettingsPage() {
  const api = useDesktopApi();
  const load = useCallback(() => api.getSettings(), [api]);
  const [loaded, reload] = useAsync(load);
  const [changed, setChanged] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const inputId = useId();
  const hintId = useId();

  const settings = changed ?? (loaded.status === "ready" ? loaded.data : null);

  const update = (next: Settings) => {
    setChanged(next);
    setError(null);
    api.setSettings(next).catch((reason: unknown) => {
      setError(errorMessage(reason));
    });
  };

  let body;
  if (settings) {
    body = (
      <Card>
        <h2 className="settings__group">Notifications</h2>
        <div className="setting">
          <input
            id={inputId}
            type="checkbox"
            role="switch"
            checked={settings.showNotificationText ?? false}
            aria-describedby={hintId}
            onChange={(event) => {
              update({ ...settings, showNotificationText: event.target.checked });
            }}
          />
          <div className="setting__text">
            <label htmlFor={inputId} className="setting__label">
              Show notification text in pop-ups
            </label>
            <p id={hintId} className="setting__hint">
              Pop-ups pass through your desktop's notification service, where other apps on this
              computer can read them. With this off, a pop-up only names the app, and the
              notification stays in this window. While the screen is locked, pop-ups never show the
              text.
            </p>
          </div>
        </div>
      </Card>
    );
  } else if (loaded.status === "error") {
    body = (
      <EmptyState
        icon={TriangleAlert}
        title="Couldn't load the settings"
        description={loaded.error}
        actions={
          <Button variant="secondary" icon={RefreshCw} onClick={reload}>
            Try again
          </Button>
        }
      />
    );
  } else {
    body = <Spinner label="Loading settings…" />;
  }

  return (
    <>
      <PageHeader title="Settings" description="How link2linux behaves on this computer." />
      <div className="page-body settings">
        {body}
        {error && <Callout tone="warning">{error}</Callout>}
      </div>
    </>
  );
}
