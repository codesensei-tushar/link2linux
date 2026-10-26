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

use link2linux_core::discovery::onlink::is_on_link;
use link2linux_core::discovery::reconnect::dial_plan;
use link2linux_core::pairing::{MAX_QR_ADDRESSES, QrOffer};
use tokio::time::{self, Instant};

use crate::link::lock;
use crate::qr;
use crate::shared::Core;
use crate::view::{BackendError, PairingFailure, PairingMethod, PairingState, PhoneTarget};

use state::{MAX_OFFERS, OFFER_REFRESH_BEFORE, Phase, SAS_MAX_ATTEMPTS};
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
    if ctl.dialing {
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
    begin_connection(core, &mut ctl, method, plan, name);
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
pub(crate) fn offer_seen(core: &Arc<Core>, addresses: &[SocketAddr]) {
    let mut ctl = lock(&core.pairing);
    if !matches!(ctl.phase, Phase::Qr { .. }) || ctl.dialing {
        return;
    }
    let Some(offer) = ctl.offer.clone() else {
        return;
    };
    let locals = core.locals();
    let usable: Vec<SocketAddr> = addresses
        .iter()
        .copied()
        .filter(|address| {
            is_on_link(*address, &locals)
                && offer.lock().is_ok_and(|offer| offer.may_dial(address.ip()))
        })
        .collect();
    let plan = dial_plan(None, &usable, &[]);
    if plan.is_empty() {
        return;
    }
    begin_connection(core, &mut ctl, PairingMethod::QrCode, plan, None);
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

fn begin_connection(
    core: &Arc<Core>,
    ctl: &mut PairingCtl,
    method: PairingMethod,
    plan: crate::candidates::Plan,
    phone: Option<String>,
) {
    if plan.is_empty() {
        // Nothing to dial: the phone is not where it announced.
        match method {
            PairingMethod::QrCode => {
                ctl.phase = Phase::Failed {
                    method,
                    reason: PairingFailure::PhoneNotFound,
                };
                ctl.offer = None;
                clear_offer_secret(core);
            }
            PairingMethod::Sas => ctl.back_to_search(PairingFailure::PhoneNotFound),
        }
        emit(core, ctl);
        return;
    }
    ctl.dialing = true;
    ctl.phase = Phase::Connecting {
        method,
        phone: phone.clone(),
    };
    emit(core, ctl);
    let offer = ctl.offer.clone();
    core.handle.spawn(run::pair(
        core.clone(),
        run::Job {
            generation: ctl.generation,
            token: ctl.token.clone(),
            method,
            plan,
            offer,
        },
    ));
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
    let uri = offer.uri().to_owned();
    let svg = qr::svg(&uri).unwrap_or_default();
    *lock(&core.offer_secret) = Some(offer.secret().as_bytes().to_vec());
    let expires = offer.expires_at();
    ctl.offer = Some(Arc::new(Mutex::new(offer)));
    ctl.phase = Phase::Qr {
        uri,
        svg,
        expires,
        offer: number,
    };
    core.publish_keys();
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
        let due = {
            let ctl = lock(&core.pairing);
            match &ctl.phase {
                Phase::Qr { expires, offer, .. } if ctl.generation == generation => {
                    if *offer < MAX_OFFERS {
                        expires
                            .checked_sub(OFFER_REFRESH_BEFORE)
                            .unwrap_or(*expires)
                    } else {
                        *expires
                    }
                }
                _ => return,
            }
        };
        tokio::select! {
            () = token.cancelled() => return,
            () = time::sleep_until(due) => {}
        }
        let mut ctl = lock(&core.pairing);
        let Phase::Qr { offer, .. } = &ctl.phase else {
            return;
        };
        if ctl.generation != generation {
            return;
        }
        let number = *offer;
        if number < MAX_OFFERS {
            show_offer(&core, &mut ctl, number + 1);
        } else {
            ctl.phase = Phase::Failed {
                method: PairingMethod::QrCode,
                reason: PairingFailure::Expired,
            };
            ctl.offer = None;
            clear_offer_secret(&core);
        }
        emit(&core, &ctl);
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
