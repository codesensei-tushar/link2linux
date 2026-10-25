import { FeaturePlaceholder } from "@/components/FeaturePlaceholder";

import { messagesMeta } from "./meta";

// TODO: replace the placeholder once crates/core provides the messages service.
export function MessagesPage() {
  return (
    <FeaturePlaceholder feature={messagesMeta} emptyTitle="Your conversations will show up here" />
  );
}
