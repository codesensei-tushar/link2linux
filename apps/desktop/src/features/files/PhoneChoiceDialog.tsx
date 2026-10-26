import { Smartphone } from "lucide-react";
import { useEffect, useEffectEvent, useId, useRef } from "react";

import { Button } from "@/components/ui";
import type { Device } from "@/lib/api";

export interface PhoneChoiceDialogProps {
  /** What was dropped, for the question. */
  names: string[];
  phones: Device[];
  onChoose: (phone: Device) => void;
  onCancel: () => void;
}

/** "Send these files to which phone?" Render it only while it is open. */
export function PhoneChoiceDialog({ names, phones, onChoose, onCancel }: PhoneChoiceDialogProps) {
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const what = names.length === 1 ? (names[0] ?? "1 file") : `${String(names.length)} files`;

  useEffect(() => {
    const opener = document.activeElement;
    dialogRef.current?.querySelector("button")?.focus();
    return () => {
      if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
    };
  }, []);

  // Listens on the document: the phone whose button has focus can go
  // away, and focus with it.
  const onKeyDown = useEffectEvent((event: KeyboardEvent) => {
    const dialog = dialogRef.current;
    if (event.key === "Escape") {
      event.preventDefault();
      onCancel();
    } else if (event.key === "Tab" && dialog) {
      // Keep focus inside the dialog.
      const buttons = [...dialog.querySelectorAll("button")];
      const first = buttons[0];
      const last = buttons.at(-1);
      const inside = dialog.contains(document.activeElement);
      if (!inside || (event.shiftKey && document.activeElement === first)) {
        event.preventDefault();
        (event.shiftKey ? last : first)?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    }
  });

  useEffect(() => {
    const listener = (event: KeyboardEvent) => {
      onKeyDown(event);
    };
    document.addEventListener("keydown", listener);
    return () => {
      document.removeEventListener("keydown", listener);
    };
  }, []);

  return (
    <div className="dialog-backdrop" role="presentation">
      <div
        ref={dialogRef}
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
      >
        <h2 id={titleId} className="dialog__title">
          Send {what} to which phone?
        </h2>
        <ul className="phone-choice">
          {phones.map((phone) => (
            <li key={phone.id}>
              <Button
                icon={Smartphone}
                className="phone-choice__button"
                aria-label={
                  phone.status === "connected"
                    ? phone.name
                    : `${phone.name}, sends when it connects`
                }
                onClick={() => {
                  onChoose(phone);
                }}
              >
                {phone.name}
                {phone.status !== "connected" && (
                  <span className="phone-choice__note"> · sends when it connects</span>
                )}
              </Button>
            </li>
          ))}
        </ul>
        <div className="dialog__actions">
          <Button onClick={onCancel}>Cancel</Button>
        </div>
      </div>
    </div>
  );
}
