import { cn } from "@/lib/utils";
import "./Spinner.css";

export interface SpinnerProps {
  /** Describes what is loading. Always announced; shown unless `hideLabel`. */
  label: string;
  hideLabel?: boolean;
  className?: string;
}

/** An indeterminate loading indicator with a live label. */
export function Spinner({ label, hideLabel = false, className }: SpinnerProps) {
  return (
    <span role="status" className={cn("spinner", className)}>
      <span className="spinner__ring" aria-hidden="true" />
      <span className={hideLabel ? "sr-only" : "spinner__label"}>{label}</span>
    </span>
  );
}
