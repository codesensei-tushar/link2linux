import { RefreshCw, TriangleAlert } from "lucide-react";
import { useCallback, useId, useState, type ReactNode } from "react";

import { Button, Callout, Card, EmptyState, PageHeader, Spinner } from "@/components/ui";
import { useDesktopApi, type Settings } from "@/lib/api";
import { useAsync } from "@/lib/use-async";
import { errorMessage } from "@/lib/utils";

import "./settings.css";

interface ToggleProps {
  label: string;
  hint: ReactNode;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}

/** One on/off setting with its explanation. */
function Toggle({ label, hint, checked, disabled = false, onChange }: ToggleProps) {
  const inputId = useId();
  const hintId = useId();
  return (
    <div className="setting">
      <input
        id={inputId}
        type="checkbox"
        role="switch"
        checked={checked}
        disabled={disabled}
        aria-describedby={hintId}
        onChange={(event) => {
          onChange(event.target.checked);
        }}
      />
      <div className="setting__text">
        <label htmlFor={inputId} className="setting__label">
          {label}
        </label>
        <p id={hintId} className="setting__hint">
          {hint}
        </p>
      </div>
    </div>
  );
}

/** The desktop app's own settings. */
export function SettingsPage() {
  const api = useDesktopApi();
  const load = useCallback(() => api.getSettings(), [api]);
  const [loaded, reload] = useAsync(load);
  const [changed, setChanged] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);

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
    const sharing = settings.shareClipboard ?? true;
    body = (
      <>
        <Card>
          <h2 className="settings__group">Notifications</h2>
          <Toggle
            label="Show notification text in pop-ups"
            checked={settings.showNotificationText ?? false}
            onChange={(checked) => {
              update({ ...settings, showNotificationText: checked });
            }}
            hint="Pop-ups pass through your desktop's notification service, where other apps on this computer can read them. With this off, a pop-up only names the app, and the notification stays in this window. While the screen is locked, or this app can't tell whether it is, pop-ups never show the text."
          />
        </Card>
        <Card>
          <h2 className="settings__group">Clipboard</h2>
          <Toggle
            label="Share the clipboard with your phones"
            checked={sharing}
            onChange={(checked) => {
              update({ ...settings, shareClipboard: checked });
            }}
            hint="Text you copy here goes to your phones, and text you share from a phone lands on this computer's clipboard. Each phone also has its own switch in the link2linux app."
          />
          <Toggle
            label="Share passwords"
            checked={settings.sharePasswords ?? false}
            disabled={!sharing}
            onChange={(checked) => {
              update({ ...settings, sharePasswords: checked });
            }}
            hint="Password managers mark the passwords you copy. With this off, they stay on this computer. Phones keep the mark, so their clipboard history and previews leave the passwords out."
          />
        </Card>
      </>
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
