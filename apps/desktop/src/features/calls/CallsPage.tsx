import { FeaturePlaceholder } from "@/components/FeaturePlaceholder";

import { callsMeta } from "./meta";

// TODO: replace the placeholder once crates/core provides the calls service.
export function CallsPage() {
  return <FeaturePlaceholder feature={callsMeta} emptyTitle="Recent calls will show up here" />;
}
