//! The desktop's side of pairing, on a connection it dialed to pair.
//!
//! Pairing runs after the `Hello` exchange, on the envelopes of the same
//! connection, and the connection continues as a session once it succeeds.
//! The session layer provides the envelopes through a [`PairingChannel`],
//! and the user interface answers questions through a [`PairingPrompt`].
//!
//! The order of `PairingResult` is fixed: the phone accepts first, then the
//! desktop stores the pairing and sends its own acceptance. The new record
//! stays unconfirmed until the phone's first `FeatureStatus`
//! ([`records::confirm`](super::records::confirm)).
//!

use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use link2linux_protocol::Payload;
use link2linux_protocol::v1::PairingMethod;
use zeroize::Zeroize;

use super::crypto::{self, EKM_LEN};
use super::{CertificateFingerprint, PairedDevice, PairingCode, PairingError};
use crate::device::DeviceInfo;
use crate::transport::Connection;

/// The envelopes of a pairing connection after the `Hello` exchange.
///
/// The session layer implements it: it numbers and checks sequence numbers,
/// answers liveness messages itself, and hands every other payload over.
pub trait PairingChannel: Send {
    /// Sends one payload to the phone.
    fn send(&mut self, payload: Payload) -> impl Future<Output = Result<(), PairingError>> + Send;

    /// Waits for the next payload from the phone, failing with
    /// [`PairingError::Closed`] once it closed the connection.
    ///
    /// Must be cancel safe: dropping the future before it completes must not
    /// lose a payload.
    fn recv(&mut self) -> impl Future<Output = Result<Payload, PairingError>> + Send;
}

/// The user's answer to the SAS code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SasDecision {
    /// The codes on both screens match.
    Confirmed,
    /// They differ, or the user cancelled.
    Declined,
}

/// Questions pairing asks the desktop's user.
pub trait PairingPrompt: Send {
    /// Shows the SAS code (with [`PairingCode::grouped`]) and the phone's
    /// name, and waits for the user. `replaces` lists records the pairing
    /// replaces: the same device id under another key; show them too.
    /// Dropping the future means the question is withdrawn.
    fn confirm_code(
        &mut self,
        code: &PairingCode,
        phone: &DeviceInfo,
        replaces: &[PairedDevice],
    ) -> impl Future<Output = SasDecision> + Send;

    /// Asks whether a QR code pairing with `phone` replaces `replaces`,
    /// records with the same device id under another key (name, kind, date
    /// paired). `false` declines the pairing.
    fn confirm_replace(
        &mut self,
        phone: &DeviceInfo,
        replaces: &[PairedDevice],
    ) -> impl Future<Output = bool> + Send;
}

/// Time limits of a pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairingTimeouts {
    /// Each protocol step: 10 seconds.
    pub step: Duration,
    /// The user's decision: 60 seconds.
    pub confirm: Duration,
}

impl Default for PairingTimeouts {
    fn default() -> Self {
        Self {
            step: Duration::from_secs(10),
            confirm: Duration::from_secs(60),
        }
    }
}

/// What ties a pairing to its TLS connection: the method, both
/// fingerprints, the phone's address, and the exporter value `ekm`.
pub struct PairingBinding {
    method: PairingMethod,
    desktop: CertificateFingerprint,
    phone: CertificateFingerprint,
    peer_addr: SocketAddr,
    ekm: [u8; EKM_LEN],
}

impl PairingBinding {
    /// Derives the binding of a connection the desktop dialed, right after
    /// the TLS handshake and before the connection is handed to the
    /// session.
    ///
    /// # Errors
    ///
    /// Fails with [`PairingError::Exporter`] if keying material cannot be
    /// exported, or the method is unspecified. The desktop must not pair
    /// then: after the `Hello` exchange it sends
    /// `PAIRING_OUTCOME_UNSUPPORTED`.
    pub fn from_connection<C>(connection: &C, method: PairingMethod) -> Result<Self, PairingError>
    where
        C: Connection + ?Sized,
    {
        let label = crypto::exporter_label(method)
            .ok_or_else(|| PairingError::Exporter("no pairing method".into()))?;
        let desktop = connection.local_fingerprint();
        let phone = connection.peer_fingerprint();
        let mut ekm = [0u8; EKM_LEN];
        connection
            .export_keying_material(label, &crypto::exporter_context(&desktop, &phone), &mut ekm)
            .map_err(|error| PairingError::Exporter(error.to_string()))?;
        Ok(Self::from_parts(
            method,
            desktop,
            phone,
            connection.peer_addr(),
            ekm,
        ))
    }

    /// A binding from known values, for the phone side of tests.
    #[must_use]
    pub fn from_parts(
        method: PairingMethod,
        desktop: CertificateFingerprint,
        phone: CertificateFingerprint,
        peer_addr: SocketAddr,
        ekm: [u8; EKM_LEN],
    ) -> Self {
        Self {
            method,
            desktop,
            phone,
            peer_addr,
            ekm,
        }
    }

    /// The pairing method the exporter value was derived for.
    #[must_use]
    pub fn method(&self) -> PairingMethod {
        self.method
    }

    /// The desktop's fingerprint, `fp_D`.
    #[must_use]
    pub fn desktop(&self) -> CertificateFingerprint {
        self.desktop
    }

    /// The phone's fingerprint, `fp_P`.
    #[must_use]
    pub fn phone(&self) -> CertificateFingerprint {
        self.phone
    }

    /// The peer's address.
    #[must_use]
    pub fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }

    /// The exporter value, `ekm`.
    #[must_use]
    pub fn ekm(&self) -> &[u8; EKM_LEN] {
        &self.ekm
    }
}

impl Drop for PairingBinding {
    fn drop(&mut self) {
        self.ekm.zeroize();
    }
}

impl fmt::Debug for PairingBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingBinding")
            .field("method", &self.method)
            .field("desktop", &self.desktop)
            .field("phone", &self.phone)
            .field("peer_addr", &self.peer_addr)
            .finish_non_exhaustive()
    }
}
