import type { LucideIcon } from "lucide-react";
import type { ComponentProps } from "react";

import { cn } from "@/lib/utils";
import "./Button.css";

export type ButtonVariant = "primary" | "secondary" | "ghost";

export interface ButtonProps extends ComponentProps<"button"> {
  variant?: ButtonVariant;
  size?: "sm" | "md";
  /** Optional icon shown before the label. Decorative: the label names the button. */
  icon?: LucideIcon;
}

export function Button({
  variant = "secondary",
  size = "md",
  icon: Icon,
  type = "button",
  className,
  children,
  ...props
}: ButtonProps) {
  return (
    <button
      type={type}
      className={cn("button", `button--${variant}`, `button--${size}`, className)}
      {...props}
    >
      {Icon && <Icon className="button__icon" aria-hidden="true" />}
      {children}
    </button>
  );
}
