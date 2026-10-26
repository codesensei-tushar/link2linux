import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { DesktopApiContext, type StartupError } from "@/lib/api";
import { createFakeApi, type FakeApi } from "@/test/fake-api";

import { StartupGate } from "./StartupGate";

const keyringLocked: StartupError = {
  message:
    "link2linux couldn't open your keyring, which holds the key your phones know this computer by.",
  hint: "Your keyring is locked or not running. Unlock it, then try again.",
  detail: "identity: the keyring that holds the private key is unavailable: dismissed",
};

function renderGate(api: FakeApi) {
  render(
    <DesktopApiContext value={api}>
      <StartupGate>
        <p>The app</p>
      </StartupGate>
    </DesktopApiContext>,
  );
}

describe("StartupGate", () => {
  it("shows the app when it started", async () => {
    renderGate(createFakeApi());
    expect(await screen.findByText("The app")).toBeVisible();
  });

  it("says what failed and starts the app again on request", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.startupError.mockResolvedValue(keyringLocked);
    renderGate(api);

    expect(
      await screen.findByRole("heading", { level: 1, name: keyringLocked.message }),
    ).toBeVisible();
    expect(screen.getByText(keyringLocked.hint)).toBeVisible();
    expect(screen.getByText(keyringLocked.detail)).toBeInTheDocument();
    expect(screen.queryByText("The app")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(api.restartApp).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "Starting again…" })).toBeDisabled();
  });

  it("lets the user try again when starting again fails", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.startupError.mockResolvedValue(keyringLocked);
    api.restartApp.mockRejectedValue(new Error("Couldn't start again."));
    renderGate(api);

    await user.click(await screen.findByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't start again.");
    expect(screen.getByRole("button", { name: "Try again" })).toBeEnabled();
  });
});
