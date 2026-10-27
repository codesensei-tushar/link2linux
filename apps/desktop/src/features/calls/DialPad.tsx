import { Delete, Phone } from "lucide-react";
import { useId, useState, type SubmitEvent } from "react";

import { Button } from "@/components/ui";
import { useDesktopApi } from "@/lib/api";
import { errorMessage } from "@/lib/utils";

import { Keypad } from "./Keypad";

/** Types or taps a number and calls it from the phone. */
export function DialPad({
  deviceId,
  onError,
}: {
  deviceId: string;
  onError: (message: string | null) => void;
}) {
  const api = useDesktopApi();
  const [number, setNumber] = useState("");
  const [dialing, setDialing] = useState(false);
  const titleId = useId();
  const inputId = useId();

  const dial = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    onError(null);
    setDialing(true);
    api
      .dialNumber(deviceId, number)
      .then(() => {
        setNumber("");
      })
      .catch((reason: unknown) => {
        onError(errorMessage(reason));
      })
      .finally(() => {
        setDialing(false);
      });
  };

  return (
    <section className="dial-pad" aria-labelledby={titleId}>
      <h2 id={titleId} className="calls__heading">
        Call a number
      </h2>
      <form className="dial-pad__form" onSubmit={dial}>
        <label htmlFor={inputId} className="sr-only">
          Number
        </label>
        <input
          id={inputId}
          className="dial-pad__number"
          type="tel"
          inputMode="tel"
          autoComplete="off"
          placeholder="Number"
          value={number}
          onChange={(event) => {
            setNumber(event.target.value);
          }}
        />
        {number && (
          <Button
            variant="ghost"
            icon={Delete}
            aria-label="Delete the last digit"
            onClick={() => {
              setNumber(number.slice(0, -1));
            }}
          />
        )}
        <Button type="submit" variant="primary" icon={Phone} disabled={!number.trim() || dialing}>
          Call
        </Button>
      </form>
      <Keypad
        label="Dial pad"
        onKey={(key) => {
          setNumber(number + key);
        }}
      />
    </section>
  );
}
