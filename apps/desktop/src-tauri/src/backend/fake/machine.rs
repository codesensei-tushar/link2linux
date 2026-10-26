//! The pairing and device state machine behind the fake backend.

use super::script::{
    CONNECT_MS, Changed, DISCOVERY_DELAY_MS, MAX_OFFERS, Now, OFFER_LIFETIME_MS,
    OFFER_REFRESH_BEFORE_MS, PHONE_CONFIRM_MS, SAS_LIFETIME_MS, SAS_MAX_ATTEMPTS, SCAN_DELAY_MS,
    Script, VERIFY_MS,
};
use super::support::{PHONE_NAME, demo_device, offer_uri, pretend_phones, typed_phone};
use crate::backend::{
    BackendError, Device, DeviceStatus, DiscoveredPhone, PairingFailure, PairingMethod,
    PairingState, PhoneTarget,
};
use crate::qr;

#[derive(Debug, Clone)]
enum Phase {
    Idle,
    Qr {
        uri: String,
        svg: String,
        offer: u8,
        expires: u64,
        /// When a pretend phone scans the code.
        scan_at: Option<u64>,
    },
    Searching {
        last_failure: Option<PairingFailure>,
        /// When the pretend phones show up.
        appear_at: Option<u64>,
    },
    Connecting {
        method: PairingMethod,
        phone: Option<String>,
        until: u64,
    },
    Verifying {
        method: PairingMethod,
        phone: Option<String>,
        until: u64,
    },
    Sas {
        code: String,
        phone: String,
        expires: u64,
        /// When the phone's side of the confirmation arrives, once we confirmed.
        phone_confirms_at: Option<u64>,
    },
    Paired(Device),
    Failed {
        method: PairingMethod,
        reason: PairingFailure,
    },
}

/// The pairing and device state machine behind [`FakeBackend`].
#[derive(Debug)]
pub struct Machine {
    script: Script,
    desktop_name: String,
    phase: Phase,
    devices: Vec<Device>,
    phones: Vec<DiscoveredPhone>,
    sas_attempts_left: u8,
    next_id: u64,
    next_offer_seed: u64,
}

impl Machine {
    pub fn new(script: Script, desktop_name: impl Into<String>, now: Now) -> Self {
        let mut machine = Self {
            script,
            desktop_name: desktop_name.into(),
            phase: Phase::Idle,
            devices: Vec::new(),
            phones: Vec::new(),
            sas_attempts_left: SAS_MAX_ATTEMPTS,
            next_id: 1,
            next_offer_seed: 1,
        };
        if script == Script::Demo {
            let day = 86_400_000;
            machine.devices = vec![
                demo_device(
                    "demo-1",
                    "Pixel 9",
                    "Pixel 9",
                    DeviceStatus::Connected,
                    now,
                    3 * day,
                ),
                demo_device(
                    "demo-2",
                    "Old Galaxy",
                    "Galaxy S21",
                    DeviceStatus::Disconnected,
                    now,
                    40 * day,
                ),
            ];
        }
        machine
    }

    pub fn devices(&self) -> Vec<Device> {
        self.devices.clone()
    }

    /// The pairing state as the UI should see it at `now`.
    pub fn pairing(&self, now: Now) -> PairingState {
        let left =
            |expires: u64| u32::try_from(expires.saturating_sub(now.mono_ms)).unwrap_or(u32::MAX);
        match &self.phase {
            Phase::Idle => PairingState::Idle,
            Phase::Qr {
                uri, svg, expires, ..
            } => PairingState::QrOffer {
                uri: uri.clone(),
                svg: svg.clone(),
                desktop_name: self.desktop_name.clone(),
                expires_in_ms: left(*expires),
                lifetime_ms: OFFER_LIFETIME_MS,
            },
            Phase::Searching { last_failure, .. } => PairingState::Searching {
                phones: self.phones.clone(),
                attempts_left: self.sas_attempts_left,
                last_failure: *last_failure,
            },
            Phase::Connecting { method, phone, .. } => PairingState::Connecting {
                method: *method,
                phone_name: phone.clone(),
            },
            Phase::Verifying { method, phone, .. } => PairingState::Verifying {
                method: *method,
                phone_name: phone.clone(),
            },
            Phase::Sas {
                code,
                phone,
                expires,
                phone_confirms_at,
            } => PairingState::SasCode {
                code: code.clone(),
                phone_name: phone.clone(),
                expires_in_ms: left(*expires),
                lifetime_ms: SAS_LIFETIME_MS,
                confirmed_here: phone_confirms_at.is_some(),
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

    pub fn start_pairing(
        &mut self,
        method: PairingMethod,
        now: Now,
    ) -> Result<Changed, BackendError> {
        if !matches!(
            self.phase,
            Phase::Idle | Phase::Paired(_) | Phase::Failed { .. }
        ) {
            return Err(BackendError::WrongState);
        }
        match method {
            PairingMethod::QrCode => self.new_offer(1, now.mono_ms),
            PairingMethod::Sas => {
                self.sas_attempts_left = SAS_MAX_ATTEMPTS;
                self.phones.clear();
                self.phase = Phase::Searching {
                    last_failure: None,
                    appear_at: (self.script != Script::NotFound)
                        .then_some(now.mono_ms + DISCOVERY_DELAY_MS),
                };
            }
        }
        Ok(Changed {
            pairing: true,
            devices: false,
        })
    }

    pub fn pair_with(&mut self, target: PhoneTarget, now: Now) -> Result<Changed, BackendError> {
        let (method, phone) = match (&self.phase, &target) {
            (Phase::Qr { .. }, PhoneTarget::Address { address }) => {
                (PairingMethod::QrCode, typed_phone(address)?)
            }
            (Phase::Qr { .. }, PhoneTarget::Discovered { .. }) => {
                return Err(BackendError::WrongState);
            }
            (Phase::Searching { .. }, PhoneTarget::Address { address }) => {
                (PairingMethod::Sas, typed_phone(address)?)
            }
            (Phase::Searching { .. }, PhoneTarget::Discovered { id }) => {
                let found = self
                    .phones
                    .iter()
                    .find(|p| &p.id == id)
                    .ok_or(BackendError::NotFound)?;
                (PairingMethod::Sas, found.name.clone())
            }
            _ => return Err(BackendError::WrongState),
        };
        self.phase = Phase::Connecting {
            method,
            phone: Some(phone),
            until: now.mono_ms + CONNECT_MS,
        };
        Ok(Changed {
            pairing: true,
            devices: false,
        })
    }

    pub fn answer_code(&mut self, matches: bool, now: Now) -> Result<Changed, BackendError> {
        let Phase::Sas {
            phone_confirms_at, ..
        } = &mut self.phase
        else {
            return Err(BackendError::WrongState);
        };
        if phone_confirms_at.is_some() {
            return Err(BackendError::WrongState);
        }
        if matches {
            // The phone answers a moment later. With the `wrong-code` script
            // it compared a different code and says no.
            *phone_confirms_at = Some(now.mono_ms + PHONE_CONFIRM_MS);
        } else {
            self.back_to_search(PairingFailure::VerificationFailed);
        }
        Ok(Changed {
            pairing: true,
            devices: false,
        })
    }

    pub fn cancel_pairing(&mut self) -> Changed {
        let was_idle = matches!(self.phase, Phase::Idle);
        self.phase = Phase::Idle;
        Changed {
            pairing: !was_idle,
            devices: false,
        }
    }

    pub fn unpair(&mut self, device_id: &str) -> Result<Changed, BackendError> {
        let before = self.devices.len();
        self.devices.retain(|d| d.id != device_id);
        if self.devices.len() == before {
            return Err(BackendError::NotFound);
        }
        Ok(Changed {
            pairing: false,
            devices: true,
        })
    }

    /// When the machine next needs to run, as a monotonic reading.
    pub fn next_deadline(&self) -> Option<u64> {
        match &self.phase {
            Phase::Idle | Phase::Paired(_) | Phase::Failed { .. } => None,
            Phase::Qr {
                expires,
                scan_at,
                offer,
                ..
            } => {
                let renew = if *offer < MAX_OFFERS {
                    expires.saturating_sub(u64::from(OFFER_REFRESH_BEFORE_MS))
                } else {
                    *expires
                };
                Some(scan_at.map_or(renew, |scan| scan.min(renew)))
            }
            Phase::Searching { appear_at, .. } => *appear_at,
            Phase::Connecting { until, .. } | Phase::Verifying { until, .. } => Some(*until),
            Phase::Sas {
                expires,
                phone_confirms_at,
                ..
            } => Some(phone_confirms_at.map_or(*expires, |at| at.min(*expires))),
        }
    }

    /// Runs everything due at or before `now`.
    pub fn tick(&mut self, now: Now) -> Changed {
        let mut changed = Changed::default();
        while let Some(due) = self.next_deadline() {
            if due > now.mono_ms {
                break;
            }
            changed.merge(self.step(now.at(due)));
        }
        changed
    }

    fn step(&mut self, at: Now) -> Changed {
        let mut changed = Changed {
            pairing: true,
            devices: false,
        };
        match self.phase.clone() {
            Phase::Qr {
                offer,
                expires,
                scan_at,
                ..
            } => {
                if scan_at.is_some_and(|scan| scan <= at.mono_ms) {
                    self.phase = Phase::Connecting {
                        method: PairingMethod::QrCode,
                        phone: Some(PHONE_NAME.to_owned()),
                        until: at.mono_ms + CONNECT_MS,
                    };
                } else if offer < MAX_OFFERS {
                    self.new_offer(offer + 1, at.mono_ms);
                } else {
                    debug_assert!(at.mono_ms >= expires);
                    self.phase = Phase::Failed {
                        method: PairingMethod::QrCode,
                        reason: PairingFailure::Expired,
                    };
                }
            }
            Phase::Searching { last_failure, .. } => {
                self.phones = pretend_phones();
                self.phase = Phase::Searching {
                    last_failure,
                    appear_at: None,
                };
            }
            Phase::Connecting { method, phone, .. } => match (method, self.script) {
                (_, Script::NotFound) => self.fail(method, PairingFailure::PhoneNotFound),
                (_, Script::Declined) => self.fail(method, PairingFailure::DeclinedByPhone),
                (PairingMethod::QrCode, _) => {
                    self.phase = Phase::Verifying {
                        method,
                        phone,
                        until: at.mono_ms + VERIFY_MS,
                    };
                }
                (PairingMethod::Sas, _) => {
                    // The phone has revealed its nonce: this attempt counts.
                    self.sas_attempts_left = self.sas_attempts_left.saturating_sub(1);
                    self.phase = Phase::Sas {
                        code: format!(
                            "{:06}",
                            100_000 + self.next_offer_seed.wrapping_mul(7919) % 900_000
                        ),
                        phone: phone.unwrap_or_else(|| PHONE_NAME.to_owned()),
                        expires: at.mono_ms + u64::from(SAS_LIFETIME_MS),
                        phone_confirms_at: None,
                    };
                    self.next_offer_seed += 1;
                }
            },
            Phase::Verifying { method, phone, .. } => {
                if self.script == Script::WrongCode {
                    self.fail(method, PairingFailure::VerificationFailed);
                } else {
                    changed.merge(self.finish(phone, at));
                }
            }
            Phase::Sas {
                phone,
                expires,
                phone_confirms_at,
                ..
            } => {
                if phone_confirms_at.is_some_and(|c| c <= at.mono_ms) {
                    if self.script == Script::WrongCode {
                        self.back_to_search(PairingFailure::VerificationFailed);
                    } else {
                        changed.merge(self.finish(Some(phone), at));
                    }
                } else {
                    debug_assert!(at.mono_ms >= expires);
                    self.back_to_search(PairingFailure::TimedOut);
                }
            }
            Phase::Idle | Phase::Paired(_) | Phase::Failed { .. } => changed.pairing = false,
        }
        changed
    }

    fn new_offer(&mut self, offer: u8, now_ms: u64) {
        let seed = self.next_offer_seed;
        self.next_offer_seed += 1;
        let uri = offer_uri(seed, &self.desktop_name);
        // A URI built here always fits a code; an empty image only means the
        // fake could not draw one.
        let svg = qr::svg(&uri).unwrap_or_default();
        let scan = matches!(
            self.script,
            Script::Success | Script::Demo | Script::Declined | Script::WrongCode
        );
        self.phase = Phase::Qr {
            uri,
            svg,
            offer,
            expires: now_ms + u64::from(OFFER_LIFETIME_MS),
            scan_at: (scan && offer == 1).then_some(now_ms + SCAN_DELAY_MS),
        };
    }

    fn fail(&mut self, method: PairingMethod, reason: PairingFailure) {
        match method {
            PairingMethod::QrCode => self.phase = Phase::Failed { method, reason },
            // Failures before the phone reveals its nonce cost no attempt.
            PairingMethod::Sas => self.back_to_search(reason),
        }
    }

    /// A code comparison attempt ended. Go back to the list while attempts
    /// remain; the user has to start again once they are used up.
    fn back_to_search(&mut self, reason: PairingFailure) {
        if self.sas_attempts_left == 0 {
            self.phase = Phase::Failed {
                method: PairingMethod::Sas,
                reason: PairingFailure::TooManyAttempts,
            };
        } else {
            self.phase = Phase::Searching {
                last_failure: Some(reason),
                appear_at: None,
            };
        }
    }

    fn finish(&mut self, phone: Option<String>, at: Now) -> Changed {
        let id = format!("phone-{}", self.next_id);
        self.next_id += 1;
        let device = Device {
            id,
            name: phone.unwrap_or_else(|| PHONE_NAME.to_owned()),
            model: Some("Pixel 9".to_owned()),
            status: DeviceStatus::Connected,
            paired_at_unix_ms: at.unix_ms,
            last_connected_unix_ms: Some(at.unix_ms),
        };
        self.devices.push(device.clone());
        self.phase = Phase::Paired(device);
        Changed {
            pairing: true,
            devices: true,
        }
    }
}

#[cfg(test)]
mod sas_tests;
#[cfg(test)]
mod tests;
