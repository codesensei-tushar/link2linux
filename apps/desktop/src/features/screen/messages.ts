import type { ScreenEnd, ScreenInputProblem, ScreenProblem } from "@/lib/api";

/** Why the screen of `phone` can't be shown, or why a start didn't work. Null for nothing. */
export function problemText(problem: ScreenProblem, phone: string): string | null {
  switch (problem) {
    case "notConnected":
      return `${phone} isn't connected.`;
    case "waiting":
      return null;
    case "unsupported":
      return `The app on ${phone} is too old to share its screen. Update it, then try again.`;
    case "turnedOff":
      return `Screen sharing is off on ${phone}. Turn on Screen in the link2linux app on your phone.`;
    case "unavailable":
      return `${phone} can't share its screen right now.`;
    case "busy":
      return `${phone} is sharing its screen with another computer.`;
    case "cannotAsk":
      return `${phone} couldn't ask you to share its screen. Allow notifications for link2linux on your phone, then try again.`;
    case "noCodec":
      return `${phone} can't send its screen in a format this computer can show.`;
    case "failed":
      return `${phone} didn't answer. Try again.`;
  }
}

/** Why the last stream from `phone` ended. */
export function endText(end: ScreenEnd, phone: string): string {
  switch (end) {
    case "declined":
      return `Sharing was declined on ${phone}.`;
    case "timedOut":
      return `Nobody answered on ${phone} in time.`;
    case "stoppedOnPhone":
      return `Sharing was stopped on ${phone}.`;
    case "locked":
      return `${phone} locked, which ends screen sharing.`;
    case "turnedOff":
      return `Screen sharing was turned off on ${phone}.`;
    case "interrupted":
      return `${phone} stopped sharing its screen.`;
    case "disconnected":
      return `The connection with ${phone} ended.`;
  }
}

/** Why `phone` didn't carry out the user's last input. */
export function inputProblemText(problem: ScreenInputProblem, phone: string): string {
  switch (problem) {
    case "controlOff":
      return `Control is off on ${phone}.`;
    case "screenChanged":
      return `The screen changed before that reached ${phone}. Try again.`;
    case "noTextField":
      return "Tap a text field on the phone's screen first, then type.";
    case "failed":
      return `${phone} didn't take that. Try again.`;
  }
}
