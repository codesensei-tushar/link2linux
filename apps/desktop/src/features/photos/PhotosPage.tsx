import { FeaturePlaceholder } from "@/components/FeaturePlaceholder";

import { photosMeta } from "./meta";

// TODO: replace the placeholder once crates/core provides the photos service.
export function PhotosPage() {
  return <FeaturePlaceholder feature={photosMeta} emptyTitle="Recent photos will show up here" />;
}
