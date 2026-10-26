import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { DesktopApiContext } from "@/lib/api";
import { DevicesProvider } from "@/features/pairing/DevicesProvider";
import { createFakeApi, testDevice, testNotification, type FakeApi } from "@/test/fake-api";

import { NotificationsPage } from "./NotificationsPage";
import type { MirroredNotification, NotificationActionInfo } from "./types";

function renderPage(
  notifications: MirroredNotification[] = [],
  api: FakeApi = createFakeApi(),
  connected = true,
) {
  api.listDevices.mockResolvedValue([
    testDevice({ id: "phone-1", status: connected ? "connected" : "disconnected" }),
  ]);
  api.listNotifications.mockResolvedValue(notifications);
  const view = render(
    <DesktopApiContext value={api}>
      <DevicesProvider>
        <NotificationsPage />
      </DevicesProvider>
    </DesktopApiContext>,
  );
  return Object.assign(api, { view });
}

const now = Date.now();

describe("NotificationsPage list", () => {
  it("lists notifications newest first with app, time, title and text", async () => {
    renderPage([
      testNotification({
        key: "old",
        appName: "Mail",
        title: "Invoice",
        text: "Due Friday",
        whenUnixMs: now - 3_600_000,
      }),
      testNotification({
        key: "new",
        appName: "Chat",
        title: "Ada",
        text: "Lunch?",
        whenUnixMs: now - 120_000,
      }),
    ]);

    const items = within(await screen.findByRole("list", { name: "Notifications" })).getAllByRole(
      "article",
    );
    expect(items[0]).toHaveTextContent(/Chat.*2 min ago.*Ada.*Lunch\?/);
    expect(items[0]).toHaveTextContent("2 min ago");
    expect(items[1]).toHaveTextContent("Mail");
    expect(items[1]).toHaveTextContent("1 h ago");
    expect(items[1]).toHaveTextContent("Due Friday");
  });

  it("stacks notifications of the same group", async () => {
    renderPage([
      testNotification({ key: "a", group: "chat", title: "One", whenUnixMs: now - 5000 }),
      testNotification({ key: "b", group: "chat", title: "Two", whenUnixMs: now - 1000 }),
      testNotification({ key: "c", appName: "Mail", title: "Other", whenUnixMs: now - 3000 }),
    ]);

    const group = await screen.findByRole("region", { name: "Chat, 2 notifications" });
    const titles = within(group)
      .getAllByRole("article")
      .map((a) => a.getAttribute("aria-label"));
    expect(titles).toEqual(["Chat: Two", "Chat: One"]);
    expect(screen.getByRole("article", { name: "Mail: Other" })).toBeInTheDocument();
  });

  it("shows a conversation's recent messages and progress", async () => {
    renderPage([
      testNotification({
        key: "c",
        title: "Weekend",
        messages: [
          { sender: "Ada", text: "Saturday?", sentUnixMs: now - 2000, outgoing: false },
          { sender: "", text: "Yes", sentUnixMs: now - 1000, outgoing: true },
        ],
      }),
      testNotification({
        key: "p",
        title: "Download",
        progress: { current: 5, max: 10, indeterminate: false },
        whenUnixMs: now - 9999,
      }),
    ]);

    const messages = await screen.findByRole("list", { name: "Recent messages" });
    expect(messages).toHaveTextContent("Ada Saturday?");
    expect(messages).toHaveTextContent("You Yes");
    expect(screen.getByRole("progressbar", { name: "Download progress" })).toHaveValue(5);
  });

  it("explains when no phone is connected", async () => {
    renderPage([], createFakeApi(), false);
    expect(await screen.findByRole("region", { name: "No phone connected" })).toBeVisible();
  });

  it("explains a loading failure and retries", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.listNotifications.mockRejectedValueOnce(new Error("Phone unreachable"));
    renderPage([testNotification({ title: "Back again" })], api);

    const failure = await screen.findByRole("region", { name: "Couldn't load notifications" });
    expect(failure).toHaveTextContent("Phone unreachable");

    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("article", { name: "Chat: Back again" })).toBeInTheDocument();
  });

  it("explains when there are no notifications", async () => {
    renderPage([]);
    expect(await screen.findByRole("region", { name: "No notifications" })).toBeVisible();
  });

  it("applies live events and announces alerts", async () => {
    const api = renderPage([
      testNotification({ key: "a", title: "First", whenUnixMs: now - 9000 }),
    ]);
    await screen.findByRole("article", { name: "Chat: First" });

    act(() => {
      api.emitNotification({
        type: "posted",
        alert: true,
        notification: testNotification({ key: "b", title: "Second" }),
      });
    });
    expect(screen.getByRole("article", { name: "Chat: Second" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("New notification from Chat: Second");

    act(() => {
      api.emitNotification({
        type: "posted",
        alert: false,
        notification: testNotification({
          key: "a",
          title: "First, edited",
          whenUnixMs: now - 9000,
        }),
      });
    });
    expect(screen.queryByRole("article", { name: "Chat: First" })).not.toBeInTheDocument();
    expect(screen.getByRole("article", { name: "Chat: First, edited" })).toBeInTheDocument();

    act(() => {
      api.emitNotification({ type: "removed", key: "b" });
    });
    expect(screen.queryByRole("article", { name: "Chat: Second" })).not.toBeInTheDocument();

    act(() => {
      api.emitNotification({
        type: "reset",
        notifications: [testNotification({ key: "z", title: "Fresh" })],
      });
    });
    expect(screen.getAllByRole("article")).toHaveLength(1);
    expect(screen.getByRole("article", { name: "Chat: Fresh" })).toBeInTheDocument();
  });

  it("unsubscribes when the page goes away", async () => {
    const api = renderPage([]);
    await screen.findByRole("region", { name: "No notifications" });
    expect(api.notificationSubscribers()).toBe(1);
    api.view.unmount();
    expect(api.notificationSubscribers()).toBe(0);
  });
});

const replyAction: NotificationActionInfo = {
  id: "reply",
  label: "Reply",
  kind: "reply",
  acceptsReply: true,
  replyChoices: ["Yes", "No"],
  requiresUnlock: false,
};

const archiveAction: NotificationActionInfo = {
  id: "archive",
  label: "Archive",
  kind: "archive",
  acceptsReply: false,
  replyChoices: [],
  requiresUnlock: false,
};

describe("NotificationsPage dismiss", () => {
  it("removes the notification at once and tells the phone", async () => {
    const user = userEvent.setup();
    const api = renderPage([testNotification({ key: "a", title: "Ada" })]);

    await user.click(await screen.findByRole("button", { name: "Dismiss Ada" }));

    expect(api.dismissNotification).toHaveBeenCalledWith("phone-1", "a");
    expect(screen.queryByRole("article")).not.toBeInTheDocument();
    expect(await screen.findByRole("region", { name: "No notifications" })).toBeVisible();
  });

  it("puts the notification back with an error when dismissing fails", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.dismissNotification.mockRejectedValue(new Error("Phone unreachable"));
    renderPage([testNotification({ key: "a", title: "Ada" })], api);

    await user.click(await screen.findByRole("button", { name: "Dismiss Ada" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Phone unreachable");
    expect(screen.getByRole("article", { name: "Chat: Ada" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("offers no dismiss button on a notification that cannot be dismissed", async () => {
    renderPage([testNotification({ title: "Now playing", ongoing: true, dismissible: false })]);

    await screen.findByRole("article", { name: "Chat: Now playing" });
    expect(screen.queryByRole("button", { name: /Dismiss/ })).not.toBeInTheDocument();
  });
});

describe("NotificationsPage actions", () => {
  it("runs a plain action", async () => {
    const user = userEvent.setup();
    const api = renderPage([testNotification({ key: "a", actions: [archiveAction] })]);

    await user.click(await screen.findByRole("button", { name: "Archive" }));

    expect(api.runNotificationAction).toHaveBeenCalledWith("phone-1", "a", "archive", undefined);
  });

  it("reports an action that fails", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.runNotificationAction.mockRejectedValue("Notification not found");
    renderPage([testNotification({ actions: [archiveAction] })], api);

    await user.click(await screen.findByRole("button", { name: "Archive" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Notification not found");
  });

  it("opens an inline reply with focus, and sends it on Enter", async () => {
    const user = userEvent.setup();
    const api = renderPage([testNotification({ key: "a", actions: [replyAction] })]);

    await user.click(await screen.findByRole("button", { name: "Reply" }));
    const field = screen.getByRole("textbox", { name: "Reply text" });
    expect(field).toHaveFocus();

    await user.type(field, "On my way{Enter}");

    expect(api.runNotificationAction).toHaveBeenCalledWith("phone-1", "a", "reply", "On my way");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reply" })).toHaveFocus();
  });

  it("sends a quick reply chip", async () => {
    const user = userEvent.setup();
    const api = renderPage([testNotification({ key: "a", actions: [replyAction] })]);

    await user.click(await screen.findByRole("button", { name: "Reply" }));
    const chips = screen.getByRole("group", { name: "Quick replies" });
    await user.click(within(chips).getByRole("button", { name: "Yes" }));

    expect(api.runNotificationAction).toHaveBeenCalledWith("phone-1", "a", "reply", "Yes");
  });

  it("cancels the reply on Escape and does not send empty text", async () => {
    const user = userEvent.setup();
    const api = renderPage([testNotification({ actions: [replyAction] })]);

    await user.click(await screen.findByRole("button", { name: "Reply" }));
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    await user.keyboard("{Enter}");
    expect(api.runNotificationAction).not.toHaveBeenCalled();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reply" })).toHaveFocus();
  });
});
