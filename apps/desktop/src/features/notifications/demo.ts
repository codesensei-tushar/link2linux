import type { MirroredNotification, NotificationActionInfo } from "./types";

const MINUTE = 60_000;

const base: Omit<MirroredNotification, "key" | "appName" | "whenUnixMs" | "title" | "text"> = {
  appIcon: null,
  subText: "",
  category: "unspecified",
  importance: "default",
  ongoing: false,
  dismissible: true,
  group: "",
  actions: [],
  messages: [],
  conversationTitle: "",
  isGroupConversation: false,
  progress: null,
  largeIcon: null,
  picture: null,
  content: "full",
  visibility: "private",
};

function action(overrides: Partial<NotificationActionInfo> & { id: string; label: string }) {
  return {
    kind: "unspecified",
    acceptsReply: false,
    replyChoices: [],
    requiresUnlock: false,
    ...overrides,
  } satisfies NotificationActionInfo;
}

/** A small, built-in set for previewing the page in a browser without a phone. */
export function demoNotifications(now: number): MirroredNotification[] {
  return [
    {
      ...base,
      key: "demo-chat",
      appName: "Chat",
      whenUnixMs: now - 2 * MINUTE,
      title: "Weekend plans",
      text: "Bring the tent?",
      category: "message",
      importance: "high",
      group: "chat",
      conversationTitle: "Weekend plans",
      isGroupConversation: true,
      messages: [
        {
          sender: "Ada",
          text: "Are we still on for Saturday?",
          sentUnixMs: now - 9 * MINUTE,
          outgoing: false,
        },
        { sender: "", text: "Yes, leaving at nine.", sentUnixMs: now - 6 * MINUTE, outgoing: true },
        { sender: "Grace", text: "Bring the tent?", sentUnixMs: now - 2 * MINUTE, outgoing: false },
      ],
      actions: [
        action({
          id: "reply",
          label: "Reply",
          kind: "reply",
          acceptsReply: true,
          replyChoices: ["Yes", "On my way", "Can't talk now"],
        }),
        action({ id: "mark-read", label: "Mark as read", kind: "markRead" }),
      ],
    },
    {
      ...base,
      key: "demo-calendar",
      appName: "Calendar",
      whenUnixMs: now - 15 * MINUTE,
      title: "Team sync in 10 minutes",
      text: "Room 4, with the design group",
      subText: "work@example.org",
      category: "reminder",
      actions: [action({ id: "snooze", label: "Snooze" })],
    },
    {
      ...base,
      key: "demo-download",
      appName: "Files",
      whenUnixMs: now - 40 * MINUTE,
      title: "Downloading holiday-photos.zip",
      text: "62% of 1.2 GB",
      category: "progress",
      importance: "low",
      progress: { current: 62, max: 100, indeterminate: false },
    },
    {
      ...base,
      key: "demo-otp",
      appName: "Bank",
      whenUnixMs: now - 3 * MINUTE,
      title: "",
      text: "",
      content: "oneTimeCode",
      visibility: "secret",
    },
    {
      ...base,
      key: "demo-hidden",
      appName: "Private notes",
      whenUnixMs: now - 55 * MINUTE,
      title: "",
      text: "",
      content: "hiddenByUser",
    },
    {
      ...base,
      key: "demo-music",
      appName: "Music",
      whenUnixMs: now - 70 * MINUTE,
      title: "Now playing",
      text: "Evening Radio",
      category: "media",
      importance: "low",
      ongoing: true,
      dismissible: false,
    },
  ];
}
