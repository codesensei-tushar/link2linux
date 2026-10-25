import { Badge, EmptyState, PageHeader } from "@/components/ui";
import type { FeatureMeta } from "@/features/types";

export interface FeaturePlaceholderProps {
  feature: FeatureMeta;
  /** What will fill the page once the feature works. */
  emptyTitle: string;
}

/** Stand-in page for a feature that is not built yet. */
export function FeaturePlaceholder({ feature, emptyTitle }: FeaturePlaceholderProps) {
  return (
    <>
      <PageHeader
        title={feature.label}
        description={feature.description}
        actions={<Badge tone="accent">In development</Badge>}
      />
      <div className="page-body">
        <EmptyState
          icon={feature.icon}
          title={emptyTitle}
          description="This feature is still in development. It will work with any phone you pair with this computer."
        />
      </div>
    </>
  );
}
