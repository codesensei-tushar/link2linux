import { FeaturePlaceholder } from "@/components/FeaturePlaceholder";

import { clipboardMeta } from "./meta";

// TODO: replace the placeholder once crates/core provides the clipboard service.
export function ClipboardPage() {
  return (
    <FeaturePlaceholder
      feature={clipboardMeta}
      emptyTitle="Shared clipboard items will show up here"
    />
  );
}
