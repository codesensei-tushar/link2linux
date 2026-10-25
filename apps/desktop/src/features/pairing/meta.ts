import { Link2 } from "lucide-react";

import type { FeatureMeta } from "@/features/types";

export const pairingMeta: FeatureMeta = {
  id: "pairing",
  label: "Pairing",
  icon: Link2,
  description:
    "Connect a phone to this computer. You pair once, over your local network, and the two devices remember each other.",
};
