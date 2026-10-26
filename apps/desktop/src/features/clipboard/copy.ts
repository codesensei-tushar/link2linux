import type { Tone } from "@/components/ui";
import type { ClipboardMode, ClipboardPhoneState, ClipboardProblem } from "@/lib/api";

import { formatBytes } from "./format";

export interface ModeCopy {
  title: string;
  body: string;
}

/** How each way of sharing reads on the page. */
export const modeCopy: Record<ClipboardMode, ModeCopy> = {
  automatic: {
    title: "Shares automatically",
    body: "Text you copy on this computer goes to your phones as you copy it, and text you share from a phone is ready to paste here.",
  },
  whenFocused: {
    title: "Shares when this window is focused or when you click Send",
    body: "Your desktop lets apps use the clipboard only while their window is focused. Text you copy in other apps goes to your phones when you come back to this window, and text from a phone waits until then.",
  },
  unavailable: {
    title: "The clipboard isn't available",
    body: "link2linux can't reach this computer's clipboard, so it can't share it.",
  },
};

/** What each phone state is called, and its color. */
export const phoneStateCopy: Record<ClipboardPhoneState, { label: string; tone: Tone }> = {
  ready: { label: "Sharing", tone: "success" },
  turnedOff: { label: "Off on the phone", tone: "warning" },
  unavailable: { label: "Not available", tone: "neutral" },
  unsupported: { label: "Update the phone app", tone: "neutral" },
  disconnected: { label: "Not connected", tone: "neutral" },
};

/** What to tell the user about a problem, and what to do about it. */
export function problemText(problem: ClipboardProblem): string {
  const phone = problem.phoneName ?? "your phone";
  switch (problem.kind) {
    case "tooLarge": {
      const size = problem.bytes === null ? "" : ` (${formatBytes(problem.bytes)})`;
      return `The text you copied is too long to share${size}. Up to 256 KB can be shared.`;
    }
    case "passwordNotShared":
      return "A password you copied wasn't shared. To share passwords too, turn on Share passwords in Settings.";
    case "noText":
      return "There's no text on the clipboard to send.";
    case "sharingOff":
      return "Clipboard sharing is off on this computer.";
    case "noPhone":
      return "No phone can take the clipboard right now. A phone that connects within a minute still gets it.";
    case "turnedOffOnPhone":
      return `Clipboard sharing is off on ${phone}. Turn it on in the link2linux app on the phone.`;
    case "phoneUnavailable":
      return `${phone} couldn't put the text on its clipboard. Try again in a moment.`;
    case "failed":
      return `The text didn't reach ${phone}.`;
  }
}
