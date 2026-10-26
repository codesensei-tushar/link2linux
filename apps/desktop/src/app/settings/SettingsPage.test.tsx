import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { DesktopApiContext } from "@/lib/api";
import { createFakeApi } from "@/test/fake-api";

import { SettingsPage } from "./SettingsPage";

function renderPage(api = createFakeApi()) {
  render(
    <DesktopApiContext value={api}>
      <SettingsPage />
    </DesktopApiContext>,
  );
  return api;
}

describe("SettingsPage", () => {
  it("keeps notification text out of pop-ups until the user turns it on", async () => {
    const user = userEvent.setup();
    const api = renderPage();

    const toggle = await screen.findByRole("switch", { name: "Show notification text in pop-ups" });
    expect(toggle).not.toBeChecked();
    expect(toggle).toHaveAccessibleDescription(/other apps on this computer can read them/);

    await user.click(toggle);
    expect(toggle).toBeChecked();
    expect(api.setSettings).toHaveBeenCalledWith({ showNotificationText: true });
  });

  it("says when a change could not be saved", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.setSettings.mockRejectedValue(new Error("The setting couldn't be saved."));
    renderPage(api);

    await user.click(await screen.findByRole("switch"));
    expect(await screen.findByRole("note")).toHaveTextContent("The setting couldn't be saved.");
  });

  it("offers to try again when the settings don't load", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.getSettings.mockRejectedValueOnce(new Error("No backend."));
    renderPage(api);

    expect(await screen.findByText("Couldn't load the settings")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("switch")).not.toBeChecked();
  });
});
