// UI-side model of the notifications feature. It mirrors
// proto/link2linux/v1/notifications.proto in camelCase. The real types will
// come from Rust through tauri-specta; keep the names close so they can be
// swapped for the generated ones.

/** A reference to an image the phone shared, by the SHA-256 of its bytes. */
export interface ImageRef {
  sha256: string;
}

export type NotificationCategory =
  | "unspecified"
  | "message"
  | "email"
  | "call"
  | "missedCall"
  | "event"
  | "reminder"
  | "alarm"
  | "progress"
  | "media"
  | "social"
  | "promotion"
  | "navigation"
  | "system";

export type NotificationImportance = "unspecified" | "min" | "low" | "default" | "high";

/** Whether the text fields hold the real content. */
export type NotificationContent =
  "full" | "redactedBySystem" | "redactedAppLocked" | "hiddenByUser" | "oneTimeCode" | "withheld";

export type NotificationVisibility = "unspecified" | "public" | "private" | "secret";

export type NotificationActionKind =
  | "unspecified"
  | "reply"
  | "markRead"
  | "delete"
  | "archive"
  | "mute"
  | "answerCall"
  | "declineCall"
  | "hangUp";

export interface NotificationActionInfo {
  /** Identifies the action within its notification. Opaque. */
  id: string;
  label: string;
  kind: NotificationActionKind;
  /** The action takes free-form reply text. */
  acceptsReply: boolean;
  /** Ready-made replies the app offers. */
  replyChoices: string[];
  /** Android runs this action only on an unlocked phone. */
  requiresUnlock: boolean;
}

export interface ConversationMessageInfo {
  /** Empty for messages the phone's user sent. */
  sender: string;
  text: string;
  sentUnixMs: number;
  outgoing: boolean;
}

export interface NotificationProgressInfo {
  current: number;
  max: number;
  indeterminate: boolean;
}

/** A notification as the phone shares it and the desktop mirrors it. */
export interface MirroredNotification {
  /** Opaque and stable for the notification's lifetime. */
  key: string;
  appName: string;
  appIcon: ImageRef | null;
  whenUnixMs: number;
  title: string;
  text: string;
  subText: string;
  category: NotificationCategory;
  importance: NotificationImportance;
  ongoing: boolean;
  /** Dismissal on the phone is possible. */
  dismissible: boolean;
  /** Notifications with the same group are shown stacked. Empty for none. */
  group: string;
  actions: NotificationActionInfo[];
  /** Conversations only: latest messages, oldest first. */
  messages: ConversationMessageInfo[];
  conversationTitle: string;
  isGroupConversation: boolean;
  progress: NotificationProgressInfo | null;
  largeIcon: ImageRef | null;
  picture: ImageRef | null;
  content: NotificationContent;
  visibility: NotificationVisibility;
}

/** A change to the mirrored list, as pushed by the backend. */
export type NotificationEvent =
  | {
      type: "posted";
      /** New, or an update that replaces the earlier version in place. */
      notification: MirroredNotification;
      /** The phone asks to alert the user. False for sync and quiet updates. */
      alert: boolean;
    }
  | { type: "removed"; key: string }
  /** A full snapshot replaces the whole list. */
  | { type: "reset"; notifications: MirroredNotification[] };
