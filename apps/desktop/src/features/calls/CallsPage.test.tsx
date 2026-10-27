import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { CallsStatus, CallView, PhoneCalls } from "@/lib/api";
import { DesktopApiContext } from "@/lib/api";
import { createFakeApi, testCallsStatus, type FakeApi } from "@/test/fake-api";

import { CallsPage } from "./CallsPage";
import { demoCall } from "./demo";

const pixelDevice = { address: "0C:1A:2B:3C:4D:5E", name: "Pixel 8" };

function phone(overrides: Partial<PhoneCalls> = {}): PhoneCalls {
  return {
    deviceId: "phone-1",
    phoneName: "Pixel",
    link: { state: "connected", device: pixelDevice, canMute: true, muted: false },
    calls: [],
    problem: null,
    ...overrides,
  };
}

function active(overrides: Partial<CallView> = {}): CallView {
  return demoCall({
    state: "active",
    canAnswer: false,
    canDecline: false,
    canHangUp: true,
    canSendTones: true,
    activeSinceUnixMs: Date.now() - 65_000,
    ...overrides,
  });
}

function renderPage(status: Partial<CallsStatus> = {}, api: FakeApi = createFakeApi()) {
  api.getCallsStatus.mockResolvedValue(testCallsStatus(status));
  render(
    <DesktopApiContext value={api}>
      <CallsPage />
    </DesktopApiContext>,
  );
  return api;
}

describe("CallsPage", () => {
  it("says what this computer needs to take calls", async () => {
    renderPage({ desktop: "noCallControl", phones: [phone({ link: { state: "off" } })] });
    expect(
      await screen.findByRole("heading", { name: "Your audio system can't take calls yet" }),
    ).toBeVisible();
    expect(screen.getByText(/PipeWire 1.4 or later/)).toBeVisible();
    expect(screen.queryByRole("group", { name: "Dial pad" })).not.toBeInTheDocument();
  });

  it("explains a computer without Bluetooth", async () => {
    renderPage({ desktop: "noAdapter" });
    expect(
      await screen.findByRole("heading", { name: "This computer has no Bluetooth" }),
    ).toBeVisible();
    expect(screen.getByRole("heading", { name: "No phone paired" })).toBeVisible();
  });

  it("turns calls back on", async () => {
    const user = userEvent.setup();
    const api = renderPage({ desktop: "disabled", enabled: false, phones: [phone()] });
    await user.click(await screen.findByRole("button", { name: "Turn on calls" }));
    expect(api.setSettings).toHaveBeenCalledWith(expect.objectContaining({ useForCalls: true }));
  });

  it("pairs a phone that isn't paired over Bluetooth", async () => {
    const user = userEvent.setup();
    const api = renderPage({ phones: [phone({ link: { state: "notPaired" } })] });
    expect(await screen.findByRole("heading", { name: "Pair Pixel over Bluetooth" })).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Pair over Bluetooth" }));
    expect(api.startBluetoothPairing).toHaveBeenCalledWith("phone-1");
  });

  it("lets the user pick which paired device is the phone", async () => {
    const user = userEvent.setup();
    const api = renderPage({
      phones: [
        phone({
          link: {
            state: "choose",
            candidates: [
              { address: "00:11:22:33:44:55", name: "Galaxy" },
              { address: "66:77:88:99:AA:BB", name: "" },
            ],
          },
        }),
      ],
    });
    expect(
      await screen.findByRole("heading", { name: "Which Bluetooth device is Pixel?" }),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "66:77:88:99:AA:BB, 66:77:88:99:AA:BB" }));
    expect(api.chooseBluetoothDevice).toHaveBeenCalledWith("phone-1", "66:77:88:99:AA:BB");
    await user.click(screen.getByRole("button", { name: "It isn't listed: pair it" }));
    expect(api.startBluetoothPairing).toHaveBeenCalledWith("phone-1");
  });

  it("asks before taking the only paired device that could be the phone", async () => {
    renderPage({
      phones: [
        phone({
          link: {
            state: "choose",
            candidates: [{ address: "00:11:22:33:44:55", name: "Pixel" }],
          },
        }),
      ],
    });
    expect(await screen.findByText(/Pick it only if it's this phone/)).toBeVisible();
    expect(screen.getByRole("button", { name: "Pixel, 00:11:22:33:44:55" })).toBeVisible();
  });

  it("connects a phone whose link is down", async () => {
    const user = userEvent.setup();
    const api = renderPage({
      phones: [phone({ link: { state: "disconnected", device: pixelDevice } })],
    });
    expect(
      await screen.findByText(/paired with this computer over Bluetooth as “Pixel 8”/),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Connect" }));
    expect(api.connectCalls).toHaveBeenCalledWith("phone-1");
  });

  it("asks the user to confirm the pairing code, and only then pairs", async () => {
    const user = userEvent.setup();
    const pairing = {
      deviceId: "phone-1",
      adapterName: "Desk",
      adapterAddress: "A0:B1:C2:D3:E4:F5",
      endsUnixMs: Date.now() + 90_000,
      phone: "asked" as const,
      prompt: null,
    };
    const api = renderPage({ pairing, phones: [phone({ link: { state: "notPaired" } })] });
    expect(await screen.findByText(/Pixel shows a notification to pair/)).toBeVisible();
    expect(screen.getByText(/open Bluetooth settings and pick “Desk”/)).toBeVisible();
    expect(screen.getByText(/Pairing stays open for 1:(29|30)/)).toBeVisible();
    expect(screen.queryByRole("button", { name: "Yes, it matches" })).not.toBeInTheDocument();

    api.emitCalls(
      testCallsStatus({
        pairing: { ...pairing, prompt: { id: 7, deviceName: "Pixel", code: "482913" } },
        phones: [phone({ link: { state: "notPaired" } })],
      }),
    );
    const prompt = await screen.findByRole("alert");
    expect(within(prompt).getByRole("img", { name: "Code 4 8 2 9 1 3" })).toHaveTextContent(
      "482 913",
    );
    expect(screen.getByRole("button", { name: "Yes, it matches" })).toHaveFocus();
    expect(screen.queryByText(/shows a notification to pair/)).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "No" }));
    expect(api.confirmBluetoothCode).toHaveBeenLastCalledWith(7, false);
    await user.click(screen.getByRole("button", { name: "Yes, it matches" }));
    expect(api.confirmBluetoothCode).toHaveBeenLastCalledWith(7, true);
    expect(api.confirmBluetoothCode).toHaveBeenCalledTimes(2);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(api.cancelBluetoothPairing).toHaveBeenCalled();
  });

  it("sends the user to the phone's Bluetooth settings when the phone can't prompt", async () => {
    renderPage({
      pairing: {
        deviceId: "phone-1",
        adapterName: "Desk",
        adapterAddress: "A0:B1:C2:D3:E4:F5",
        endsUnixMs: Date.now() + 90_000,
        phone: "notAllowed",
        prompt: null,
      },
      phones: [phone({ link: { state: "notPaired" } })],
    });
    expect(
      await screen.findByText(
        "Pixel can't show a prompt to pair, maybe because its notifications are blocked. Open Bluetooth settings on Pixel and pick “Desk”.",
      ),
    ).toBeVisible();
    // Pairing stays open for the code.
    expect(screen.getByRole("button", { name: "Cancel" })).toBeVisible();
  });

  it("answers and declines a ringing call", async () => {
    const user = userEvent.setup();
    const api = renderPage({ phones: [phone({ calls: [demoCall()] })] });
    const call = await screen.findByRole("region", { name: "Call with Anna Müller" });
    expect(call).toHaveTextContent("+49 151 2345678");
    expect(call).toHaveTextContent("Incoming call");
    await user.click(within(call).getByRole("button", { name: "Answer" }));
    expect(api.answerCall).toHaveBeenCalledWith("phone-1", "bt:/ag0/call1");
    await user.click(within(call).getByRole("button", { name: "Decline" }));
    expect(api.declineCall).toHaveBeenCalledWith("phone-1", "bt:/ag0/call1");
    expect(within(call).queryByRole("button", { name: "Hang up" })).not.toBeInTheDocument();
  });

  it("shows another app's call, whose sound stays on the phone", async () => {
    renderPage({
      phones: [
        phone({
          link: { state: "notPaired" },
          calls: [
            demoCall({
              id: "app:k1",
              number: "",
              name: "Carol",
              appName: "Messenger",
              audioHere: false,
            }),
          ],
        }),
      ],
    });
    const call = await screen.findByRole("region", { name: "Call with Carol" });
    expect(call).toHaveTextContent("Messenger call");
    expect(call).toHaveTextContent("The call's sound stays on your phone.");
  });

  it("times an active call and offers hang up, mute and the keypad", async () => {
    const user = userEvent.setup();
    const api = renderPage({ phones: [phone({ calls: [active()] })] });
    const call = await screen.findByRole("region", { name: "Call with Anna Müller" });
    expect(call).toHaveTextContent(/1:0[5-9]/);

    const mute = within(call).getByRole("button", { name: "Mute" });
    expect(mute).toHaveAttribute("aria-pressed", "false");
    await user.click(mute);
    expect(api.setCallMuted).toHaveBeenCalledWith("phone-1", true);

    await user.click(within(call).getByRole("button", { name: "Keypad" }));
    await user.click(
      within(within(call).getByRole("group", { name: "Keypad" })).getByRole("button", {
        name: "5",
      }),
    );
    expect(api.sendCallTones).toHaveBeenCalledWith("phone-1", "5");

    await user.click(within(call).getByRole("button", { name: "Hang up" }));
    expect(api.hangUpCall).toHaveBeenCalledWith("phone-1", "bt:/ag0/call1");
  });

  it("offers no mute where the computer can't mute", async () => {
    renderPage({
      phones: [
        phone({
          link: { state: "connected", device: pixelDevice, canMute: false, muted: false },
          calls: [active()],
        }),
      ],
    });
    const call = await screen.findByRole("region", { name: "Call with Anna Müller" });
    expect(within(call).queryByRole("button", { name: "Mute" })).not.toBeInTheDocument();
  });

  it("dials a number typed or tapped, and says when the phone refuses", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.dialNumber.mockRejectedValueOnce(
      new Error("Enter a phone number: digits, and + * # if needed."),
    );
    renderPage({ phones: [phone()] }, api);

    const number = await screen.findByRole("textbox", { name: "Number" });
    await user.type(number, "+1 555 01");
    const pad = screen.getByRole("group", { name: "Dial pad" });
    await user.click(within(pad).getByRole("button", { name: "0" }));
    await user.click(within(pad).getByRole("button", { name: "#" }));
    await user.click(screen.getByRole("button", { name: "Delete the last digit" }));
    expect(number).toHaveValue("+1 555 010");

    await user.click(screen.getByRole("button", { name: "Call" }));
    expect(api.dialNumber).toHaveBeenCalledWith("phone-1", "+1 555 010");
    expect(await screen.findByRole("alert")).toHaveTextContent("Enter a phone number");
    expect(number).toHaveValue("+1 555 010");

    await user.click(screen.getByRole("button", { name: "Call" }));
    expect(number).toHaveValue("");
  });

  it("keeps the selected phone and typed number when a call arrives elsewhere", async () => {
    const user = userEvent.setup();
    const api = renderPage({
      phones: [phone({ deviceId: "phone-1" }), phone({ deviceId: "phone-2", phoneName: "Galaxy" })],
    });
    await user.type(await screen.findByLabelText("Number"), "123");
    api.emitCalls(
      testCallsStatus({
        phones: [
          phone({ deviceId: "phone-1" }),
          phone({ deviceId: "phone-2", phoneName: "Galaxy", calls: [demoCall()] }),
        ],
      }),
    );
    await screen.findByRole("region", { name: "Call with Anna Müller" });
    expect(screen.getByRole("radio", { name: "Pixel" })).toBeChecked();
    expect(screen.getByLabelText("Number")).toHaveValue("123");
  });

  it("keeps cards in place when a ringing call is answered", async () => {
    const api = renderPage({
      phones: [
        phone({
          deviceId: "phone-1",
          phoneName: "Pixel",
          calls: [active({ id: "a", name: "Old" })],
        }),
        phone({ deviceId: "phone-2", phoneName: "Galaxy" }),
      ],
    });
    await screen.findByRole("region", { name: "Call with Old" });
    api.emitCalls(
      testCallsStatus({
        phones: [
          phone({
            deviceId: "phone-1",
            phoneName: "Pixel",
            calls: [active({ id: "a", name: "Old" })],
          }),
          phone({ deviceId: "phone-2", phoneName: "Galaxy", calls: [demoCall({ name: "New" })] }),
        ],
      }),
    );
    await screen.findByRole("region", { name: "Call with New" });
    const names = () =>
      screen
        .getAllByRole("region")
        .map((r) => r.getAttribute("aria-label"))
        .filter((label) => label?.startsWith("Call with"));
    expect(names()).toEqual(["Call with New", "Call with Old"]);
    api.emitCalls(
      testCallsStatus({
        phones: [
          phone({
            deviceId: "phone-1",
            phoneName: "Pixel",
            calls: [active({ id: "a", name: "Old" })],
          }),
          phone({ deviceId: "phone-2", phoneName: "Galaxy", calls: [active({ name: "New" })] }),
        ],
      }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "Answer" })).toBeNull();
    });
    expect(names()).toEqual(["Call with New", "Call with Old"]);
  });

  it("keeps an error from a call card when the shown phone changes by itself", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.hangUpCall.mockRejectedValueOnce(new Error("Nope"));
    const two = (calls: CallView[]) => [
      phone({ deviceId: "phone-1", phoneName: "Pixel", link: { state: "notPaired" } }),
      phone({ deviceId: "phone-2", phoneName: "Galaxy", calls }),
    ];
    renderPage({ phones: two([active()]) }, api);
    await user.click(await screen.findByRole("button", { name: "Hang up" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Nope");
    api.emitCalls(testCallsStatus({ phones: two([]) }));
    await waitFor(() => {
      expect(screen.queryByRole("region", { name: "Call with Anna Müller" })).toBeNull();
    });
    expect(screen.getByRole("alert")).toHaveTextContent("Nope");
  });

  it("doesn't pull focus into a card after its action failed", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.answerCall.mockRejectedValueOnce(new Error("Nope"));
    renderPage({ phones: [phone({ calls: [demoCall()] })] }, api);
    await user.click(await screen.findByRole("button", { name: "Answer" }));
    await screen.findByRole("alert");
    const input = screen.getByLabelText("Number");
    await user.click(input);
    api.emitCalls(testCallsStatus({ phones: [phone({ calls: [active()] })] }));
    await screen.findByRole("button", { name: "Hang up" });
    expect(input).toHaveFocus();
  });

  it("shows every phone's calls whichever phone is selected", async () => {
    renderPage({
      phones: [
        phone({ deviceId: "phone-1", phoneName: "Pixel", link: { state: "notPaired" } }),
        phone({ deviceId: "phone-2", phoneName: "Galaxy", calls: [demoCall()] }),
      ],
    });
    expect(await screen.findByRole("region", { name: "Call with Anna Müller" })).toBeVisible();
    expect(screen.getByRole("radio", { name: "Galaxy, has a call" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Pair Pixel over Bluetooth" })).toBeVisible();
    // The call stays answerable, labelled with its phone.
    expect(screen.getByRole("region", { name: "Call with Anna Müller" })).toHaveTextContent(
      "On Galaxy",
    );
  });

  it("shows a call that starts ringing on a phone that isn't selected", async () => {
    const user = userEvent.setup();
    const api = renderPage({
      phones: [
        phone({ deviceId: "phone-1", phoneName: "Pixel", link: { state: "notPaired" } }),
        phone({ deviceId: "phone-2", phoneName: "Galaxy" }),
      ],
    });
    // Picking a phone is what keeps it selected.
    await user.click(await screen.findByRole("radio", { name: "Galaxy" }));
    await user.click(screen.getByRole("radio", { name: "Pixel" }));
    api.emitCalls(
      testCallsStatus({
        phones: [
          phone({ deviceId: "phone-1", phoneName: "Pixel", link: { state: "notPaired" } }),
          phone({ deviceId: "phone-2", phoneName: "Galaxy", calls: [demoCall()] }),
        ],
      }),
    );
    const call = await screen.findByRole("region", { name: "Call with Anna Müller" });
    expect(call).toHaveTextContent("On Galaxy");
    expect(screen.getByRole("heading", { name: "Pair Pixel over Bluetooth" })).toBeVisible();
    await user.click(within(call).getByRole("button", { name: "Answer" }));
    expect(api.answerCall).toHaveBeenCalledWith("phone-2", "bt:/ag0/call1");
    expect(screen.getByText("Incoming call from Anna Müller on Galaxy")).toBeInTheDocument();
  });

  it("shows why turning calls on failed when no phone is paired", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.setSettings.mockRejectedValueOnce(new Error("Couldn't save settings"));
    renderPage({ desktop: "disabled", enabled: false, phones: [] }, api);
    await user.click(await screen.findByRole("button", { name: "Turn on calls" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't save settings");
  });

  it("sends an answer once however often the button is clicked", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    let finish: () => void = () => undefined;
    api.answerCall.mockReturnValueOnce(
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
    );
    renderPage({ phones: [phone({ calls: [demoCall()] })] }, api);
    const answer = await screen.findByRole("button", { name: "Answer" });
    await user.dblClick(answer);
    expect(api.answerCall).toHaveBeenCalledTimes(1);
    expect(answer).toBeDisabled();
    expect(screen.getByRole("button", { name: "Decline" })).toBeDisabled();
    await act(async () => {
      finish();
      await Promise.resolve();
    });
    expect(answer).toBeEnabled();
  });

  it("keeps focus in the call when the button used goes away", async () => {
    const user = userEvent.setup();
    const api = renderPage({ phones: [phone({ calls: [demoCall()] })] });
    const answer = await screen.findByRole("button", { name: "Answer" });
    await user.click(answer);
    api.emitCalls(testCallsStatus({ phones: [phone({ calls: [active()] })] }));
    const call = await screen.findByRole("region", { name: "Call with Anna Müller" });
    await screen.findByRole("button", { name: "Hang up" });
    expect(call).toHaveFocus();
  });

  it("sends keypad tones in the order they were tapped", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    let finish: () => void = () => undefined;
    api.sendCallTones.mockReturnValueOnce(
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
    );
    renderPage({ phones: [phone({ calls: [active()] })] }, api);
    await user.click(await screen.findByRole("button", { name: "Keypad" }));
    const keys = screen.getByRole("group", { name: "Keypad" });
    await user.click(within(keys).getByRole("button", { name: "1" }));
    await user.click(within(keys).getByRole("button", { name: "2" }));
    expect(api.sendCallTones.mock.calls).toEqual([["phone-1", "1"]]);
    await act(async () => {
      finish();
      await Promise.resolve();
    });
    await waitFor(() => {
      expect(api.sendCallTones.mock.calls).toEqual([
        ["phone-1", "1"],
        ["phone-1", "2"],
      ]);
    });
  });

  it("drops an error when another phone is shown", async () => {
    const user = userEvent.setup();
    const api = createFakeApi();
    api.connectCalls.mockRejectedValueOnce(new Error("Nope"));
    renderPage(
      {
        phones: [
          phone({
            deviceId: "phone-1",
            phoneName: "Pixel",
            link: { state: "disconnected", device: pixelDevice },
          }),
          phone({ deviceId: "phone-2", phoneName: "Galaxy" }),
        ],
      },
      api,
    );
    await user.click(await screen.findByRole("button", { name: "Connect" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Nope");
    await user.click(screen.getByRole("radio", { name: "Galaxy" }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("stops offering the pairing code once the window is over", async () => {
    const api = renderPage({
      pairing: {
        deviceId: "phone-1",
        adapterName: "Desk",
        adapterAddress: "A0:B1:C2:D3:E4:F5",
        endsUnixMs: Date.now() - 1_000,
        phone: "asked",
        prompt: { id: 7, deviceName: "Pixel", code: "482913" },
      },
      phones: [phone({ link: { state: "notPaired" } })],
    });
    expect(await screen.findByText("The pairing window has closed.")).toBeVisible();
    const yes = screen.getByRole("button", { name: "Yes, it matches" });
    expect(yes).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("button", { name: "No" })).toHaveAttribute("aria-disabled", "true");
    await userEvent.setup().click(yes);
    expect(api.confirmBluetoothCode).not.toHaveBeenCalled();
  });

  it("keeps the dial pad's delete button and focus when the number empties", async () => {
    const user = userEvent.setup();
    renderPage({ phones: [phone()] });
    const number = await screen.findByRole("textbox", { name: "Number" });
    expect(number).toHaveAttribute("maxlength", "80");
    const del = screen.getByRole("button", { name: "Delete the last digit" });
    expect(del).toBeDisabled();
    await user.type(number, "7");
    await user.click(del);
    expect(number).toHaveValue("");
    expect(number).toHaveFocus();
    expect(del).toBeDisabled();
  });

  it("advances the timer of an active call", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      renderPage({ phones: [phone({ calls: [active({ activeSinceUnixMs: Date.now() })] })] });
      const call = await screen.findByRole("region", { name: "Call with Anna Müller" });
      expect(call).toHaveTextContent("0:00");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5_000);
      });
      expect(call).toHaveTextContent("0:05");
    } finally {
      vi.useRealTimers();
    }
  });

  it("reports a problem with the phone's calls", async () => {
    renderPage({
      phones: [phone({ link: { state: "notPaired" }, problem: "pairingTimedOut" })],
    });
    expect(await screen.findByText("Pixel didn't pair in time. Try again.")).toBeVisible();
  });
});
