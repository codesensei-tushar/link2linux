import {
  Battery,
  BatteryCharging,
  BatteryFull,
  BatteryLow,
  BatteryMedium,
  BatteryWarning,
  BellOff,
  Leaf,
  Moon,
  Signal,
  SignalHigh,
  SignalLow,
  SignalMedium,
  SignalZero,
  Vibrate,
  Wifi,
  WifiHigh,
  WifiLow,
  WifiZero,
  type LucideIcon,
} from "lucide-react";

import type {
  PhoneBattery,
  PhoneCellular,
  PhoneStatus,
  PhoneWifi,
  RingEnd,
  RingProblem,
} from "@/lib/api";

/** A phone status that holds values. */
export type CurrentStatus = Extract<PhoneStatus, { state: "current" }>;

/** One part of the phone's status, as the sidebar shows it. */
export interface StatusItem {
  key: string;
  icon: LucideIcon;
  /** Short text next to the icon, or empty. */
  text: string;
  /** What the item says in words, for screen readers and the tooltip. */
  label: string;
}

const generations = { noService: "No service", "2g": "2G", "3g": "3G", "4g": "4G", "5g": "5G" };

function batteryIcon({ percent, pluggedIn }: PhoneBattery): LucideIcon {
  if (pluggedIn) return BatteryCharging;
  if (percent === null) return Battery;
  if (percent <= 10) return BatteryWarning;
  if (percent <= 30) return BatteryLow;
  if (percent <= 70) return BatteryMedium;
  return BatteryFull;
}

function battery(battery: PhoneBattery): StatusItem {
  const level =
    battery.percent === null ? "Battery level unknown" : `Battery ${String(battery.percent)}%`;
  return {
    key: "battery",
    icon: batteryIcon(battery),
    text: battery.percent === null ? "" : `${String(battery.percent)}%`,
    label: battery.pluggedIn ? `${level}, plugged in` : level,
  };
}

/** Bars out of four, in words. */
function bars(level: number): string {
  return level === 1 ? "1 bar of 4" : `${String(level)} bars of 4`;
}

const signalIcons = [SignalZero, SignalLow, SignalMedium, SignalHigh, Signal];

function cellular({ level, network, operatorName }: PhoneCellular): StatusItem {
  if (network === "noService") {
    return { key: "cellular", icon: SignalZero, text: "No service", label: "No mobile service" };
  }
  const parts = [level === null ? "Mobile signal" : `Mobile signal ${bars(level)}`];
  if (network) parts.push(generations[network]);
  if (operatorName) parts.push(operatorName);
  return {
    key: "cellular",
    icon: level === null ? Signal : (signalIcons[level] ?? Signal),
    text: network ? generations[network] : "",
    label: parts.join(", "),
  };
}

const wifiIcons = [WifiZero, WifiLow, WifiLow, WifiHigh, Wifi];

function wifi({ level }: PhoneWifi): StatusItem {
  return {
    key: "wifi",
    icon: level === null ? Wifi : (wifiIcons[level] ?? Wifi),
    text: "",
    label: level === null ? "Wi-Fi" : `Wi-Fi ${bars(level)}`,
  };
}

/** The parts of a current status, in the order the sidebar shows them. Unknown parts are left out. */
export function statusItems(status: CurrentStatus): StatusItem[] {
  const items: StatusItem[] = [];
  if (status.battery) {
    items.push(battery(status.battery));
    if (status.battery.powerSaver) {
      items.push({ key: "saver", icon: Leaf, text: "", label: "Battery saver on" });
    }
  }
  if (status.cellular) items.push(cellular(status.cellular));
  if (status.wifi) items.push(wifi(status.wifi));
  if (status.ringer === "vibrate") {
    items.push({ key: "ringer", icon: Vibrate, text: "", label: "Ringer set to vibrate" });
  } else if (status.ringer === "silent") {
    items.push({ key: "ringer", icon: BellOff, text: "", label: "Ringer silenced" });
  }
  if (status.doNotDisturb) {
    items.push({ key: "dnd", icon: Moon, text: "", label: "Do Not Disturb on" });
  }
  return items;
}

/** What to say instead of a status that holds no values, or null to say nothing. */
export function statusNote(status: Exclude<PhoneStatus, CurrentStatus>): string | null {
  switch (status.state) {
    case "disconnected":
      return "Not connected";
    case "turnedOff":
      return "Status is turned off on the phone";
    case "unavailable":
      return "Status unavailable";
    case "unsupported":
      return "Update link2linux on the phone to see its status";
    case "waiting":
    case "hidden":
      return null;
  }
}

/** Why the phone can't ring, or didn't do what the user asked. */
export function ringProblemText(problem: RingProblem): string {
  switch (problem) {
    case "turnedOff":
      return "Ringing is turned off on your phone.";
    case "unavailable":
      return "Your phone can't ring right now, for example during a call.";
    case "unsupported":
      return "Update link2linux on your phone to ring it from here.";
    case "notConnected":
      return "Your phone isn't connected.";
    case "failed":
      return "Your phone didn't answer. Try again.";
  }
}

/** Why the phone stopped ringing without this computer asking. */
export function ringEndText(ended: RingEnd): string {
  switch (ended) {
    case "stoppedOnPhone":
      return "Stopped on the phone.";
    case "stoppedFromDesktop":
      return "Stopped from another computer.";
    case "timedOut":
      return "Your phone rang for a full minute.";
    case "interrupted":
      return "Your phone stopped ringing, for example because a call started.";
  }
}
