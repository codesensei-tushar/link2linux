import type { ReactNode } from "react";

import { cn } from "@/lib/utils";
import "./Badge.css";

export type Tone = "neutral" | "accent" | "success" | "warning" | "danger";

export interface BadgeProps {
  tone?: Tone;
  /** Shows a small colored dot before the label. */
  dot?: boolean;
  className?: string;
  children: ReactNode;
}

/** A short, non-interactive label, e.g. a status. */
export function Badge({ tone = "neutral", dot = false, className, children }: BadgeProps) {
  return (
    <span className={cn("badge", `badge--${tone}`, className)}>
      {dot && <StatusDot tone={tone} />}
      {children}
    </span>
  );
}

/** A colored dot. Decorative: always pair it with text that states the status. */
export function StatusDot({ tone = "neutral", className }: { tone?: Tone; className?: string }) {
  return <span className={cn("status-dot", `status-dot--${tone}`, className)} aria-hidden="true" />;
}
