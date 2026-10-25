import type { LucideIcon } from "lucide-react";
import { useId, type ReactNode } from "react";

import "./EmptyState.css";

export interface EmptyStateProps {
  icon: LucideIcon;
  title: string;
  description: ReactNode;
  /** Buttons or links; put the main action first. */
  actions?: ReactNode;
  /** Pages already own the h1, so this defaults to h2. */
  headingLevel?: 2 | 3;
}

/** Explains why an area is empty and what to do next. */
export function EmptyState({
  icon: Icon,
  title,
  description,
  actions,
  headingLevel = 2,
}: EmptyStateProps) {
  const titleId = useId();
  const Heading = headingLevel === 3 ? "h3" : "h2";

  return (
    <section className="empty-state" aria-labelledby={titleId}>
      <div className="empty-state__icon">
        <Icon aria-hidden="true" />
      </div>
      <Heading id={titleId} className="empty-state__title">
        {title}
      </Heading>
      <p className="empty-state__description">{description}</p>
      {actions && <div className="empty-state__actions">{actions}</div>}
    </section>
  );
}
