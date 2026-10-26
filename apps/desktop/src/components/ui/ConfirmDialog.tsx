import { useEffect, useId, useRef, type KeyboardEvent, type ReactNode } from "react";

import { Button } from "./Button";
import "./ConfirmDialog.css";

export interface ConfirmDialogProps {
  title: string;
  children: ReactNode;
  confirmLabel: string;
  /** Shown inside the dialog when the confirmed action failed. */
  error?: string | null;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * A modal question with two answers. Render it only while it is open.
 *
 * Focus starts on Cancel (the safe answer), Tab stays inside, Escape cancels,
 * and focus goes back to whatever had it when the dialog opened.
 */
export function ConfirmDialog({
  title,
  children,
  confirmLabel,
  error,
  busy = false,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const titleId = useId();
  const bodyId = useId();
  const cancelRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const opener = document.activeElement;
    cancelRef.current?.focus();
    return () => {
      if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
    };
  }, []);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      onCancel();
    } else if (event.key === "Tab") {
      // Two buttons: wrapping around by hand keeps focus in the dialog.
      const first = cancelRef.current;
      const last = confirmRef.current;
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    }
  };

  return (
    <div className="dialog-backdrop" onKeyDown={onKeyDown} role="presentation">
      <div
        className="dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={bodyId}
      >
        <h2 id={titleId} className="dialog__title">
          {title}
        </h2>
        <div id={bodyId} className="dialog__body">
          {children}
        </div>
        {error && (
          <p role="alert" className="dialog__error">
            {error}
          </p>
        )}
        <div className="dialog__actions">
          <Button ref={cancelRef} onClick={onCancel}>
            Cancel
          </Button>
          <Button ref={confirmRef} variant="primary" disabled={busy} onClick={onConfirm}>
            {confirmLabel}
          </Button>
        </div>
      </div>
    </div>
  );
}
