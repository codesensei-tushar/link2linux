//! The pairing state machine's data: the phase, what the UI is shown for it,
//! the phones found in pairing mode, and how errors map to what the user
//! reads.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use link2linux_core::pairing::{OfferStatus, PairingError, PairingOutcome, QrOffer};
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::view::{
    BackendError, Device, DiscoveredPhone, PairingFailure, PairingMethod, PairingState,
};

/// The port phones listen on unless the user types another.
pub const DEFAULT_PHONE_PORT: u16 = 27380;
/// A new QR offer replaces the old one this long before it expires.
pub const OFFER_REFRESH_BEFORE: Duration = Duration::from_secs(20);
/// Offers shown before giving up with `expired`.
pub const MAX_OFFERS: u8 = 3;
/// How long a code comparison can take.
pub const SAS_LIFETIME: Duration = Duration::from_secs(60);
/// Code comparisons allowed per activation.
pub const SAS_MAX_ATTEMPTS: u8 = 3;

/// Where a pairing activation stands.
#[derive(Debug)]
pub enum Phase {
    Idle,
    Qr {
        uri: String,
        svg: String,
        expires: Instant,
        /// Which offer of [`MAX_OFFERS`] this is.
        offer: u8,
    },
    Searching {
        last_failure: Option<PairingFailure>,
    },
    Connecting {
        method: PairingMethod,
        phone: Option<String>,
    },
    Verifying {
        method: PairingMethod,
        phone: Option<String>,
    },
    Sas {
        code: String,
        phone: String,
        expires: Instant,
        confirmed_here: bool,
    },
    Paired(Device),
    Failed {
        method: PairingMethod,
        reason: PairingFailure,
    },
}

impl Phase {
    /// Whether a new activation may start.
    pub fn is_startable(&self) -> bool {
        matches!(self, Self::Idle | Self::Paired(_) | Self::Failed { .. })
    }
}

/// Everything about the current activation.
pub struct PairingCtl {
    /// Changes whenever an activation ends, so late results of an old one
    /// are dropped.
    pub generation: u64,
    pub phase: Phase,
    pub attempts_left: u8,
    /// Answers the question "do the codes match?".
    pub answer: Option<oneshot::Sender<bool>>,
    /// Ends the tasks of the activation.
    pub token: CancellationToken,
    /// The QR offer being shown.
    pub offer: Option<Arc<Mutex<QrOffer>>>,
    /// A pairing connection is open.
    pub dialing: bool,
}

impl Default for PairingCtl {
    fn default() -> Self {
        Self {
            generation: 0,
            phase: Phase::Idle,
            attempts_left: SAS_MAX_ATTEMPTS,
            answer: None,
            token: CancellationToken::new(),
            offer: None,
            dialing: false,
        }
    }
}

impl PairingCtl {
    /// Ends the current activation and starts a new generation.
    pub fn reset(&mut self) {
        self.generation += 1;
        self.token.cancel();
        self.token = CancellationToken::new();
        self.answer = None;
        self.offer = None;
        self.dialing = false;
    }

    /// What the UI is shown at `now`.
    pub fn view(&self, now: Instant, desktop_name: &str, phones: &PhoneList) -> PairingState {
        let left = |expires: Instant| {
            u32::try_from(expires.saturating_duration_since(now).as_millis()).unwrap_or(u32::MAX)
        };
        match &self.phase {
            Phase::Idle => PairingState::Idle,
            Phase::Qr {
                uri, svg, expires, ..
            } => PairingState::QrOffer {
                uri: uri.clone(),
                svg: svg.clone(),
                desktop_name: desktop_name.to_owned(),
                expires_in_ms: left(*expires),
                lifetime_ms: u32::try_from(link2linux_core::pairing::OFFER_LIFETIME.as_millis())
                    .unwrap_or(u32::MAX),
            },
            Phase::Searching { last_failure } => PairingState::Searching {
                phones: phones.phones(),
                attempts_left: self.attempts_left,
                last_failure: *last_failure,
            },
            Phase::Connecting { method, phone } => PairingState::Connecting {
                method: *method,
                phone_name: phone.clone(),
            },
            Phase::Verifying { method, phone } => PairingState::Verifying {
                method: *method,
                phone_name: phone.clone(),
            },
            Phase::Sas {
                code,
                phone,
                expires,
                confirmed_here,
            } => PairingState::SasCode {
                code: code.clone(),
                phone_name: phone.clone(),
                expires_in_ms: left(*expires),
                lifetime_ms: u32::try_from(SAS_LIFETIME.as_millis()).unwrap_or(u32::MAX),
                confirmed_here: *confirmed_here,
            },
            Phase::Paired(device) => PairingState::Paired {
                device: device.clone(),
            },
            Phase::Failed { method, reason } => PairingState::Failed {
                method: *method,
                reason: *reason,
            },
        }
    }

    /// A code comparison attempt ended. Go back to the list while attempts
    /// remain; the user has to start again once they are used up.
    pub fn back_to_search(&mut self, reason: PairingFailure) {
        self.phase = if self.attempts_left == 0 {
            Phase::Failed {
                method: PairingMethod::Sas,
                reason: PairingFailure::TooManyAttempts,
            }
        } else {
            Phase::Searching {
                last_failure: Some(reason),
            }
        };
    }
}

/// Phones that announce pairing mode, by mDNS instance name.
#[derive(Debug, Default)]
pub struct PhoneList {
    entries: Vec<(String, Vec<SocketAddr>)>,
}

impl PhoneList {
    /// Adds a phone or updates its addresses.
    pub fn upsert(&mut self, instance: &str, addresses: Vec<SocketAddr>) {
        match self.entries.iter_mut().find(|(known, _)| known == instance) {
            Some((_, known)) => *known = addresses,
            None => self.entries.push((instance.to_owned(), addresses)),
        }
    }

    /// Removes a phone that stopped announcing. Returns whether it was
    /// listed.
    pub fn remove(&mut self, instance: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(known, _)| known != instance);
        self.entries.len() != before
    }

    /// The phones, for showing. The instance name is what the phone shows
    /// on its own screen, so it is both the id and the name.
    pub fn phones(&self) -> Vec<DiscoveredPhone> {
        self.entries
            .iter()
            .map(|(instance, addresses)| DiscoveredPhone {
                id: instance.clone(),
                name: instance.clone(),
                address: addresses
                    .first()
                    .map(|address| address.to_string())
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// The addresses of the phone with this id.
    pub fn addresses(&self, id: &str) -> Option<Vec<SocketAddr>> {
        self.entries
            .iter()
            .find(|(instance, _)| instance == id)
            .map(|(_, addresses)| addresses.clone())
    }
}

/// Reads an address the user typed: `192.168.1.20`, `192.168.1.20:27380`,
/// `fd00::2` or `[fd00::2]:27380`. The default port is 27380.
pub fn parse_address(text: &str) -> Result<SocketAddr, BackendError> {
    let text = text.trim();
    if let Ok(address) = text.parse::<SocketAddr>() {
        return Ok(address);
    }
    text.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .map(|ip| SocketAddr::new(ip, DEFAULT_PHONE_PORT))
        .map_err(|_| BackendError::InvalidAddress)
}

/// What the user reads for an error of pairing `method`.
pub fn failure_of(error: &PairingError, method: PairingMethod) -> PairingFailure {
    match error {
        PairingError::Declined => match method {
            // The user said the codes differ.
            PairingMethod::Sas => PairingFailure::VerificationFailed,
            PairingMethod::QrCode => PairingFailure::Rejected,
        },
        PairingError::TimedOut => PairingFailure::TimedOut,
        PairingError::Rejected(outcome) => match outcome {
            PairingOutcome::Declined => PairingFailure::DeclinedByPhone,
            PairingOutcome::VerificationFailed => PairingFailure::VerificationFailed,
            PairingOutcome::NotExpected => PairingFailure::NotExpected,
            PairingOutcome::TimedOut => PairingFailure::TimedOut,
            PairingOutcome::Unsupported => PairingFailure::Unsupported,
            _ => PairingFailure::Rejected,
        },
        PairingError::VerificationFailed => PairingFailure::VerificationFailed,
        PairingError::OfferUnavailable(OfferStatus::Expired) => PairingFailure::Expired,
        PairingError::OfferUnavailable(_) | PairingError::Blocked => PairingFailure::NotExpected,
        PairingError::Exporter(_) => PairingFailure::Unsupported,
        PairingError::Closed
        | PairingError::Connection(_)
        | PairingError::OutOfOrder(_)
        | PairingError::Protocol(_) => PairingFailure::ConnectionLost,
        _ => PairingFailure::Rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_addresses_get_the_default_port() {
        let ok = |text: &str| parse_address(text).unwrap().to_string();
        assert_eq!(ok("192.168.1.20"), "192.168.1.20:27380");
        assert_eq!(ok(" 192.168.1.20:4000 "), "192.168.1.20:4000");
        assert_eq!(ok("fd00::2"), "[fd00::2]:27380");
        assert_eq!(ok("[fd00::2]"), "[fd00::2]:27380");
        assert_eq!(ok("[fd00::2]:5"), "[fd00::2]:5");
        for bad in ["", "phone.local", "192.168.1", "1.2.3.4:x"] {
            assert_eq!(
                parse_address(bad),
                Err(BackendError::InvalidAddress),
                "{bad}"
            );
        }
    }

    #[test]
    fn errors_map_to_what_the_user_reads() {
        use PairingFailure as F;
        use PairingMethod as M;
        let of = failure_of;
        assert_eq!(of(&PairingError::Declined, M::Sas), F::VerificationFailed);
        assert_eq!(of(&PairingError::Declined, M::QrCode), F::Rejected);
        assert_eq!(
            of(&PairingError::Rejected(PairingOutcome::Declined), M::Sas),
            F::DeclinedByPhone
        );
        assert_eq!(
            of(
                &PairingError::Rejected(PairingOutcome::NotExpected),
                M::QrCode
            ),
            F::NotExpected
        );
        assert_eq!(
            of(
                &PairingError::OfferUnavailable(OfferStatus::Expired),
                M::QrCode
            ),
            F::Expired
        );
        assert_eq!(of(&PairingError::Blocked, M::QrCode), F::NotExpected);
        assert_eq!(of(&PairingError::Closed, M::Sas), F::ConnectionLost);
        assert_eq!(
            of(&PairingError::VerificationFailed, M::QrCode),
            F::VerificationFailed
        );
        assert_eq!(
            of(&PairingError::Exporter("x".into()), M::QrCode),
            F::Unsupported
        );
    }

    #[test]
    fn the_phone_list_tracks_announcements() {
        let a: SocketAddr = "192.168.1.9:27380".parse().unwrap();
        let b: SocketAddr = "192.168.1.10:27380".parse().unwrap();
        let mut list = PhoneList::default();
        list.upsert("l2l-1", vec![a]);
        list.upsert("l2l-2", vec![b]);
        list.upsert("l2l-1", vec![b]);
        let phones = list.phones();
        assert_eq!(phones.len(), 2);
        assert_eq!(phones[0].address, "192.168.1.10:27380");
        assert_eq!(list.addresses("l2l-2"), Some(vec![b]));
        assert!(list.remove("l2l-1") && !list.remove("l2l-1"));
        assert_eq!(list.addresses("l2l-1"), None);
    }

    #[test]
    fn attempts_run_out_after_the_last_comparison() {
        let mut ctl = PairingCtl {
            attempts_left: 1,
            ..PairingCtl::default()
        };
        ctl.back_to_search(PairingFailure::TimedOut);
        assert!(matches!(
            ctl.phase,
            Phase::Searching {
                last_failure: Some(PairingFailure::TimedOut)
            }
        ));
        ctl.attempts_left = 0;
        ctl.back_to_search(PairingFailure::TimedOut);
        assert!(matches!(
            ctl.phase,
            Phase::Failed {
                reason: PairingFailure::TooManyAttempts,
                ..
            }
        ));
    }

    #[test]
    fn a_reset_starts_a_new_generation_and_cancels_the_old_tasks() {
        let mut ctl = PairingCtl::default();
        let old = ctl.token.clone();
        ctl.dialing = true;
        ctl.reset();
        assert!(old.is_cancelled() && !ctl.token.is_cancelled());
        assert_eq!((ctl.generation, ctl.dialing), (1, false));
    }
}
