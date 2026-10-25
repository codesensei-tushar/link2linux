import { vi, type Mock } from "vitest";

import type { AppInfo, DesktopApi, Device } from "@/lib/api";

export const testAppInfo: AppInfo = {
  name: "link2linux",
  version: "0.1.0",
  identifier: "dev.link2linux.desktop",
  os: "linux",
  arch: "x86_64",
};

export function testDevice(overrides: Partial<Device> = {}): Device {
  return { id: "phone-1", name: "Test phone", model: null, status: "disconnected", ...overrides };
}

export interface FakeApi extends DesktopApi {
  appInfo: Mock<DesktopApi["appInfo"]>;
  listDevices: Mock<DesktopApi["listDevices"]>;
}

/** A `DesktopApi` backed by spies. Starts with no paired phones. */
export function createFakeApi(): FakeApi {
  return {
    appInfo: vi.fn<DesktopApi["appInfo"]>().mockResolvedValue(testAppInfo),
    listDevices: vi.fn<DesktopApi["listDevices"]>().mockResolvedValue([]),
  };
}
