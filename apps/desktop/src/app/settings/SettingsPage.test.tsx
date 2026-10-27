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
    expect(api.setSettings).toHaveBeenCalledWith({
      showNotificationText: true,
      shareClipboard: true,
      sharePasswords: false,
    });
  });

  it("shares the clipboard but not passwords until the user says otherwise", async () => {
    const user = userEvent.setup();
    const api = renderPage();

    const sharing = await screen.findByRole("switch", {
      name: "Share the clipboard with your phones",
    });
    const passwords = screen.getByRole("switch", { name: "Share passwords" });
    expect(sharing).toBeChecked();
    expect(passwords).not.toBeChecked();
    expect(passwords).toHaveAccessibleDescription(/they stay on this computer/);

    await user.click(passwords);
    expect(api.setSettings).toHaveBeenLastCalledWith({
      showNotificationText: false,
      shareClipboard: true,
      sharePasswords: true,
    });

    // Passwords cannot be shared while the clipboard is not.
    await user.click(sharing);
    expect(api.setSettings).toHaveBeenLastCalledWith({
      showNotificationText: false,
      shareClipboard: false,
      sharePasswords: true,
    });
    expect(passwords).toBeDisabled();
  });

  it("treats settings saved before the clipboard was shared as the defaults", async () => {
    const api = createFakeApi();
    api.getSettings.mockResolvedValue({ showNotificationText: true });
    renderPage(api);

    expect(
      await screen.findByRole("switch", { name: "Share the clipboard with your phones" }),
    ).toBeChecked();
    expect(screen.getByRole("switch", { name: "Share passwords" })).not.toBeChecked();
  });

  it("sends and receives files until the user turns them off", async () => {
    const user = userEvent.setup();
    const api = renderPage();

    const files = await screen.findByRole("switch", { name: "Send and receive files" });
    expect(files).toBeChecked();
    expect(files).toHaveAccessibleDescription(/saved in your Downloads folder/);

    await user.click(files);
    expect(files).not.toBeChecked();
    expect(api.setSettings).toHaveBeenLastCalledWith(
      expect.objectContaining({ shareFiles: false }),
    );
  });

  it("shows the phone's status until the user hides it", async () => {
    const user = userEvent.setup();
    const api = renderPage();

    const status = await screen.findByRole("switch", { name: "Show the phone's status" });
    expect(status).toBeChecked();
    expect(status).toHaveAccessibleDescription(/battery, signal and sound mode/);

    await user.click(status);
    expect(status).not.toBeChecked();
    expect(api.setSettings).toHaveBeenLastCalledWith(
      expect.objectContaining({ showPhoneStatus: false }),
    );
  });

  it("takes calls on this computer until the user turns them off", async () => {
    const user = userEvent.setup();
    const api = renderPage();

    const calls = await screen.findByRole("switch", { name: "Use this computer for calls" });
    expect(calls).toBeChecked();
    expect(calls).toHaveAccessibleDescription(/your phones keep their calls/);

    await user.click(calls);
    expect(calls).not.toBeChecked();
    expect(api.setSettings).toHaveBeenLastCalledWith(
      expect.objectContaining({ useForCalls: false }),
    );
  });

  it("says when a change could not be saved", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.setSettings.mockRejectedValue(new Error("The setting couldn't be saved."));
    renderPage(api);

    await user.click(
      await screen.findByRole("switch", { name: "Show notification text in pop-ups" }),
    );
    expect(await screen.findByRole("note")).toHaveTextContent("The setting couldn't be saved.");
  });

  it("offers to try again when the settings don't load", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.getSettings.mockRejectedValueOnce(new Error("No backend."));
    renderPage(api);

    expect(await screen.findByText("Couldn't load the settings")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(
      await screen.findByRole("switch", { name: "Show notification text in pop-ups" }),
    ).not.toBeChecked();
  });
});
