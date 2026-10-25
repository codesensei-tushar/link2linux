import type { ComponentProps } from "react";

import { cn } from "@/lib/utils";
import "./Card.css";

/** A bordered surface that groups related content. */
export function Card({ className, ...props }: ComponentProps<"div">) {
  return <div className={cn("card", className)} {...props} />;
}

/** A bordered note that draws attention to supporting information. */
export function Callout({
  tone = "info",
  className,
  ...props
}: ComponentProps<"div"> & { tone?: "info" | "warning" }) {
  return <div role="note" className={cn("callout", `callout--${tone}`, className)} {...props} />;
}
