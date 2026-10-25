import type { KeyboardEvent } from "react";
import { NavLink } from "react-router";

import { features } from "@/features";

import { BrandMark } from "./BrandMark";

/** Up/Down/Home/End move focus between navigation links; Tab still works as usual. */
function moveFocus(event: KeyboardEvent<HTMLAnchorElement>) {
  const list = event.currentTarget.closest("ul");
  if (!list) return;
  const links = Array.from(list.querySelectorAll("a"));
  const index = links.indexOf(event.currentTarget);
  const last = links.length - 1;
  const target = {
    ArrowDown: links[index === last ? 0 : index + 1],
    ArrowUp: links[index === 0 ? last : index - 1],
    Home: links[0],
    End: links[last],
  }[event.key];
  if (target) {
    event.preventDefault();
    target.focus();
  }
}

export function Sidebar() {
  return (
    <aside className="sidebar">
      <div className="sidebar__brand">
        <BrandMark className="sidebar__logo" />
        <span>link2linux</span>
      </div>

      <nav className="sidebar__nav" aria-label="Features">
        <ul>
          {features.map(({ id, label, icon: Icon }) => (
            <li key={id}>
              <NavLink to={`/${id}`} className="nav-link" onKeyDown={moveFocus}>
                <Icon aria-hidden="true" />
                {label}
              </NavLink>
            </li>
          ))}
        </ul>
      </nav>
    </aside>
  );
}
