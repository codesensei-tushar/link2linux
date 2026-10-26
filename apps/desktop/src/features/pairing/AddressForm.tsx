import { useEffect, useId, useRef, useState, type SubmitEvent } from "react";

import { Button } from "@/components/ui";

import { usePairing } from "./pairing-context";

/**
 * The last resort when the phone can't be found on its own: type the address
 * the phone shows. Works while a QR code is up and while searching.
 */
export function AddressForm() {
  const { pairWith, error } = usePairing();
  const [open, setOpen] = useState(false);
  const [address, setAddress] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const inputId = useId();
  const hintId = useId();
  const errorId = useId();

  // The user just asked for this field, so move straight into it.
  useEffect(() => {
    if (open) inputRef.current?.focus();
  }, [open]);

  if (!open) {
    return (
      <>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            setOpen(true);
          }}
        >
          Type the phone&apos;s address instead
        </Button>
        {error && (
          <p role="alert" className="address-form__error">
            {error}
          </p>
        )}
      </>
    );
  }

  const submit = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    void pairWith({ kind: "address", address: address.trim() });
  };

  return (
    <form className="address-form" onSubmit={submit}>
      <label htmlFor={inputId}>Phone address</label>
      <div className="address-form__row">
        {/* The user opened this field on purpose, so focusing it is expected. */}
        {}
        <input
          id={inputId}
          className="text-input"
          value={address}
          ref={inputRef}
          autoComplete="off"
          spellCheck={false}
          inputMode="decimal"
          placeholder="192.168.1.20:27380"
          aria-describedby={error ? `${hintId} ${errorId}` : hintId}
          aria-invalid={error ? true : undefined}
          onChange={(event) => {
            setAddress(event.target.value);
          }}
        />
        <Button type="submit" variant="primary" disabled={address.trim() === ""}>
          Connect
        </Button>
      </div>
      <p id={hintId} className="address-form__hint">
        The phone shows its address after a short wait. The port is optional.
      </p>
      {error && (
        <p id={errorId} role="alert" className="address-form__error">
          {error}
        </p>
      )}
    </form>
  );
}
