import { describe, expect, it, vi } from "vitest";

import type { CommandError } from "./bindings";

const refused = (error: CommandError) => Promise.resolve({ status: "error" as const, error });

vi.mock("./bindings", () => ({
  commands: {
    sendDroppedFiles: vi.fn(() =>
      refused({
        code: "folderNotSendable",
        message: "Folders can't be sent yet. Send the files in them instead.",
      }),
    ),
    pickFilesToSend: vi.fn(() =>
      refused({ code: "filesOff", message: "Turn on Files in Settings to send files." }),
    ),
  },
  events: {},
}));

const { tauriApi } = await import("./api");

describe("tauriApi", () => {
  it("passes the backend's own words on when it refuses to send files", async () => {
    await expect(tauriApi.sendDroppedFiles(1, "phone-1")).rejects.toThrow(
      "Folders can't be sent yet. Send the files in them instead.",
    );
    await expect(tauriApi.pickFilesToSend("phone-1")).rejects.toThrow(
      "Turn on Files in Settings to send files.",
    );
  });
});
