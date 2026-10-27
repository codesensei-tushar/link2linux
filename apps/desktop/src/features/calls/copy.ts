import type { CallProblem, CallsDesktop, CallStateView, PairingRequestState } from "@/lib/api";

/** Why this computer can't take calls, and what to do about it. */
export const desktopCopy: Record<
  Exclude<CallsDesktop, "ready">,
  { title: string; body: string }
> = {
  checking: {
    title: "Checking Bluetooth…",
    body: "link2linux is looking for this computer's Bluetooth and call support.",
  },
  disabled: {
    title: "Calls are off on this computer",
    body: "Your phones keep their calls to themselves.",
  },
  unsupported: {
    title: "Calls aren't available here",
    body: "This version of link2linux can't take calls on this system.",
  },
  noAdapter: {
    title: "This computer has no Bluetooth",
    body: "Calls reach this computer over Bluetooth. Plug in a Bluetooth adapter to take them here.",
  },
  bluetoothOff: {
    title: "Bluetooth is off",
    body: "Turn on Bluetooth in your system settings to take calls on this computer.",
  },
  noCallControl: {
    title: "Your audio system can't take calls yet",
    body: "Taking calls needs PipeWire 1.4 or later with its own Bluetooth support. Update your system, then open this page again.",
  },
};

/** What a call is doing, for its card. Active calls show their timer instead. */
export const callStateLabel: Record<CallStateView, string> = {
  incoming: "Incoming call",
  waiting: "Call waiting",
  dialing: "Calling…",
  alerting: "Ringing…",
  active: "On a call",
  held: "On hold",
};

/** What became of asking the phone to pair; `name` is this computer's Bluetooth name. */
export function requestText(state: PairingRequestState, phone: string, name: string): string {
  switch (state) {
    case "notAsked":
      return `${phone} isn't connected to link2linux right now, so pair from its Bluetooth settings.`;
    case "waiting":
      return `Asking ${phone} to pair…`;
    case "asked":
      return `${phone} shows a notification to pair. Tap it, or pair from its Bluetooth settings.`;
    case "unavailable":
      return `${phone} can't pair over Bluetooth right now. Check that its Bluetooth is on.`;
    case "notAllowed":
      return `${phone} can't show a prompt to pair, maybe because its notifications are blocked. Open Bluetooth settings on ${phone} and pick “${name}”.`;
    case "failed":
      return `Couldn't reach ${phone}, so pair from its Bluetooth settings.`;
  }
}

export function problemText(problem: CallProblem, phone: string): string {
  switch (problem) {
    case "connectFailed":
      return `Couldn't connect to ${phone} over Bluetooth. Check that its Bluetooth is on and it's nearby.`;
    case "pairingFailed":
      return "Couldn't start Bluetooth pairing. Check that Bluetooth is on, then try again.";
    case "pairingTimedOut":
      return `${phone} didn't pair in time. Try again.`;
    case "commandFailed":
      return `${phone} didn't do that. Try again.`;
  }
}
