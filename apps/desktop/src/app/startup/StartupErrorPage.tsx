import { RefreshCw, TriangleAlert } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui";
import { useDesktopApi, type StartupError } from "@/lib/api";
import { errorMessage } from "@/lib/utils";

import "./startup.css";

/** Shown instead of the app when it could not start: what failed, and a way to try again. */
export function StartupErrorPage({ error }: { error: StartupError }) {
  const api = useDesktopApi();
  const [restarting, setRestarting] = useState(false);
  const [restartError, setRestartError] = useState<string | null>(null);

  const restart = () => {
    setRestarting(true);
    setRestartError(null);
    // On success the app is replaced by a new one, so this never resolves.
    api.restartApp().catch((reason: unknown) => {
      setRestarting(false);
      setRestartError(errorMessage(reason));
    });
  };

  return (
    <main className="startup-error">
      <div className="startup-error__icon">
        <TriangleAlert aria-hidden="true" />
      </div>
      <h1 className="startup-error__title">{error.message}</h1>
      <p className="startup-error__hint">{error.hint}</p>
      <Button variant="primary" icon={RefreshCw} disabled={restarting} onClick={restart}>
        {restarting ? "Starting again…" : "Try again"}
      </Button>
      {restartError && (
        <p role="alert" className="startup-error__hint">
          {restartError}
        </p>
      )}
      <details className="startup-error__details">
        <summary>Details</summary>
        <code>{error.detail}</code>
      </details>
    </main>
  );
}
