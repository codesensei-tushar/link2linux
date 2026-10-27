import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { DesktopApiContext } from "@/lib/api";
import {
  createFakeApi,
  testDevice,
  testPhoneStatus,
  testRing,
  type FakeApi,
} from "@/test/fake-api";

import { PhoneSummary } from "./PhoneSummary";

const pixel = testDevice({ id: "phone-1", name: "Pixel", status: "connected" });

function renderSummary(api: FakeApi = createFakeApi()) {
  const view = render(
    <DesktopApiContext value={api}>
      <PhoneSummary device={pixel} />
    </DesktopApiContext>,
  );
  return { api, ...view };
}

describe("PhoneSummary", () => {
  it("shows the phone's status and follows its changes", async () => {
    const api = createFakeApi();
    api.getPhoneStatus.mockResolvedValue(testPhoneStatus());
    renderSummary(api);

    const list = await screen.findByRole("list", { name: "Phone status" });
    expect(api.getPhoneStatus).toHaveBeenCalledWith("phone-1");
    expect(within(list).getByRole("listitem", { name: "Battery 76%" })).toHaveTextContent("76%");
    expect(
      within(list).getByRole("listitem", { name: "Mobile signal 3 bars of 4, 5G, Carrier" }),
    ).toHaveTextContent("5G");
    expect(within(list).getByRole("listitem", { name: "Wi-Fi 4 bars of 4" })).toBeVisible();

    api.emitPhoneStatus(
      "phone-1",
      testPhoneStatus({
        battery: { percent: 20, pluggedIn: true, powerSaver: false },
        doNotDisturb: true,
      }),
    );
    expect(screen.getByRole("listitem", { name: "Battery 20%, plugged in" })).toBeVisible();
    expect(screen.getByRole("listitem", { name: "Do Not Disturb on" })).toBeVisible();

    // Another phone's status is not this one's.
    api.emitPhoneStatus("phone-2", { state: "turnedOff" });
    expect(screen.getByRole("list", { name: "Phone status" })).toBeVisible();

    api.emitPhoneStatus("phone-1", { state: "disconnected" });
    expect(screen.queryByRole("list", { name: "Phone status" })).not.toBeInTheDocument();
    // The summary is for a connected phone, so it says nothing until the
    // session has published its first status.
    expect(screen.queryByText("Not connected")).not.toBeInTheDocument();

    api.emitPhoneStatus("phone-1", { state: "hidden" });
    expect(screen.queryByText("Not connected")).not.toBeInTheDocument();
  });

  it("rings the phone and stops it", async () => {
    const user = userEvent.setup();
    const { api } = renderSummary();

    await user.click(await screen.findByRole("button", { name: "Ring phone" }));
    expect(api.ringPhone).toHaveBeenCalledWith("phone-1");

    api.emitRing("phone-1", testRing({ phase: "starting" }));
    expect(screen.getByRole("button", { name: "Ring phone" })).toBeDisabled();
    api.emitRing("phone-1", testRing({ phase: "ringing" }));
    expect(screen.getByText("Pixel is ringing")).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Stop ringing" }));
    expect(api.stopRinging).toHaveBeenCalledWith("phone-1");
    api.emitRing("phone-1", testRing({ phase: "stopping" }));
    expect(screen.getByRole("button", { name: "Stop ringing" })).toBeDisabled();
    api.emitRing("phone-1", testRing());
    expect(screen.getByRole("button", { name: "Ring phone" })).toBeEnabled();
    expect(screen.queryByText("Pixel is ringing")).not.toBeInTheDocument();
  });

  it("says why the phone stopped ringing by itself", async () => {
    const { api } = renderSummary();
    await screen.findByRole("button", { name: "Ring phone" });

    api.emitRing("phone-1", testRing({ ended: "stoppedOnPhone" }));
    expect(screen.getByText("Stopped on the phone.")).toBeVisible();
    api.emitRing("phone-1", testRing({ ended: "interrupted" }));
    expect(screen.getByText(/because a call started/)).toBeVisible();
  });

  it("says why the phone didn't ring", async () => {
    const { api } = renderSummary();
    await screen.findByRole("button", { name: "Ring phone" });

    api.emitRing("phone-1", testRing({ problem: "unavailable" }));
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Your phone can't ring right now, for example during a call.",
    );
    api.emitRing("phone-1", testRing({ problem: "failed" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Your phone didn't answer. Try again.");
  });

  it("says nothing of ringing until the phone has reported", async () => {
    const api = createFakeApi();
    api.getRingStatus.mockResolvedValue(testRing({ blocked: "notConnected" }));
    renderSummary(api);

    expect(await screen.findByRole("button", { name: "Ring phone" })).toBeDisabled();
    expect(screen.queryByText("Your phone isn't connected.")).not.toBeInTheDocument();

    api.emitRing("phone-1", testRing({ blocked: "waiting" }));
    expect(screen.getByRole("button", { name: "Ring phone" })).toBeDisabled();
    expect(screen.queryByText(/phone/i, { selector: ".ring__note" })).not.toBeInTheDocument();

    api.emitRing("phone-1", testRing());
    expect(screen.getByRole("button", { name: "Ring phone" })).toBeEnabled();
  });

  it("keeps the button off while the phone has ringing turned off", async () => {
    const api = createFakeApi();
    api.getRingStatus.mockResolvedValue(testRing({ blocked: "turnedOff" }));
    renderSummary(api);

    expect(await screen.findByRole("button", { name: "Ring phone" })).toBeDisabled();
    expect(screen.getByText("Ringing is turned off on your phone.")).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    api.emitRing("phone-1", testRing());
    expect(screen.getByRole("button", { name: "Ring phone" })).toBeEnabled();
    expect(screen.queryByText("Ringing is turned off on your phone.")).not.toBeInTheDocument();
  });

  it("shows a refused command inline, and the next try clears it", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.ringPhone.mockRejectedValueOnce(new Error("That phone is no longer available."));
    renderSummary(api);

    await user.click(await screen.findByRole("button", { name: "Ring phone" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "That phone is no longer available.",
    );
    await user.click(screen.getByRole("button", { name: "Ring phone" }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("stops listening once it is gone", async () => {
    const { api, unmount } = renderSummary();
    await screen.findByRole("button", { name: "Ring phone" });
    expect(api.listenerCount()).toBe(2);
    unmount();
    expect(api.listenerCount()).toBe(0);
  });
});
