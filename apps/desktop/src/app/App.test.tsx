import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { describe, expect, it } from "vitest";

import { createFakeApi, testDevice, type FakeApi } from "@/test/fake-api";

import { AppProviders } from "./providers";
import { routes } from "./routes";

function renderApp({ api = createFakeApi(), path = "/" }: { api?: FakeApi; path?: string } = {}) {
  const router = createMemoryRouter(routes, { initialEntries: [path] });
  render(
    <AppProviders api={api}>
      <RouterProvider router={router} />
    </AppProviders>,
  );
  return { api, router };
}

describe("app shell", () => {
  it("opens on pairing, with every feature in the sidebar", async () => {
    renderApp();

    const nav = screen.getByRole("navigation", { name: "Features" });
    const links = within(nav).getAllByRole("link");
    expect(links.map((link) => link.textContent)).toEqual([
      "Pairing",
      "Notifications",
      "Calls",
      "Photos",
      "Files",
      "Clipboard",
      "Screen",
    ]);
    expect(within(nav).getByRole("link", { name: "Pairing" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(await screen.findByRole("region", { name: "No phone paired yet" })).toBeVisible();
    expect(document.title).toBe("Pairing · link2linux");
  });

  it("navigates between features from the sidebar", async () => {
    const user = userEvent.setup();
    const { router } = renderApp();

    await user.click(screen.getByRole("link", { name: "Photos" }));

    expect(router.state.location.pathname).toBe("/photos");
    expect(screen.getByRole("heading", { level: 1, name: "Photos" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Photos" })).toHaveAttribute("aria-current", "page");
  });

  it("opens the settings from the sidebar", async () => {
    const user = userEvent.setup();
    const { router } = renderApp();

    const nav = screen.getByRole("navigation", { name: "App" });
    await user.click(within(nav).getByRole("link", { name: "Settings" }));

    expect(router.state.location.pathname).toBe("/settings");
    expect(screen.getByRole("heading", { level: 1, name: "Settings" })).toBeInTheDocument();
    expect(document.title).toBe("Settings · link2linux");
  });

  it("moves focus through the sidebar with the arrow keys", async () => {
    const user = userEvent.setup();
    renderApp({ path: "/calls" });

    screen.getByRole("link", { name: "Calls" }).focus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("link", { name: "Photos" })).toHaveFocus();
    await user.keyboard("{ArrowUp}{ArrowUp}");
    expect(screen.getByRole("link", { name: "Notifications" })).toHaveFocus();
    await user.keyboard("{End}");
    expect(screen.getByRole("link", { name: "Screen" })).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("link", { name: "Pairing" })).toHaveFocus();
  });

  it("offers a skip link that moves focus to the page", async () => {
    const user = userEvent.setup();
    renderApp();

    await user.tab();
    const skip = screen.getByRole("link", { name: "Skip to content" });
    expect(skip).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(screen.getByRole("main")).toHaveFocus();
  });

  it("shows the connection state and app version in the status area", async () => {
    const api = createFakeApi();
    api.listDevices.mockResolvedValue([testDevice({ name: "Pixel", status: "connected" })]);
    renderApp({ api });

    const status = screen.getByRole("region", { name: "Connection" });
    expect(await within(status).findByText("Connected")).toBeInTheDocument();
    expect(within(status).getByText("Pixel")).toBeInTheDocument();
    expect(await within(status).findByText("link2linux 0.1.0")).toBeInTheDocument();
  });

  it("sends unknown routes to pairing", () => {
    const { router } = renderApp({ path: "/nowhere" });
    expect(router.state.location.pathname).toBe("/pairing");
  });
});
