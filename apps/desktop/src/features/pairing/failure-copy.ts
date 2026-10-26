import type { PairingFailure } from "@/lib/api";

export interface FailureCopy {
  title: string;
  body: string;
}

/** What to tell the user when pairing did not finish, and what to try next. */
export const failureCopy: Record<PairingFailure, FailureCopy> = {
  rejected: {
    title: "The phone refused the connection",
    body: "Open link2linux on the phone, go to its pairing screen and try again.",
  },
  declinedByPhone: {
    title: "Pairing was declined on the phone",
    body: "Nothing was paired. Start again if you want to connect this phone.",
  },
  verificationFailed: {
    title: "The codes didn't match",
    body: "Nothing was paired. If the screens showed different codes, someone else may be on this network, so check the network before trying again.",
  },
  timedOut: {
    title: "Pairing took too long",
    body: "Make sure the phone's screen is on and link2linux is open, then try again.",
  },
  notExpected: {
    title: "The phone wasn't expecting this computer",
    body: "On the phone, open the pairing screen first, then try again here.",
  },
  unsupported: {
    title: "The apps can't pair with each other",
    body: "Update link2linux on both devices to the latest version and try again.",
  },
  expired: {
    title: "The code expired",
    body: "No phone scanned it in time. Start again to get a new code.",
  },
  phoneNotFound: {
    title: "Couldn't reach the phone",
    body: "Check that both devices are on the same network. You can also type the address the phone shows.",
  },
  connectionLost: {
    title: "The connection dropped",
    body: "Check the network on both devices and try again.",
  },
  tooManyAttempts: {
    title: "Too many tries",
    body: "For safety, pairing with a code stops after three tries. Start again to get fresh codes.",
  },
};
