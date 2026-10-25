import { FeaturePlaceholder } from "@/components/FeaturePlaceholder";

import { notificationsMeta } from "./meta";

// TODO: replace the placeholder once crates/core provides the notifications service.
export function NotificationsPage() {
  return (
    <FeaturePlaceholder
      feature={notificationsMeta}
      emptyTitle="Your phone's notifications will show up here"
    />
  );
}
