import type { ReactNode } from "react";

import "./PageHeader.css";

export interface PageHeaderProps {
  title: string;
  description?: ReactNode;
  /** Page-level actions, aligned to the end of the header. */
  actions?: ReactNode;
}

/** The title block at the top of every page. Owns the page's only h1. */
export function PageHeader({ title, description, actions }: PageHeaderProps) {
  return (
    <header className="page-header">
      <div className="page-header__text">
        <h1 className="page-header__title">{title}</h1>
        {description && <p className="page-header__description">{description}</p>}
      </div>
      {actions && <div className="page-header__actions">{actions}</div>}
    </header>
  );
}
