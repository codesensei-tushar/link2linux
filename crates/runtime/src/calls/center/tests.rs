use link2linux_platform::{GatewayCall, PairingPrompt};
use link2linux_protocol::v1::{
    NotificationAction, NotificationActionKind, NotificationCategory, PhoneNotification,
};

use super::*;

const PIXEL: &str = "0C:1A:2B:3C:4D:5E";
const GALAXY: &str = "00:11:22:33:44:55";

fn address(text: &str) -> BluetoothAddress {
    text.parse().unwrap()
}

fn phones() -> Vec<Phone> {
    vec![
        Phone {
            id: "p1".into(),
            name: "Pixel 8".into(),
        },
        Phone {
            id: "p2".into(),
            name: "Galaxy".into(),
        },
    ]
}

fn device(at: &str, name: &str) -> BluetoothDevice {
    BluetoothDevice {
        address: address(at),
        name: name.into(),
        paired: true,
        trusted: true,
        connected: true,
        phone: true,
        hands_free_gateway: true,
    }
}

fn adapter(powered: bool) -> BluetoothAdapter {
    BluetoothAdapter {
        address: address("A0:B1:C2:D3:E4:F5"),
        name: "workstation".into(),
        powered,
    }
}

fn call(id: &str, number: &str, state: CallState) -> GatewayCall {
    GatewayCall {
        id: id.into(),
        number: number.into(),
        name: String::new(),
        state,
        multiparty: false,
    }
}

fn gateway(at: Option<&str>, calls: Vec<GatewayCall>) -> Gateway {
    Gateway {
        id: "/ag0".into(),
        address: at.map(address),
        calls,
        muted: Some(false),
    }
}

fn seen(devices: Vec<BluetoothDevice>, gateways: Vec<Gateway>) -> Seen {
    Seen {
        adapter: Some(adapter(true)),
        devices,
        gateways: Some(gateways),
    }
}

/// Takes in `seen` as the runtime does: then each phone's device is
/// settled.
fn feed(center: &mut Center, seen: Seen, now_ms: i64) {
    center.update(seen, now_ms);
    center.settle_choices(&phones());
}

/// A desktop that can take calls, with both phones connected to the app.
fn ready(devices: Vec<BluetoothDevice>, gateways: Vec<Gateway>) -> Center {
    let mut center = Center::new(true, true, BTreeMap::new());
    feed(&mut center, seen(devices, gateways), 0);
    center.session_started("p1");
    center.session_started("p2");
    center
}

fn ringing_note(key: &str, caller: &str) -> CallNotification {
    let action = |id: &str, kind: NotificationActionKind| NotificationAction {
        id: id.into(),
        label: id.into(),
        kind: kind.into(),
        ..NotificationAction::default()
    };
    CallNotification::of(&PhoneNotification {
        key: key.into(),
        app_name: "Phone".into(),
        title: caller.into(),
        category: NotificationCategory::Call.into(),
        actions: vec![
            action("answer", NotificationActionKind::AnswerCall),
            action("decline", NotificationActionKind::DeclineCall),
        ],
        ..PhoneNotification::default()
    })
    .unwrap()
}

#[test]
fn says_why_the_desktop_cannot_take_calls() {
    assert_eq!(
        Center::new(false, true, BTreeMap::new()).desktop(),
        CallsDesktop::Unsupported
    );
    let mut center = Center::new(true, true, BTreeMap::new());
    assert_eq!(center.desktop(), CallsDesktop::Checking);
    assert_eq!(center.availability(), Availability::Unavailable);

    center.update(Seen::default(), 0);
    assert_eq!(center.desktop(), CallsDesktop::NoAdapter);
    center.update(
        Seen {
            adapter: Some(adapter(false)),
            ..seen(vec![], vec![])
        },
        0,
    );
    assert_eq!(center.desktop(), CallsDesktop::BluetoothOff);
    center.update(
        Seen {
            gateways: None,
            ..seen(vec![], vec![])
        },
        0,
    );
    assert_eq!(center.desktop(), CallsDesktop::NoCallControl);
    assert_eq!(center.availability(), Availability::Unavailable);

    center.update(seen(vec![], vec![]), 0);
    assert_eq!(center.availability(), Availability::Ready);
    center.set_enabled(false);
    assert_eq!(center.desktop(), CallsDesktop::Disabled);
    assert_eq!(center.availability(), Availability::Disabled);
}

#[test]
fn phones_named_like_a_device_get_it_before_others_take_the_only_one() {
    let mut center = Center::new(true, true, BTreeMap::new());
    center.update(seen(vec![device(GALAXY, "Galaxy")], vec![]), 0);
    assert!(center.settle_choices(&phones()));
    assert_eq!(center.choices().get("p2"), Some(&address(GALAXY)));
    assert_eq!(center.choices().get("p1"), None);
    assert_eq!(center.device_of(&phones()[0]), PhoneDevice::NotPaired);
    assert!(!center.settle_choices(&phones()));

    // A device the user picks moves to that phone.
    assert!(center.choose("p1", address(GALAXY)));
    assert_eq!(center.choices().get("p1"), Some(&address(GALAXY)));
    assert_eq!(center.choices().get("p2"), None);
    assert!(!center.choose("p1", address("AA:AA:AA:AA:AA:AA")));
}

#[test]
fn gateways_belong_to_the_phone_with_their_address() {
    let phones = phones();
    let center = ready(
        vec![device(PIXEL, "Pixel 8"), device(GALAXY, "Galaxy")],
        vec![gateway(Some(GALAXY), vec![])],
    );
    assert!(center.gateway_of(&phones[0], &phones).is_none());
    assert!(center.gateway_of(&phones[1], &phones).is_some());
    assert!(center.shows_calls(&phones[1], &phones));
    assert!(!center.shows_calls(&phones[0], &phones));

    // A gateway without an address goes to the only phone with a device.
    let one = ready(vec![device(PIXEL, "Pixel 8")], vec![gateway(None, vec![])]);
    assert!(one.gateway_of(&phones[0], &phones).is_some());
    let two = ready(
        vec![device(PIXEL, "Pixel 8"), device(GALAXY, "Galaxy")],
        vec![gateway(None, vec![])],
    );
    assert!(two.gateway_of(&phones[0], &phones).is_none());
}

#[test]
fn the_snapshot_shows_each_phones_link_and_calls() {
    let phones = phones();
    let mut center = ready(
        vec![device(PIXEL, "Pixel 8")],
        vec![gateway(
            Some(PIXEL),
            vec![call("/ag0/call0", "+15550100", CallState::Incoming)],
        )],
    );
    center.set_note("p1", "k1", Some(ringing_note("k1", "Anna")));
    center.set_note("p2", "k2", Some(ringing_note("k2", "Bob")));
    let status = center.status(&phones, Instant::now(), 0);
    assert_eq!(status.desktop, CallsDesktop::Ready);
    assert!(status.pairing.is_none());

    let pixel = &status.phones[0];
    assert_eq!(
        pixel.link,
        CallLink::Connected {
            device: BluetoothChoice {
                address: PIXEL.into(),
                name: "Pixel 8".into()
            },
            can_mute: true,
            muted: false,
        }
    );
    // One call: the link's, named by the notification.
    assert_eq!(pixel.calls.len(), 1);
    let ringing = &pixel.calls[0];
    assert_eq!(ringing.name, "Anna");
    assert!(ringing.audio_here && ringing.can_answer && ringing.can_decline);
    assert!(!ringing.can_hang_up && !ringing.can_send_tones);

    // The other phone is not paired over Bluetooth; its call comes only
    // as a notification, answered on the phone.
    let galaxy = &status.phones[1];
    assert_eq!(galaxy.link, CallLink::NotPaired);
    assert!(!galaxy.calls[0].audio_here);
    assert!(galaxy.calls[0].can_answer);

    center.set_connecting("p1", true);
    feed(&mut center, seen(vec![device(PIXEL, "Pixel 8")], vec![]), 0);
    let status = center.status(&phones, Instant::now(), 0);
    assert!(matches!(status.phones[0].link, CallLink::Connecting { .. }));

    center.set_enabled(false);
    let status = center.status(&phones, Instant::now(), 0);
    assert_eq!(status.phones[0].link, CallLink::Off);
    assert!(!format!("{status:?}").contains("Anna"));
}

#[test]
fn several_possible_devices_are_offered() {
    let phones = phones();
    let center = ready(vec![device(PIXEL, "Work"), device(GALAXY, "Home")], vec![]);
    let status = center.status(&phones, Instant::now(), 0);
    let CallLink::Choose { candidates } = &status.phones[0].link else {
        panic!("expected a choice, got {:?}", status.phones[0].link);
    };
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].name, "Work");
}

#[test]
fn a_call_has_a_timer_from_when_it_became_active() {
    let phones = phones();
    let mut center = ready(
        vec![device(PIXEL, "Pixel 8")],
        vec![gateway(
            Some(PIXEL),
            vec![call("/ag0/call0", "1", CallState::Dialing)],
        )],
    );
    let link = |state| {
        seen(
            vec![device(PIXEL, "Pixel 8")],
            vec![gateway(Some(PIXEL), vec![call("/ag0/call0", "1", state)])],
        )
    };
    feed(&mut center, link(CallState::Active), 1_000);
    feed(&mut center, link(CallState::Active), 5_000);
    let status = center.status(&phones, Instant::now(), 5_000);
    assert_eq!(status.phones[0].calls[0].active_since_unix_ms, Some(1_000));
    assert!(status.phones[0].calls[0].can_send_tones);
    feed(&mut center, link(CallState::Held), 6_000);
    feed(&mut center, link(CallState::Active), 7_000);
    let status = center.status(&phones, Instant::now(), 7_000);
    assert_eq!(status.phones[0].calls[0].active_since_unix_ms, Some(7_000));
}

#[test]
fn the_pairing_window_runs_until_the_phone_pairs() {
    let phones = phones();
    let now = Instant::now();
    let mut center = ready(vec![], vec![]);
    center.open_window(PairingWindow::open("p1", adapter(true), now));
    let request = center.request_for("p1").unwrap();
    assert_eq!(request.name, "workstation");
    assert!(center.request_for("p2").is_none());
    center.phone_answered("p1", PhoneAnswer::Waiting);
    assert!(center.request_for("p1").is_none());

    let window = center.window_mut().unwrap();
    window.prompted(
        PairingPrompt {
            id: 7,
            device: address(PIXEL),
            device_name: "Pixel 8".into(),
            code: 12_345,
        },
        now,
    );
    let status = center.status(&phones, now, 1_000);
    let pairing = status.pairing.unwrap();
    assert_eq!(pairing.device_id, "p1");
    assert_eq!(pairing.ends_unix_ms, 121_000);
    assert_eq!(pairing.phone, PairingRequestState::Waiting);
    let prompt = pairing.prompt.unwrap();
    assert_eq!((prompt.id, prompt.code.as_str()), (7, "012345"));

    assert_eq!(
        center.window_mut().unwrap().answer(7, true, now),
        Some(true)
    );
    assert_eq!(center.pairing_done(), None);
    center.update(seen(vec![device(PIXEL, "Phone")], vec![]), 0);
    assert_eq!(center.pairing_done(), Some(("p1".into(), address(PIXEL))));
    assert!(center.window().is_none());
    assert_eq!(center.choices().get("p1"), Some(&address(PIXEL)));
}

#[test]
fn a_window_that_runs_out_reports_it() {
    let phones = phones();
    let now = Instant::now();
    let mut center = ready(vec![], vec![]);
    center.open_window(PairingWindow::open("p1", adapter(true), now));
    center.window_mut().unwrap().prompted(
        PairingPrompt {
            id: 3,
            device: address(PIXEL),
            device_name: String::new(),
            code: 1,
        },
        now,
    );
    assert_eq!(center.expire(now), None);
    assert_eq!(
        center.expire(now + link2linux_core::calls::PAIRING_WINDOW),
        Some(vec![3])
    );
    let status = center.status(&phones, now, 0);
    assert!(status.pairing.is_none());
    assert_eq!(status.phones[0].problem, Some(CallProblem::PairingTimedOut));
    // Opening it again clears the problem.
    center.open_window(PairingWindow::open("p1", adapter(true), now));
    assert_eq!(center.status(&phones, now, 0).phones[0].problem, None);
}

#[test]
fn ringing_calls_get_one_popup_while_the_desktop_shows_them() {
    let phones = phones();
    let link = |state| {
        seen(
            vec![device(PIXEL, "Pixel 8")],
            vec![gateway(
                Some(PIXEL),
                vec![call("/ag0/call0", "+15550100", state)],
            )],
        )
    };
    let mut center = ready(vec![], vec![]);
    feed(&mut center, link(CallState::Incoming), 0);
    let plan = center.plan_popups(&phones);
    assert_eq!(plan.show.len(), 1);
    let (call, popup, locked, replaces) = &plan.show[0];
    assert_eq!(call, "bt:/ag0/call0");
    assert_eq!(popup.summary, "Call from +15550100");
    assert!(popup.body.contains("Pixel 8"));
    assert_eq!(locked.summary, "Incoming call");
    assert!(!locked.body.contains("5550100"));
    assert_eq!(*replaces, None);
    let keys: Vec<&str> = popup.actions.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(keys, [ANSWER_KEY, DECLINE_KEY]);
    center.popup_shown("bt:/ag0/call0", NotificationId(9));
    assert_eq!(
        center.popup_call(NotificationId(9)),
        Some(("p1".into(), "bt:/ag0/call0".into()))
    );
    assert!(center.plan_popups(&phones).show.is_empty());

    // The caller's name arrives with the notification: the pop-up says it.
    center.set_note("p1", "k1", Some(ringing_note("k1", "Anna")));
    let plan = center.plan_popups(&phones);
    assert_eq!(plan.show.len(), 1);
    assert_eq!(plan.show[0].1.summary, "Call from Anna");
    assert_eq!(plan.show[0].3, Some(NotificationId(9)));

    // Answered: the pop-up goes.
    feed(&mut center, link(CallState::Active), 0);
    let plan = center.plan_popups(&phones);
    assert!(plan.show.is_empty());
    assert_eq!(plan.close, [NotificationId(9)]);
    assert_eq!(center.popup_call(NotificationId(9)), None);
}

#[test]
fn without_the_link_the_phones_own_banner_shows_the_call() {
    let phones = phones();
    let mut center = ready(vec![device(PIXEL, "Pixel 8")], vec![]);
    center.set_note("p1", "k1", Some(ringing_note("k1", "Anna")));
    assert!(center.plan_popups(&phones).show.is_empty());
}

#[test]
fn call_notifications_count_only_while_the_phone_has_a_session() {
    let phones = phones();
    let mut center = ready(vec![], vec![]);
    center.set_note("p1", "k1", Some(ringing_note("k1", "Anna")));
    assert_eq!(center.calls_of(&phones[0], &phones).len(), 1);
    // A new connection replaces the old one: the old session ends last.
    center.session_started("p1");
    center.session_ended("p1");
    assert_eq!(center.calls_of(&phones[0], &phones).len(), 1);
    center.session_ended("p1");
    assert!(center.calls_of(&phones[0], &phones).is_empty());
    assert!(center.has_note("p1", "k1"));
}

#[test]
fn forgetting_a_phone_drops_its_window_and_choice() {
    let mut center = ready(vec![device(PIXEL, "Pixel 8")], vec![]);
    center.settle_choices(&phones());
    center.open_window(PairingWindow::open("p1", adapter(true), Instant::now()));
    center.forget("p1");
    assert!(center.window().is_none());
    assert!(center.choices().is_empty());
}
