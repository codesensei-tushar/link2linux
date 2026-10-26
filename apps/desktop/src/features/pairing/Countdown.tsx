import { useEffect, useState } from "react";

import { formatRemaining } from "./format-time";

export interface CountdownProps {
  /** Time left when this component mounted. Remount (change `key`) to restart. */
  expiresInMs: number;
  lifetimeMs: number;
  /** Text before the time, e.g. "New code in". */
  label: string;
}

const RADIUS = 15;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

/** A ring that empties as time passes, with the time left as text. */
export function Countdown({ expiresInMs, lifetimeMs, label }: CountdownProps) {
  const [remaining, setRemaining] = useState(expiresInMs);

  useEffect(() => {
    const deadline = Date.now() + expiresInMs;
    const timer = setInterval(() => {
      setRemaining(Math.max(0, deadline - Date.now()));
    }, 250);
    return () => {
      clearInterval(timer);
    };
  }, [expiresInMs]);

  const fraction = lifetimeMs > 0 ? Math.min(1, remaining / lifetimeMs) : 0;

  return (
    <p className="countdown" role="timer">
      <svg className="countdown__ring" viewBox="0 0 36 36" aria-hidden="true">
        <circle className="countdown__track" cx="18" cy="18" r={RADIUS} />
        <circle
          className="countdown__bar"
          cx="18"
          cy="18"
          r={RADIUS}
          strokeDasharray={CIRCUMFERENCE}
          strokeDashoffset={CIRCUMFERENCE * (1 - fraction)}
        />
      </svg>
      <span>
        {label} <strong>{formatRemaining(remaining)}</strong>
      </span>
    </p>
  );
}
