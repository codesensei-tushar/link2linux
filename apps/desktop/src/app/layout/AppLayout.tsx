import { useEffect, useRef, type MouseEvent } from "react";
import { Outlet, useLocation } from "react-router";

import { features } from "@/features";

import { Sidebar } from "./Sidebar";
import "./layout.css";

/** The app frame: sidebar navigation on the left, the current page on the right. */
export function AppLayout() {
  const mainRef = useRef<HTMLElement>(null);
  const { pathname } = useLocation();
  const current = features.find((f) => pathname === `/${f.id}`);

  useEffect(() => {
    document.title = current ? `${current.label} · link2linux` : "link2linux";
  }, [current]);

  // Routes live in the URL hash, so the skip link moves focus itself
  // instead of navigating to "#main".
  const skipToMain = (event: MouseEvent) => {
    event.preventDefault();
    mainRef.current?.focus();
  };

  return (
    <div className="app-shell">
      <a className="skip-link" href="#main" onClick={skipToMain}>
        Skip to content
      </a>
      <Sidebar />
      <main id="main" ref={mainRef} tabIndex={-1} className="app-main">
        <Outlet />
      </main>
    </div>
  );
}
