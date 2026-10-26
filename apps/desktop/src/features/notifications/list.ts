import type { MirroredNotification, NotificationEvent } from "./types";

/** Applies a pushed change to the list. Updates replace the earlier version in place. */
export function applyEvent(
  list: MirroredNotification[],
  event: NotificationEvent,
): MirroredNotification[] {
  switch (event.type) {
    case "reset":
      return event.notifications;
    case "removed":
      return list.filter((n) => n.key !== event.key);
    case "posted": {
      const at = list.findIndex((n) => n.key === event.notification.key);
      if (at === -1) return [...list, event.notification];
      return list.map((n, i) => (i === at ? event.notification : n));
    }
  }
}

/** One row of the page: a single notification, or the notifications of one group. */
export interface NotificationGroup {
  /** Stable across updates: the group, or the key of a lone notification. */
  id: string;
  /** The newest notification, which names the group. */
  first: MirroredNotification;
  /** Newest first. */
  items: MirroredNotification[];
}

/** Groups by `group`, newest first, both within groups and between them. */
export function groupNotifications(list: MirroredNotification[]): NotificationGroup[] {
  const groups = new Map<string, NotificationGroup>();
  for (const n of [...list].sort((a, b) => b.whenUnixMs - a.whenUnixMs)) {
    const id = n.group ? `group:${n.group}` : `key:${n.key}`;
    const existing = groups.get(id);
    if (existing) existing.items.push(n);
    else groups.set(id, { id, first: n, items: [n] });
  }
  // Maps keep insertion order, and each group started with its newest notification.
  return [...groups.values()];
}
