import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Plus } from "lucide-react";
import { describe, expect, it, vi } from "vitest";

import { Button } from "./Button";

describe("Button", () => {
  it("is a plain button by default, so it never submits a form by accident", () => {
    render(<Button>Save</Button>);
    expect(screen.getByRole("button", { name: "Save" })).toHaveAttribute("type", "button");
  });

  it("keeps the icon out of the accessible name", () => {
    render(
      <Button icon={Plus} variant="primary">
        Pair a phone
      </Button>,
    );
    const button = screen.getByRole("button", { name: "Pair a phone" });
    expect(button).toHaveClass("button--primary");
    expect(button.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
  });

  it("runs onClick from the keyboard", async () => {
    const onClick = vi.fn();
    render(<Button onClick={onClick}>Retry</Button>);

    await userEvent.tab();
    expect(screen.getByRole("button", { name: "Retry" })).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    expect(onClick).toHaveBeenCalledOnce();
  });

  it("does not run onClick while disabled", async () => {
    const onClick = vi.fn();
    render(
      <Button onClick={onClick} disabled>
        Retry
      </Button>,
    );

    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onClick).not.toHaveBeenCalled();
  });
});
