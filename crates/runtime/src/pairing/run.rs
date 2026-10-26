//! The task of one pairing connection: dial, exchange `Hello`s, run the
//! pairing exchange, and hand the session over to the phone's own task.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use link2linux_core::device::DeviceInfo;
use link2linux_core::identity::CertificateFingerprint;
use link2linux_core::pairing::{
    PairedDevice, PairingBinding, PairingCode, PairingError, PairingOutcome, PairingPrompt,
    PairingSuccess, PairingTimeouts, QrOffer, SasDecision, TrustStore, pair_by_qr, pair_by_sas,
    records, refuse,
};
use link2linux_core::session::{SessionConfig, SessionError};
use link2linux_core::transport::{Connection, PeerPin};
use link2linux_protocol::v1::PairingMethod as WireMethod;
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::state::{Lead, Phase, SAS_LIFETIME, failure_of};
use super::{clear_offer_secret, connection_failed, finish_with};
use crate::candidates::Plan;
use crate::device::Established;
use crate::dial::race;
use crate::link::lock;
use crate::notify::PhoneSession;
use crate::shared::Core;
use crate::view::{PairingFailure, PairingMethod};

/// What a pairing connection is for.
pub(super) struct Job {
    /// Its id among the activation's connections.
    pub id: u64,
    pub generation: u64,
    /// Ends it: with the activation, or to make room for another.
    pub cancel: CancellationToken,
    pub method: PairingMethod,
    pub plan: Plan,
    pub offer: Option<Arc<Mutex<QrOffer>>>,
    pub lead: Lead,
}

/// A pairing that succeeded, with the connection it ran on.
struct Done {
    session: PhoneSession,
    address: SocketAddr,
    success: PairingSuccess,
}

enum Outcome {
    Paired(Box<Done>),
    /// The phone, or this desktop, ended the pairing.
    Failed(PairingFailure),
    /// The connection ended before its peer proved it knows the QR code's
    /// secret.
    Unproven(PairingFailure),
    /// Another connection had reached the same phone first. This one closed
    /// before its `Hello`, so the phone never saw it.
    Duplicate,
}

/// Runs one pairing connection to its end and reports the result.
pub(super) async fn pair(core: Arc<Core>, job: Job) {
    let outcome = tokio::select! {
        // The activation was cancelled, or this connection closed to make
        // room for another: it is dropped.
        () = job.cancel.cancelled() => return,
        outcome = connect_and_pair(&core, &job) => outcome,
    };
    match outcome {
        Outcome::Paired(done) => complete(&core, job.generation, *done),
        Outcome::Failed(reason) => connection_failed(&core, &job, reason, true),
        Outcome::Unproven(reason) => connection_failed(&core, &job, reason, false),
        Outcome::Duplicate => {
            connection_failed(&core, &job, PairingFailure::ConnectionLost, false);
        }
    }
}

async fn connect_and_pair(core: &Arc<Core>, job: &Job) -> Outcome {
    let method = job.method;
    let wire = match method {
        PairingMethod::QrCode => WireMethod::QrCode,
        PairingMethod::Sas => WireMethod::Sas,
    };
    let (connection, address) = match race(&core.dialer, job.plan.clone(), PeerPin::Pairing).await {
        Ok(winner) => winner,
        Err(error) => {
            tracing::debug!(%error, "could not reach the phone to pair");
            return Outcome::Unproven(PairingFailure::PhoneNotFound);
        }
    };
    let peer = connection.peer_fingerprint();
    // One connection per phone, and the addresses it did not reach are
    // free for others (`PairingCtl::connected`).
    let first = {
        let mut ctl = lock(&core.pairing);
        ctl.generation == job.generation && ctl.connected(job.id, address.ip(), peer)
    };
    if !first {
        return Outcome::Duplicate;
    }
    // Before the pairing exchange the peer has proven nothing: a failure
    // counts against its key and address for the rest of the QR offer.
    let unproven = |reason| {
        if let Some(offer) = &job.offer {
            lock(offer).record_failed_proof(peer, address.ip());
        }
        Outcome::Unproven(reason)
    };
    // The binding needs the connection, which the session then takes.
    let binding = PairingBinding::from_connection(&connection, wire);
    let mut start = core.session_start();
    start.pairing = Some(wire);
    let mut session =
        match PhoneSession::handshake(connection, start, SessionConfig::default()).await {
            Ok(session) => session,
            Err(SessionError::VersionMismatch { .. }) => {
                return unproven(PairingFailure::Unsupported);
            }
            Err(error) => {
                tracing::debug!(%error, "the pairing handshake failed");
                return unproven(PairingFailure::ConnectionLost);
            }
        };
    let phone: DeviceInfo = session.peer().clone();
    let name = Some(phone.name.clone());
    if method == PairingMethod::QrCode {
        finish_with(core, job.generation, |ctl| {
            ctl.phase = Phase::Verifying {
                method,
                phone: name.clone(),
            };
        });
    } else {
        finish_with(core, job.generation, |ctl| {
            ctl.phase = Phase::Connecting {
                method,
                phone: name.clone(),
            };
        });
    }

    let binding = match binding {
        Ok(binding) => binding,
        Err(error) => {
            // Without the exporter pairing is not allowed.
            let _ = refuse(&mut session, PairingOutcome::Unsupported).await;
            let _ = session.close().await;
            return Outcome::Failed(failure_of(&error, method));
        }
    };
    let timeouts = PairingTimeouts::default();
    let mut prompt = Prompt {
        core: core.clone(),
        generation: job.generation,
        attempt: job.id,
    };
    let result = match method {
        PairingMethod::QrCode => {
            let Some(offer) = &job.offer else {
                let _ = refuse(&mut session, PairingOutcome::NotExpected).await;
                let _ = session.close().await;
                return Outcome::Failed(PairingFailure::NotExpected);
            };
            pair_by_qr(
                &mut session,
                &binding,
                &phone,
                offer,
                &core.store,
                &mut prompt,
                &timeouts,
            )
            .await
        }
        PairingMethod::Sas => {
            pair_by_sas(
                &mut session,
                &binding,
                &phone,
                &core.store,
                &mut prompt,
                &timeouts,
            )
            .await
        }
    };
    match result {
        Err(error) => {
            tracing::debug!(%error, "pairing failed");
            let _ = match error.goodbye_reason() {
                Some(reason) => session.goodbye(reason, "pairing failed").await,
                None => session.close().await,
            };
            let reason = failure_of(&error, method);
            // pair_by_qr blocks a peer whose proof never verified.
            let blocked = job
                .offer
                .as_ref()
                .is_some_and(|offer| lock(offer).is_blocked(&peer, address.ip()));
            if blocked || matches!(error, PairingError::OfferUnavailable(_)) {
                Outcome::Unproven(reason)
            } else {
                Outcome::Failed(reason)
            }
        }
        Ok(success) => match session.finish_pairing().await {
            Ok(()) => Outcome::Paired(Box::new(Done {
                session,
                address,
                success,
            })),
            Err(error) => {
                tracing::debug!(%error, "the connection ended right after pairing");
                Outcome::Failed(PairingFailure::ConnectionLost)
            }
        },
    }
}

/// A pairing is stored: start the phone's task on the connection it was
/// made on, and report.
fn complete(core: &Arc<Core>, generation: u64, done: Done) {
    let Done {
        session,
        address,
        success,
    } = done;
    let fingerprint: CertificateFingerprint = success.device.fingerprint;
    if let Err(error) = records::note_connected(&mut *lock(&core.store), &fingerprint, address) {
        tracing::warn!(%error, "could not note the phone's address");
    }
    let record: PairedDevice = lock(&core.store)
        .get(&fingerprint)
        .unwrap_or(success.device.clone());
    for old in &success.replaced {
        if old.id != record.id {
            core.forget_device(&old.id, true);
        }
    }
    core.spawn_device(
        &record,
        Some(Established {
            session,
            fingerprint,
        }),
    );
    clear_offer_secret(core);
    let device = core
        .devices()
        .into_iter()
        .find(|device| device.id == record.id);
    if let Some(device) = device {
        finish_with(core, generation, |ctl| {
            // Other connections on the same offer are closed.
            ctl.reset();
            ctl.phase = Phase::Paired(device);
        });
    }
    core.emit_devices();
}

/// Asks the user the questions pairing has.
struct Prompt {
    core: Arc<Core>,
    generation: u64,
    /// The connection it asks for.
    attempt: u64,
}

impl PairingPrompt for Prompt {
    async fn confirm_code(
        &mut self,
        code: &PairingCode,
        phone: &DeviceInfo,
        _replaces: &[PairedDevice],
    ) -> SasDecision {
        let (answer, decision) = oneshot::channel();
        let shown = finish_with(&self.core, self.generation, |ctl| {
            // The phone revealed its nonce: this attempt counts.
            ctl.attempts_left = ctl.attempts_left.saturating_sub(1);
            ctl.answer = Some(answer);
            ctl.phase = Phase::Sas {
                code: code.as_str().to_owned(),
                phone: phone.name.clone(),
                expires: Instant::now() + SAS_LIFETIME,
                confirmed_here: false,
            };
        });
        if !shown {
            return SasDecision::Declined;
        }
        match decision.await {
            Ok(true) => SasDecision::Confirmed,
            _ => SasDecision::Declined,
        }
    }

    async fn confirm_replace(&mut self, phone: &DeviceInfo, _replaces: &[PairedDevice]) -> bool {
        // The screen has no way to ask yet, so a pairing by either method
        // never replaces a record of the same device under another key:
        // remove the old one first.
        tracing::info!(phone = %phone.name, "declined to replace a pairing record without asking");
        false
    }

    fn proof_verified(&mut self) {
        let mut ctl = lock(&self.core.pairing);
        if ctl.generation == self.generation {
            ctl.proven(self.attempt);
        }
    }
}
