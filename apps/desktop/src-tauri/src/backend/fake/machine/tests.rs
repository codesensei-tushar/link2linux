//! Machine tests: helpers, QR pairing and request checks.

use super::*;
use crate::backend::fake::script::*;

pub const T0: i64 = 1_800_000_000_000;

pub fn at(ms: u64) -> Now {
    Now {
        mono_ms: ms,
        unix_ms: T0 + i64::try_from(ms).unwrap(),
    }
}

pub fn machine(script: Script) -> Machine {
    Machine::new(script, "Desk", at(0))
}

pub fn step(m: &Machine, now: u64) -> &'static str {
    match m.pairing(at(now)) {
        PairingState::Idle => "idle",
        PairingState::QrOffer { .. } => "qrOffer",
        PairingState::Searching { .. } => "searching",
        PairingState::Connecting { .. } => "connecting",
        PairingState::Verifying { .. } => "verifying",
        PairingState::SasCode { .. } => "sasCode",
        PairingState::Paired { .. } => "paired",
        PairingState::Failed { .. } => "failed",
    }
}

pub fn failure(m: &Machine, now: u64) -> PairingFailure {
    match m.pairing(at(now)) {
        PairingState::Failed { reason, .. } => reason,
        other => panic!("expected failed, got {other:?}"),
    }
}

pub fn discovered(id: &str) -> PhoneTarget {
    PhoneTarget::Discovered { id: id.to_owned() }
}

pub fn address(text: &str) -> PhoneTarget {
    PhoneTarget::Address {
        address: text.to_owned(),
    }
}

pub const PIXEL: &str = "l2l-3f9a12bc";

#[test]
fn qr_pairing_runs_through_every_step() {
    let mut m = machine(Script::Success);
    m.start_pairing(PairingMethod::QrCode, at(0)).unwrap();
    let PairingState::QrOffer {
        uri,
        svg,
        desktop_name,
        expires_in_ms,
        lifetime_ms,
    } = m.pairing(at(1_000))
    else {
        panic!("expected a QR offer");
    };
    assert!(uri.starts_with("link2linux://pair?v=1&f="));
    assert!(svg.contains("<svg"));
    assert_eq!(desktop_name, "Desk");
    assert_eq!((expires_in_ms, lifetime_ms), (299_000, OFFER_LIFETIME_MS));

    assert_eq!(m.next_deadline(), Some(SCAN_DELAY_MS));
    assert_eq!(m.tick(at(SCAN_DELAY_MS - 1)), Changed::default());
    let changed = m.tick(at(SCAN_DELAY_MS));
    assert!(changed.pairing && !changed.devices);
    assert_eq!(step(&m, SCAN_DELAY_MS), "connecting");

    m.tick(at(SCAN_DELAY_MS + CONNECT_MS));
    assert_eq!(step(&m, SCAN_DELAY_MS + CONNECT_MS), "verifying");

    let done = SCAN_DELAY_MS + CONNECT_MS + VERIFY_MS;
    let changed = m.tick(at(done));
    assert!(changed.pairing && changed.devices);
    let PairingState::Paired { device } = m.pairing(at(done)) else {
        panic!("not paired")
    };
    assert_eq!(device.status, DeviceStatus::Connected);
    assert_eq!(device.paired_at_unix_ms, T0 + i64::try_from(done).unwrap());
    assert_eq!(m.devices(), vec![device]);
    assert_eq!(m.next_deadline(), None);
}

#[test]
fn a_late_tick_catches_up_with_chained_steps() {
    let mut m = machine(Script::Success);
    m.start_pairing(PairingMethod::QrCode, at(0)).unwrap();
    let changed = m.tick(at(60_000));
    assert!(changed.devices);
    let PairingState::Paired { device } = m.pairing(at(60_000)) else {
        panic!("not paired")
    };
    // Timestamps follow the schedule, not the late tick.
    assert_eq!(device.paired_at_unix_ms, T0 + 6_400);
}

#[test]
fn qr_offers_are_replaced_before_they_expire_then_give_up() {
    let mut m = machine(Script::Expired);
    m.start_pairing(PairingMethod::QrCode, at(0)).unwrap();
    let first = m.pairing(at(0));

    let renew = u64::from(OFFER_LIFETIME_MS - OFFER_REFRESH_BEFORE_MS);
    assert_eq!(m.next_deadline(), Some(renew));
    m.tick(at(renew));
    let second = m.pairing(at(renew));
    assert_ne!(first, second, "a new offer has a new secret");
    let PairingState::QrOffer { expires_in_ms, .. } = second else {
        panic!("no offer")
    };
    assert_eq!(expires_in_ms, OFFER_LIFETIME_MS);

    let second_renew = renew + renew;
    m.tick(at(second_renew));
    assert_eq!(step(&m, second_renew), "qrOffer");

    // The third (last) offer runs its full length.
    let end = second_renew + u64::from(OFFER_LIFETIME_MS);
    assert_eq!(m.next_deadline(), Some(end));
    m.tick(at(end));
    assert_eq!(failure(&m, end), PairingFailure::Expired);
}

#[test]
fn identical_machines_make_identical_offers() {
    let offer = || {
        let mut m = machine(Script::Success);
        m.start_pairing(PairingMethod::QrCode, at(0)).unwrap();
        m.pairing(at(0))
    };
    assert_eq!(offer(), offer());
}

#[test]
fn qr_wrong_proof_fails_verification() {
    let mut m = machine(Script::WrongCode);
    m.start_pairing(PairingMethod::QrCode, at(0)).unwrap();
    m.tick(at(10_000));
    assert_eq!(failure(&m, 10_000), PairingFailure::VerificationFailed);
    assert!(m.devices().is_empty());
}

#[test]
fn qr_declined_and_typed_address_paths() {
    let mut m = machine(Script::Declined);
    m.start_pairing(PairingMethod::QrCode, at(0)).unwrap();
    m.tick(at(10_000));
    assert_eq!(failure(&m, 10_000), PairingFailure::DeclinedByPhone);

    let mut m = machine(Script::NotFound);
    m.start_pairing(PairingMethod::QrCode, at(0)).unwrap();
    m.pair_with(address("192.168.1.9:27380"), at(20_000))
        .unwrap();
    assert_eq!(step(&m, 20_000), "connecting");
    m.tick(at(25_000));
    assert_eq!(failure(&m, 25_000), PairingFailure::PhoneNotFound);

    let mut m = machine(Script::Success);
    m.start_pairing(PairingMethod::QrCode, at(0)).unwrap();
    m.pair_with(address("[fd00::2]:27380"), at(1_000)).unwrap();
    m.tick(at(10_000));
    assert_eq!(step(&m, 10_000), "paired");
}

#[test]
fn typed_addresses_are_checked() {
    let mut m = machine(Script::Success);
    m.start_pairing(PairingMethod::Sas, at(0)).unwrap();
    for bad in [
        "",
        "phone.local",
        "0.0.0.0",
        "10.0.0.1:0",
        "10.0.0.1:99999",
        "999.1.1.1",
        "224.0.0.1",
    ] {
        assert_eq!(
            m.pair_with(address(bad), at(0)),
            Err(BackendError::InvalidAddress),
            "{bad:?}"
        );
    }
    assert_eq!(step(&m, 0), "searching");
    assert!(m.pair_with(address(" 10.0.0.1 "), at(0)).is_ok());
}

#[test]
fn requests_that_do_not_fit_the_step_are_refused() {
    let mut m = machine(Script::Success);
    assert_eq!(m.answer_code(true, at(0)), Err(BackendError::WrongState));
    assert_eq!(
        m.pair_with(discovered(PIXEL), at(0)),
        Err(BackendError::WrongState)
    );
    m.start_pairing(PairingMethod::Sas, at(0)).unwrap();
    assert_eq!(
        m.start_pairing(PairingMethod::QrCode, at(0)),
        Err(BackendError::WrongState)
    );
    assert_eq!(
        m.pair_with(discovered("nobody"), at(0)),
        Err(BackendError::NotFound)
    );
    assert_eq!(m.unpair("nobody"), Err(BackendError::NotFound));
}

#[test]
fn cancel_always_ends_idle_and_stops_the_clock() {
    let mut m = machine(Script::Success);
    assert!(!m.cancel_pairing().pairing, "nothing to report when idle");
    m.start_pairing(PairingMethod::QrCode, at(0)).unwrap();
    assert!(m.cancel_pairing().pairing);
    assert_eq!(step(&m, 0), "idle");
    assert_eq!(m.next_deadline(), None);
    assert_eq!(m.tick(at(1_000_000)), Changed::default());

    // A finished attempt is dismissed the same way, and can be restarted.
    m.start_pairing(PairingMethod::QrCode, at(0)).unwrap();
    m.tick(at(10_000));
    assert_eq!(step(&m, 10_000), "paired");
    m.start_pairing(PairingMethod::QrCode, at(10_000)).unwrap();
    assert_eq!(step(&m, 10_000), "qrOffer");
}

#[test]
fn demo_starts_with_two_phones_and_unpair_removes_one() {
    let mut m = machine(Script::Demo);
    let devices = m.devices();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].status, DeviceStatus::Connected);
    assert_eq!(devices[1].status, DeviceStatus::Disconnected);

    let changed = m.unpair("demo-1").unwrap();
    assert_eq!(
        changed,
        Changed {
            pairing: false,
            devices: true
        }
    );
    assert_eq!(m.devices().len(), 1);
}

#[test]
fn script_names_round_trip() {
    for (name, script) in [
        ("success", Script::Success),
        ("wrong-code", Script::WrongCode),
        ("expired", Script::Expired),
        ("not-found", Script::NotFound),
        ("declined", Script::Declined),
        ("demo", Script::Demo),
    ] {
        assert_eq!(Script::parse(name), Some(script));
    }
    assert_eq!(Script::parse("bogus"), None);
}
