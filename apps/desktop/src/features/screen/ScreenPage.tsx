import { FeaturePlaceholder } from "@/components/FeaturePlaceholder";

import { screenMeta } from "./meta";

// TODO: replace the placeholder once crates/core provides the screen service.
export function ScreenPage() {
  return (
    <FeaturePlaceholder feature={screenMeta} emptyTitle="Your phone's screen will show up here" />
  );
}
