import type { LucideIcon } from "lucide-react";
import type { ComponentType } from "react";

/** Feature names match the Android modules and the crates/core services. */
export type FeatureId =
  "pairing" | "notifications" | "calls" | "photos" | "files" | "clipboard" | "screen";

export interface FeatureMeta {
  /** Also the route path: `/<id>`. */
  id: FeatureId;
  label: string;
  icon: LucideIcon;
  /** One neutral sentence on what the feature does. */
  description: string;
}

export interface Feature extends FeatureMeta {
  Page: ComponentType;
}
