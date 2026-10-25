import { FeaturePlaceholder } from "@/components/FeaturePlaceholder";

import { filesMeta } from "./meta";

// TODO: replace the placeholder once crates/core provides the files service.
export function FilesPage() {
  return <FeaturePlaceholder feature={filesMeta} emptyTitle="Files you send will show up here" />;
}
