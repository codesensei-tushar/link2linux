import { ClipboardCopy } from "lucide-react";

import type { FeatureMeta } from "@/features/types";

export const clipboardMeta: FeatureMeta = {
  id: "clipboard",
  label: "Clipboard",
  icon: ClipboardCopy,
  description: "Copy text on one device and paste it on the other.",
};
