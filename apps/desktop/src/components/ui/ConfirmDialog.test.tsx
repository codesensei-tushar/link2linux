import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { ConfirmDialog, type ConfirmDialogProps } from "./ConfirmDialog";

function open(props: Partial<ConfirmDialogProps> = {}) {
  const onConfirm = vi.fn();
  const onCancel = vi.fn();
  render(
    <>
      <button type="button">Opener</button>
      <ConfirmDialog
        title="Sure?"
        confirmLabel="Yes"
        onConfirm={onConfirm}
        onCancel={onCancel}
        {...props}
      >
        <p>Details</p>
      </ConfirmDialog>
    </>,
  );
  return { onConfirm, onCancel };
}

describe("ConfirmDialog", () => {
  it("is a named, described modal that starts on the safe answer", () => {
    open();
    const dialog = screen.getByRole("alertdialog", { name: "Sure?" });
    expect(dialog).toHaveAccessibleDescription("Details");
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
  });

  it("keeps Tab and Shift+Tab inside", async () => {
    const user = userEvent.setup();
    open();
    await user.tab();
    expect(screen.getByRole("button", { name: "Yes" })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
    await user.tab({ shift: true });
    expect(screen.getByRole("button", { name: "Yes" })).toHaveFocus();
  });

  it("cancels on Escape and confirms on the button", async () => {
    const user = userEvent.setup();
    const { onConfirm, onCancel } = open();
    await user.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalledOnce();
    await user.click(screen.getByRole("button", { name: "Yes" }));
    expect(onConfirm).toHaveBeenCalledOnce();
  });

  it("disables confirm while busy and shows an error", () => {
    open({ busy: true, error: "It failed." });
    expect(screen.getByRole("button", { name: "Yes" })).toBeDisabled();
    expect(screen.getByRole("alert")).toHaveTextContent("It failed.");
  });
});
