//! Machine tests: code comparison pairing.

use super::tests::{PIXEL, address, at, discovered, failure, machine, step};
use super::*;
use crate::backend::fake::script::*;

/// Runs one SAS attempt up to the code screen and returns the time.
pub fn reach_code(m: &mut Machine, from: u64) -> u64 {
    m.pair_with(discovered(PIXEL), at(from)).unwrap();
    let shown = from + CONNECT_MS;
    m.tick(at(shown));
    assert_eq!(step(m, shown), "sasCode");
    shown
}

#[test]
fn sas_lists_phones_then_pairs_when_both_confirm() {
    let mut m = machine(Script::Success);
    m.start_pairing(PairingMethod::Sas, at(0)).unwrap();
    let PairingState::Searching {
        phones,
        attempts_left,
        last_failure,
    } = m.pairing(at(0))
    else {
        panic!("not searching");
    };
    assert!(phones.is_empty());
    assert_eq!((attempts_left, last_failure), (3, None));

    m.tick(at(DISCOVERY_DELAY_MS));
    let PairingState::Searching { phones, .. } = m.pairing(at(DISCOVERY_DELAY_MS)) else {
        panic!()
    };
    assert_eq!(phones.len(), 2);

    let shown = reach_code(&mut m, 2_000);
    let PairingState::SasCode {
        code,
        expires_in_ms,
        lifetime_ms,
        confirmed_here,
        ..
    } = m.pairing(at(shown + 10_000))
    else {
        panic!()
    };
    assert_eq!(code.len(), 6);
    assert!(code.bytes().all(|b| b.is_ascii_digit()));
    assert_eq!(
        (expires_in_ms, lifetime_ms, confirmed_here),
        (50_000, SAS_LIFETIME_MS, false)
    );

    m.answer_code(true, at(shown + 5_000)).unwrap();
    let PairingState::SasCode { confirmed_here, .. } = m.pairing(at(shown + 5_000)) else {
        panic!()
    };
    assert!(confirmed_here);
    assert_eq!(
        m.answer_code(true, at(shown + 5_001)),
        Err(BackendError::WrongState)
    );

    let done = shown + 5_000 + PHONE_CONFIRM_MS;
    assert!(m.tick(at(done)).devices);
    assert_eq!(step(&m, done), "paired");
}

#[test]
fn sas_mismatch_returns_to_the_list_and_counts_the_attempt() {
    let mut m = machine(Script::Success);
    m.start_pairing(PairingMethod::Sas, at(0)).unwrap();
    m.tick(at(DISCOVERY_DELAY_MS));
    let shown = reach_code(&mut m, 2_000);
    m.answer_code(false, at(shown)).unwrap();
    let PairingState::Searching {
        phones,
        attempts_left,
        last_failure,
    } = m.pairing(at(shown))
    else {
        panic!("not searching");
    };
    assert_eq!(phones.len(), 2, "the list is kept");
    assert_eq!(attempts_left, 2);
    assert_eq!(last_failure, Some(PairingFailure::VerificationFailed));
}

#[test]
fn third_failed_attempt_requires_starting_again_without_redialing() {
    let mut m = machine(Script::WrongCode);
    m.start_pairing(PairingMethod::Sas, at(0)).unwrap();
    m.tick(at(DISCOVERY_DELAY_MS));
    let mut now = 2_000;
    for expected_left in [2, 1] {
        let shown = reach_code(&mut m, now);
        m.answer_code(true, at(shown)).unwrap();
        now = shown + PHONE_CONFIRM_MS;
        m.tick(at(now));
        let PairingState::Searching {
            attempts_left,
            last_failure,
            ..
        } = m.pairing(at(now))
        else {
            panic!("not searching");
        };
        assert_eq!(attempts_left, expected_left);
        assert_eq!(last_failure, Some(PairingFailure::VerificationFailed));
        assert_eq!(m.next_deadline(), None, "nothing redials by itself");
    }
    let shown = reach_code(&mut m, now);
    m.answer_code(false, at(shown)).unwrap();
    assert_eq!(failure(&m, shown), PairingFailure::TooManyAttempts);
    assert_eq!(
        m.pair_with(discovered(PIXEL), at(shown)),
        Err(BackendError::WrongState)
    );

    // A new activation gets three fresh attempts.
    m.start_pairing(PairingMethod::Sas, at(shown + 1)).unwrap();
    let PairingState::Searching { attempts_left, .. } = m.pairing(at(shown + 1)) else {
        panic!()
    };
    assert_eq!(attempts_left, 3);
}

#[test]
fn sas_code_times_out_after_a_minute() {
    let mut m = machine(Script::Success);
    m.start_pairing(PairingMethod::Sas, at(0)).unwrap();
    m.tick(at(DISCOVERY_DELAY_MS));
    let shown = reach_code(&mut m, 2_000);
    assert_eq!(m.next_deadline(), Some(shown + u64::from(SAS_LIFETIME_MS)));
    m.tick(at(shown + u64::from(SAS_LIFETIME_MS)));
    let PairingState::Searching {
        last_failure,
        attempts_left,
        ..
    } = m.pairing(at(shown + 60_000))
    else {
        panic!()
    };
    assert_eq!(
        (last_failure, attempts_left),
        (Some(PairingFailure::TimedOut), 2)
    );
}

#[test]
fn sas_failures_before_the_code_cost_no_attempt() {
    let mut m = machine(Script::NotFound);
    m.start_pairing(PairingMethod::Sas, at(0)).unwrap();
    assert_eq!(m.next_deadline(), None, "no phone ever appears");
    for round in 0..4 {
        let start = round * 10_000;
        m.pair_with(address("10.0.0.5"), at(start)).unwrap();
        m.tick(at(start + CONNECT_MS));
        let PairingState::Searching {
            attempts_left,
            last_failure,
            ..
        } = m.pairing(at(start + CONNECT_MS))
        else {
            panic!()
        };
        assert_eq!(
            (attempts_left, last_failure),
            (3, Some(PairingFailure::PhoneNotFound))
        );
    }
}
