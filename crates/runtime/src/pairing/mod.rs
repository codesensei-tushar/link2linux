//! Pairing a new phone: the QR code and the code comparison, driven by the
//! user's commands and by what discovery finds.
//!
//! The state machine's data is in [`state`]; [`run`] is the task that dials
//! a phone and runs the pairing exchange on the connection. This module is
//! the entry points the shell calls and the rules that start, renew and end
//! an activation.

mod run;
mod state;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use link2linux_core::discovery::SecretBytes;
use link2linux_core::discovery::onlink::is_on_link;
use link2linux_core::discovery::reconnect::dial_plan;
use link2linux_core::pairing::{MAX_QR_ADDRESSES, OfferStatus, QrOffer};
use tokio::time::{self, Instant};

use crate::link::lock;
use crate::qr;
use crate::shared::Core;
use crate::view::{
    BackendError, PairingFailure, PairingMethod, PairingState, PhoneTarget, QrImage,
};

use state::{Code, MAX_OFFERS, OFFER_REFRESH_BEFORE, Phase, SAS_MAX_ATTEMPTS};
pub(crate) use state::{PairingCtl, PhoneList, parse_address};

/// The state the UI shows now.
pub(crate) fn state(core: &Core) -> PairingState {
    let ctl = lock(&core.pairing);
    view(core, &ctl)
}

fn view(core: &Core, ctl: &PairingCtl) -> PairingState {
    ctl.view(
        Instant::now(),
        &core.desktop_name,
        &lock(&core.pairing_phones),
    )
}

/// Reports the state to the shell. Call it with the pairing lock held, so
/// reports arrive in order.
fn emit(core: &Core, ctl: &PairingCtl) {
    core.events.pairing_changed(&view(core, ctl));
}

/// Begins an activation.
pub(crate) fn start(core: &Arc<Core>, method: PairingMethod) -> Result<(), BackendError> {
    let mut ctl = lock(&core.pairing);
    if !ctl.phase.is_startable() {
        return Err(BackendError::WrongState);
    }
    ctl.reset();
    match method {
        PairingMethod::QrCode => {
            show_offer(core, &mut ctl, 1);
            let (generation, token) = (ctl.generation, ctl.token.clone());
            core.handle
                .spawn(renew_offers(core.clone(), generation, token));
        }
        PairingMethod::Sas => {
            ctl.attempts_left = SAS_MAX_ATTEMPTS;
            ctl.phase = Phase::Searching { last_failure: None };
        }
    }
    emit(core, &ctl);
    Ok(())
}

/// Dials the phone the user picked or typed.
pub(crate) fn pair_with(core: &Arc<Core>, target: PhoneTarget) -> Result<(), BackendError> {
    let mut ctl = lock(&core.pairing);
    if !ctl.dialing.is_empty() {
        return Err(BackendError::WrongState);
    }
    let (method, plan, name) = match (&ctl.phase, target) {
        (Phase::Qr { .. }, PhoneTarget::Address { address }) => {
            let address = parse_address(&address)?;
            (
                PairingMethod::QrCode,
                dial_plan(None, &[address], &[]),
                None,
            )
        }
        (Phase::Searching { .. }, PhoneTarget::Address { address }) => {
            let address = parse_address(&address)?;
            (PairingMethod::Sas, dial_plan(None, &[address], &[]), None)
        }
        (Phase::Searching { .. }, PhoneTarget::Discovered { id }) => {
            let addresses = lock(&core.pairing_phones)
                .addresses(&id)
                .ok_or(BackendError::NotFound)?;
            // Addresses from an announcement are hints: only on-link ones.
            let locals = core.locals();
            let usable: Vec<SocketAddr> = addresses
                .into_iter()
                .filter(|address| is_on_link(*address, &locals))
                .collect();
            (PairingMethod::Sas, dial_plan(None, &usable, &[]), Some(id))
        }
        _ => return Err(BackendError::WrongState),
    };
    begin_connection(core, &mut ctl, method, plan, name, true);
    Ok(())
}

/// The user's answer to "do the codes match?".
pub(crate) fn answer_code(core: &Core, matches: bool) -> Result<(), BackendError> {
    let mut ctl = lock(&core.pairing);
    let Phase::Sas { confirmed_here, .. } = &mut ctl.phase else {
        return Err(BackendError::WrongState);
    };
    if *confirmed_here {
        return Err(BackendError::WrongState);
    }
    if matches {
        *confirmed_here = true;
    }
    if let Some(answer) = ctl.answer.take() {
        // A closed channel means the exchange already ended.
        let _ = answer.send(matches);
    }
    emit(core, &ctl);
    Ok(())
}

/// Stops pairing, or dismisses a finished one.
pub(crate) fn cancel(core: &Core) {
    let mut ctl = lock(&core.pairing);
    let was_idle = matches!(ctl.phase, Phase::Idle);
    ctl.reset();
    ctl.phase = Phase::Idle;
    clear_offer_secret(core);
    if !was_idle {
        emit(core, &ctl);
    }
}

/// Discovery found a phone that shows the pairing tag of the QR offer, at
/// these addresses.
///
/// Anyone can copy the tag, so addresses get their own connections even
/// while others run: one that stalls must not keep the phone that scanned
/// the code from being dialed.
pub(crate) fn offer_seen(core: &Arc<Core>, addresses: &[SocketAddr]) {
    let mut ctl = lock(&core.pairing);
    let usable = ctl.offer_targets(addresses, &core.locals());
    let plan = dial_plan(None, &usable, &[]);
    if plan.is_empty() {
        return;
    }
    begin_connection(core, &mut ctl, PairingMethod::QrCode, plan, None, false);
}

/// Discovery found, or lost, a phone in pairing mode.
pub(crate) fn phone_listed(core: &Core, instance: &str, addresses: Option<Vec<SocketAddr>>) {
    let ctl = lock(&core.pairing);
    let changed = {
        let mut phones = lock(&core.pairing_phones);
        match addresses {
            Some(addresses) => {
                phones.upsert(instance, addresses);
                true
            }
            None => phones.remove(instance),
        }
    };
    if changed && matches!(ctl.phase, Phase::Searching { .. }) {
        emit(core, &ctl);
    }
}

/// Opens a pairing connection to the addresses of `plan`. `asked` says the
/// user picked or typed the phone, rather than discovery finding it.
fn begin_connection(
    core: &Arc<Core>,
    ctl: &mut PairingCtl,
    method: PairingMethod,
    plan: crate::candidates::Plan,
    phone: Option<String>,
    asked: bool,
) {
    if plan.is_empty() {
        // Nothing to dial: the phone is not where it announced.
        match method {
            PairingMethod::QrCode => {
                ctl.phase = Phase::Qr {
                    last_failure: Some(PairingFailure::PhoneNotFound),
                };
            }
            PairingMethod::Sas => ctl.back_to_search(PairingFailure::PhoneNotFound),
        }
        emit(core, ctl);
        return;
    }
    ctl.dialing
        .extend(plan.iter().map(|(address, _)| address.ip()));
    // A connection that starts while another runs leaves the screen to it.
    if !matches!(
        ctl.phase,
        Phase::Connecting { .. } | Phase::Verifying { .. }
    ) {
        ctl.phase = Phase::Connecting {
            method,
            phone: phone.clone(),
        };
        emit(core, ctl);
    }
    let offer = ctl.offer.clone();
    core.handle.spawn(run::pair(
        core.clone(),
        run::Job {
            generation: ctl.generation,
            token: ctl.token.clone(),
            method,
            plan,
            offer,
            asked,
        },
    ));
}

/// A pairing connection ended without a pairing. `proven` says that its
/// peer proved it knows the QR code's secret, or that this desktop failed;
/// only then does the QR offer end. Anyone can announce the pairing tag, so
/// any other failure says nothing about the phone that scanned the code.
fn connection_failed(core: &Core, job: &run::Job, reason: PairingFailure, proven: bool) {
    finish_with(core, job.generation, |ctl| {
        for (address, _) in &job.plan {
            ctl.dialing.remove(&address.ip());
        }
        match job.method {
            PairingMethod::Sas => ctl.back_to_search(reason),
            PairingMethod::QrCode if proven => end_offer(core, ctl, reason),
            PairingMethod::QrCode => {
                let withdrawn =
                    job.offer
                        .as_ref()
                        .zip(ctl.offer.as_ref())
                        .is_some_and(|(used, shown)| {
                            Arc::ptr_eq(used, shown)
                                && lock(shown).status(Instant::now()) == OfferStatus::Withdrawn
                        });
                if withdrawn {
                    next_offer(core, ctl);
                }
                let connecting = matches!(
                    ctl.phase,
                    Phase::Connecting { .. } | Phase::Verifying { .. }
                );
                if connecting && ctl.dialing.is_empty() {
                    ctl.phase = Phase::Qr {
                        last_failure: job.asked.then_some(reason),
                    };
                }
            }
        }
    });
}

/// Makes the offer number `number` and shows it.
fn show_offer(core: &Core, ctl: &mut PairingCtl, number: u8) {
    let beacon_port = core.beacon_port.load(std::sync::atomic::Ordering::Relaxed);
    let mut addresses: Vec<SocketAddr> = core
        .locals()
        .into_iter()
        .filter(|local| !local.ip.is_loopback())
        .map(|local| SocketAddr::new(local.ip, beacon_port))
        .collect();
    // IPv4 first: the code has room for few addresses.
    addresses.sort_by_key(SocketAddr::is_ipv6);
    addresses.truncate(MAX_QR_ADDRESSES);
    let ble = *lock(&core.ble_address);
    let offer = QrOffer::new(
        core.identity.fingerprint(),
        core.desktop_name.clone(),
        addresses,
        ble,
    );
    let svg = QrImage(qr::svg(offer.uri()).unwrap_or_default());
    *lock(&core.offer_secret) = Some(SecretBytes::new(offer.secret().as_bytes().to_vec()));
    let expires = offer.expires_at();
    ctl.offer = Some(Arc::new(Mutex::new(offer)));
    ctl.code = Some(Code {
        svg,
        expires,
        number,
    });
    // While connections run, the screen goes on showing them.
    if ctl.dialing.is_empty() {
        ctl.phase = Phase::Qr { last_failure: None };
    }
    core.publish_keys();
}

/// Shows the next QR code, or gives up once [`MAX_OFFERS`] were shown.
fn next_offer(core: &Core, ctl: &mut PairingCtl) {
    match ctl.code.as_ref().map(|code| code.number) {
        Some(number) if number < MAX_OFFERS => show_offer(core, ctl, number + 1),
        _ => end_offer(core, ctl, PairingFailure::Expired),
    }
}

/// Ends a QR activation with `reason`, closing its other connections.
fn end_offer(core: &Core, ctl: &mut PairingCtl, reason: PairingFailure) {
    ctl.reset();
    ctl.phase = Phase::Failed {
        method: PairingMethod::QrCode,
        reason,
    };
    clear_offer_secret(core);
}

pub(super) fn clear_offer_secret(core: &Core) {
    *lock(&core.offer_secret) = None;
    core.publish_keys();
}

/// Replaces the QR offer shortly before it expires, [`MAX_OFFERS`] times,
/// and then gives up.
async fn renew_offers(
    core: Arc<Core>,
    generation: u64,
    token: tokio_util::sync::CancellationToken,
) {
    loop {
        let (number, due) = {
            let ctl = lock(&core.pairing);
            let Some(code) = ctl.code.as_ref() else {
                return;
            };
            if ctl.generation != generation {
                return;
            }
            let due = if code.number < MAX_OFFERS {
                code.expires
                    .checked_sub(OFFER_REFRESH_BEFORE)
                    .unwrap_or(code.expires)
            } else {
                code.expires
            };
            (code.number, due)
        };
        tokio::select! {
            () = token.cancelled() => return,
            () = time::sleep_until(due) => {}
        }
        let mut ctl = lock(&core.pairing);
        // A withdrawn offer may have been replaced in the meantime.
        if ctl.generation == generation && ctl.code.as_ref().map(|code| code.number) == Some(number)
        {
            next_offer(&core, &mut ctl);
            emit(&core, &ctl);
        }
    }
}

/// Reports `state` after a task of generation `generation` finished.
pub(super) fn finish_with(
    core: &Core,
    generation: u64,
    change: impl FnOnce(&mut PairingCtl),
) -> bool {
    let mut ctl = lock(&core.pairing);
    if ctl.generation != generation {
        return false;
    }
    change(&mut ctl);
    emit(core, &ctl);
    true
}
