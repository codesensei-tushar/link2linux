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
//! On a failure these functions have already sent the `PairingResult` the
//! protocol asks for, if any. The caller ends the connection, with a
//! `Goodbye` when [`PairingError::goodbye_reason`] names one.

use std::fmt;
use std::net::SocketAddr;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use link2linux_protocol::Payload;
use link2linux_protocol::v1::{
    PairingMethod, PairingOutcome, PairingProof, PairingResult, SasNonce,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::{self, Instant};
use tracing::debug;
use zeroize::Zeroize;

use super::crypto::{self, EKM_LEN, NONCE_LEN, Role};
use super::{
    CertificateFingerprint, OfferStatus, PairedDevice, PairingCode, PairingError, QrOffer,
    TrustStore,
};
use crate::device::{DeviceInfo, DeviceKind};
use crate::session::{Session, SessionEvent};
use crate::transport::Connection;

/// The envelopes of a pairing connection after the `Hello` exchange.
///
/// [`Session`] implements it for a session whose `Hello` set the pairing
/// method: it numbers and checks sequence numbers, answers liveness
/// messages itself, and hands the pairing messages over. Once a pairing
/// function succeeds, [`Session::finish_pairing`] starts the session
/// proper.
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

impl<S> PairingChannel for Session<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn send(&mut self, payload: Payload) -> Result<(), PairingError> {
        Session::send(self, payload)
            .await
            .map(drop)
            .map_err(|error| PairingError::Connection(error.to_string()))
    }

    async fn recv(&mut self) -> Result<Payload, PairingError> {
        loop {
            match self.next_event().await {
                Ok(Some(SessionEvent::Message { payload, .. })) => return Ok(payload),
                Ok(Some(SessionEvent::Goodbye(goodbye))) => {
                    debug!(reason = ?goodbye.reason, "the phone ended the pairing connection");
                    return Err(PairingError::Closed);
                }
                // Liveness and the like are the session's business.
                Ok(Some(_)) => {}
                Ok(None) => return Err(PairingError::Closed),
                Err(error) => return Err(PairingError::Connection(error.to_string())),
            }
        }
    }
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
    /// would replace: the same device id under another key; show them too.
    /// The pairing still asks [`confirm_replace`](Self::confirm_replace)
    /// before it replaces them. Dropping the future means the question is
    /// withdrawn.
    fn confirm_code(
        &mut self,
        code: &PairingCode,
        phone: &DeviceInfo,
        replaces: &[PairedDevice],
    ) -> impl Future<Output = SasDecision> + Send;

    /// Asks whether a pairing with `phone` replaces `replaces`, records
    /// with the same device id under another key (name, kind, date paired).
    /// Both methods ask it once the phone has proven its key, since the
    /// device id alone must never replace a record. `false` declines the
    /// pairing.
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
    /// `PAIRING_OUTCOME_UNSUPPORTED` ([`refuse`]).
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

/// A stored pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingSuccess {
    /// The new record, unconfirmed until the phone's first `FeatureStatus`.
    pub device: PairedDevice,
    /// Records it replaced, with the same device id under another key, after
    /// the user agreed.
    pub replaced: Vec<PairedDevice>,
}

/// Pairs with the phone that scanned `offer`, on a connection dialed with
/// `PAIRING_METHOD_QR_CODE` in its `Hello`.
///
/// The desktop sends its proof at once and checks the phone's. A failed
/// proof blocks that phone's key and address for the rest of the offer; the
/// offer itself stays up until it expires, is used, or has seen
/// [`MAX_FAILED_PROOFS`](super::MAX_FAILED_PROOFS) failures. When several
/// connections run on the same offer, the first whose proof verifies wins
/// and the others get `PAIRING_OUTCOME_NOT_EXPECTED`.
///
/// # Errors
///
/// Fails when the offer is unusable or blocks this phone, when a proof does
/// not verify, when the phone ends the pairing, when the user declines to
/// replace an existing record, on timeouts, protocol violations, and
/// connection or store errors.
pub async fn pair_by_qr<C, P, S>(
    channel: &mut C,
    binding: &PairingBinding,
    phone: &DeviceInfo,
    offer: &Mutex<QrOffer>,
    store: &Mutex<S>,
    prompt: &mut P,
    timeouts: &PairingTimeouts,
) -> Result<PairingSuccess, PairingError>
where
    C: PairingChannel,
    P: PairingPrompt,
    S: TrustStore + ?Sized,
{
    check_start(binding, phone, PairingMethod::QrCode)?;
    let checked = {
        let offer = lock(offer)?;
        match offer.status(Instant::now()) {
            OfferStatus::Active if offer.is_blocked(&binding.phone, binding.peer_addr.ip()) => {
                Err(PairingError::Blocked)
            }
            OfferStatus::Active => Ok(offer.secret().clone()),
            status => Err(PairingError::OfferUnavailable(status)),
        }
    };
    let secret = match checked {
        Ok(secret) => secret,
        Err(error) => return Err(abort(channel, PairingOutcome::NotExpected, error).await),
    };

    let mac = crypto::pairing_proof(&secret, Role::Desktop, binding.ekm());
    send(
        channel,
        Payload::PairingProof(PairingProof { mac: mac.to_vec() }),
    )
    .await?;

    // The phone's proof comes right after its Hello; its verdict may take
    // longer if it asks its user something.
    let proof_deadline = Instant::now() + timeouts.step;
    let verdict_deadline = proof_deadline + timeouts.confirm;
    let mut proof_verified = false;
    let mut phone_accepted = false;
    while !(proof_verified && phone_accepted) {
        let deadline = if proof_verified {
            verdict_deadline
        } else {
            proof_deadline
        };
        let payload = match time::timeout_at(deadline, channel.recv()).await {
            Err(_) => {
                return Err(abort(channel, PairingOutcome::TimedOut, PairingError::TimedOut).await);
            }
            Ok(payload) => payload?,
        };
        match payload {
            Payload::PairingProof(proof) if !proof_verified => {
                if !crypto::verify_pairing_proof(&secret, Role::Phone, binding.ekm(), &proof.mac) {
                    lock(offer)?.record_failed_proof(binding.phone, binding.peer_addr.ip());
                    let error = PairingError::VerificationFailed;
                    return Err(abort(channel, PairingOutcome::VerificationFailed, error).await);
                }
                proof_verified = true;
            }
            Payload::PairingResult(result) if result.outcome() != PairingOutcome::Accepted => {
                if result.outcome() == PairingOutcome::VerificationFailed {
                    lock(offer)?.record_failed_proof(binding.phone, binding.peer_addr.ip());
                }
                return Err(PairingError::Rejected(result.outcome()));
            }
            Payload::PairingResult(_) if !phone_accepted => phone_accepted = true,
            other => return Err(unexpected(channel, &other).await),
        }
    }

    let replaces = conflicting_records(store, binding, phone)?;
    ask_to_replace(channel, prompt, phone, &replaces, timeouts).await?;

    let stored = {
        let mut offer = lock(offer)?;
        match offer.status(Instant::now()) {
            OfferStatus::Active => {
                let stored = store_pairing(store, binding, phone, replaces);
                if stored.is_ok() {
                    offer.mark_used();
                }
                stored
            }
            status => Err(PairingError::OfferUnavailable(status)),
        }
    };
    finish(channel, stored).await
}

/// Pairs by SAS with the phone the user picked, on a connection dialed
/// with `PAIRING_METHOD_SAS` in its `Hello`.
///
/// Run it on one connection per time the user picks a phone, and never
/// redial on its own: each run in which the phone reveals its nonce uses up
/// one of its few attempts. The phone commits first; the desktop sends its
/// nonce only after the commitment arrived, and checks the revealed nonce
/// against it. Both screens then show the code, and the pairing is stored
/// once the user here confirmed it and the phone accepted, and, if it
/// replaces records of the same device id under another key, the user
/// agreed to that too.
///
/// # Errors
///
/// Fails when the messages come in another order or the commitment does
/// not verify, when either user declines or does not decide in time, when
/// the user keeps the records the pairing would replace, when the phone
/// ends the pairing, on protocol violations, and on connection or store
/// errors.
pub async fn pair_by_sas<C, P, S>(
    channel: &mut C,
    binding: &PairingBinding,
    phone: &DeviceInfo,
    store: &Mutex<S>,
    prompt: &mut P,
    timeouts: &PairingTimeouts,
) -> Result<PairingSuccess, PairingError>
where
    C: PairingChannel,
    P: PairingPrompt,
    S: TrustStore + ?Sized,
{
    check_start(binding, phone, PairingMethod::Sas)?;

    // 1. The phone commits to its nonce.
    let commitment = match sas_step(channel, timeouts).await? {
        Payload::SasCommit(commit) if commit.commitment.len() == 32 => commit.commitment,
        other => return Err(sas_out_of_order(channel, &other).await),
    };

    // 2. Only then the desktop sends its own.
    let desktop_nonce: [u8; NONCE_LEN] = crypto::random_bytes();
    send(
        channel,
        Payload::SasNonce(SasNonce {
            nonce: desktop_nonce.to_vec(),
        }),
    )
    .await?;

    // 3. The phone reveals its nonce, which must match the commitment.
    let phone_nonce: [u8; NONCE_LEN] = match sas_step(channel, timeouts).await? {
        Payload::SasNonce(nonce) if nonce.nonce.len() == NONCE_LEN => {
            nonce.nonce.try_into().expect("length checked")
        }
        other => return Err(sas_out_of_order(channel, &other).await),
    };
    if !crypto::verify_sas_commitment(binding.ekm(), &phone_nonce, &commitment) {
        let error = PairingError::VerificationFailed;
        return Err(abort(channel, PairingOutcome::VerificationFailed, error).await);
    }

    // 4. The user compares the codes while the phone's verdict comes in.
    let code = crypto::sas_code(binding.ekm(), &desktop_nonce, &phone_nonce);
    let replaces = conflicting_records(store, binding, phone)?;
    {
        let decision = prompt.confirm_code(&code, phone, &replaces);
        tokio::pin!(decision);
        let user_deadline = Instant::now() + timeouts.confirm;
        let verdict_deadline = user_deadline + timeouts.step;
        let mut confirmed = false;
        let mut phone_accepted = false;
        while !(confirmed && phone_accepted) {
            tokio::select! {
                decision = &mut decision, if !confirmed => match decision {
                    SasDecision::Confirmed => confirmed = true,
                    SasDecision::Declined => {
                        return Err(abort(channel, PairingOutcome::Declined, PairingError::Declined).await);
                    }
                },
                () = time::sleep_until(user_deadline), if !confirmed => {
                    return Err(abort(channel, PairingOutcome::TimedOut, PairingError::TimedOut).await);
                }
                () = time::sleep_until(verdict_deadline), if confirmed => {
                    return Err(abort(channel, PairingOutcome::TimedOut, PairingError::TimedOut).await);
                }
                payload = channel.recv(), if !phone_accepted => match payload? {
                    Payload::PairingResult(result) if result.outcome() == PairingOutcome::Accepted => {
                        phone_accepted = true;
                    }
                    Payload::PairingResult(result) => return Err(PairingError::Rejected(result.outcome())),
                    other => return Err(sas_out_of_order(channel, &other).await),
                },
            }
        }
    }

    ask_to_replace(channel, prompt, phone, &replaces, timeouts).await?;
    let stored = store_pairing(store, binding, phone, replaces);
    finish(channel, stored).await
}

/// Sends `outcome` as this side's `PairingResult`, for example
/// `PAIRING_OUTCOME_UNSUPPORTED` when [`PairingBinding::from_connection`]
/// failed. The caller then closes the connection.
///
/// # Errors
///
/// Fails if the message cannot be sent.
pub async fn refuse<C: PairingChannel>(
    channel: &mut C,
    outcome: PairingOutcome,
) -> Result<(), PairingError> {
    send(channel, result(outcome)).await
}

/// Checks what every pairing needs before it starts.
fn check_start(
    binding: &PairingBinding,
    phone: &DeviceInfo,
    method: PairingMethod,
) -> Result<(), PairingError> {
    if binding.method != method {
        return Err(PairingError::Exporter(format!(
            "the binding is for {}, not {}",
            binding.method.as_str_name(),
            method.as_str_name()
        )));
    }
    if phone.kind != DeviceKind::Phone {
        let detail = "the peer's Hello does not announce a phone".to_owned();
        return Err(PairingError::Protocol(detail));
    }
    Ok(())
}

/// Waits up to one step for the next SAS message.
async fn sas_step<C: PairingChannel>(
    channel: &mut C,
    timeouts: &PairingTimeouts,
) -> Result<Payload, PairingError> {
    match time::timeout(timeouts.step, channel.recv()).await {
        Ok(payload) => payload,
        Err(_) => Err(abort(channel, PairingOutcome::TimedOut, PairingError::TimedOut).await),
    }
}

/// Handles a payload that breaks the SAS order: a verdict from the phone
/// ends the pairing; any other pairing message fails it.
async fn sas_out_of_order<C: PairingChannel>(channel: &mut C, payload: &Payload) -> PairingError {
    match payload {
        Payload::PairingResult(result) => PairingError::Rejected(result.outcome()),
        Payload::SasCommit(_) | Payload::SasNonce(_) | Payload::PairingProof(_) => {
            let error = PairingError::OutOfOrder(payload_name(payload));
            abort(channel, PairingOutcome::VerificationFailed, error).await
        }
        other => unexpected(channel, other).await,
    }
}

/// Handles a payload that has no place in this pairing.
async fn unexpected<C: PairingChannel>(channel: &mut C, payload: &Payload) -> PairingError {
    match payload {
        Payload::Goodbye(goodbye) => {
            debug!(reason = ?goodbye.reason(), "the phone closed the pairing connection");
            PairingError::Closed
        }
        Payload::PairingProof(_)
        | Payload::PairingResult(_)
        | Payload::SasCommit(_)
        | Payload::SasNonce(_) => {
            let error = PairingError::OutOfOrder(payload_name(payload));
            abort(channel, PairingOutcome::VerificationFailed, error).await
        }
        other => PairingError::Protocol(format!("{} during pairing", payload_name(other))),
    }
}

/// Asks the user whether the pairing replaces `replaces`, if there are
/// any, and declines it unless they agree in time.
async fn ask_to_replace<C, P>(
    channel: &mut C,
    prompt: &mut P,
    phone: &DeviceInfo,
    replaces: &[PairedDevice],
    timeouts: &PairingTimeouts,
) -> Result<(), PairingError>
where
    C: PairingChannel,
    P: PairingPrompt,
{
    if replaces.is_empty() {
        return Ok(());
    }
    match time::timeout(timeouts.confirm, prompt.confirm_replace(phone, replaces)).await {
        Ok(true) => Ok(()),
        Ok(false) => {
            let error = PairingError::ReplaceDeclined;
            Err(abort(channel, PairingOutcome::Declined, error).await)
        }
        Err(_) => Err(abort(channel, PairingOutcome::TimedOut, PairingError::TimedOut).await),
    }
}

/// Records with the phone's device id but another key, which the new
/// pairing would replace.
fn conflicting_records<S: TrustStore + ?Sized>(
    store: &Mutex<S>,
    binding: &PairingBinding,
    phone: &DeviceInfo,
) -> Result<Vec<PairedDevice>, PairingError> {
    Ok(lock(store)?
        .list()
        .into_iter()
        .filter(|device| device.id == phone.id && !device.accepts(&binding.phone))
        .collect())
}

/// Stores the new record, replacing a record under the same key and, in
/// the same step, the first of `replaces`; the rest of `replaces` go too.
fn store_pairing<S: TrustStore + ?Sized>(
    store: &Mutex<S>,
    binding: &PairingBinding,
    phone: &DeviceInfo,
    replaces: Vec<PairedDevice>,
) -> Result<PairingSuccess, PairingError> {
    let mut device = PairedDevice::new(
        phone.id.clone(),
        phone.name.clone(),
        phone.kind,
        binding.phone,
        SystemTime::now(),
    );
    device.last_addresses.push(binding.peer_addr);

    let mut store = lock(store)?;
    match replaces.split_first() {
        Some((first, rest)) => {
            store.replace(&first.fingerprint, device.clone())?;
            for other in rest {
                store.remove(&other.fingerprint)?;
            }
        }
        None => store.insert(device.clone())?,
    }
    Ok(PairingSuccess {
        device,
        replaced: replaces,
    })
}

/// Sends the desktop's acceptance once the pairing is stored.
async fn finish<C: PairingChannel>(
    channel: &mut C,
    stored: Result<PairingSuccess, PairingError>,
) -> Result<PairingSuccess, PairingError> {
    match stored {
        Ok(success) => {
            // If this send fails, the record stays unconfirmed and the next
            // dial sorts it out.
            send(channel, result(PairingOutcome::Accepted)).await?;
            Ok(success)
        }
        Err(error @ PairingError::OfferUnavailable(_)) => {
            Err(abort(channel, PairingOutcome::NotExpected, error).await)
        }
        Err(error) => Err(error),
    }
}

/// Sends `outcome` and returns `error`, for the caller to return.
async fn abort<C: PairingChannel>(
    channel: &mut C,
    outcome: PairingOutcome,
    error: PairingError,
) -> PairingError {
    if let Err(send_error) = send(channel, result(outcome)).await {
        debug!(%send_error, "could not send the pairing result");
    }
    error
}

async fn send<C: PairingChannel>(channel: &mut C, payload: Payload) -> Result<(), PairingError> {
    channel.send(payload).await
}

fn result(outcome: PairingOutcome) -> Payload {
    Payload::PairingResult(PairingResult {
        outcome: outcome.into(),
    })
}

fn lock<T: ?Sized>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, PairingError> {
    mutex
        .lock()
        .map_err(|_| PairingError::Store("a pairing lock was poisoned".into()))
}

/// A payload's message name, for errors and logs.
fn payload_name(payload: &Payload) -> &'static str {
    match payload {
        Payload::PairingProof(_) => "PairingProof",
        Payload::SasCommit(_) => "SasCommit",
        Payload::SasNonce(_) => "SasNonce",
        Payload::PairingResult(_) => "PairingResult",
        Payload::Hello(_) => "Hello",
        Payload::Goodbye(_) => "Goodbye",
        _ => "a feature or session message",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::net::{Ipv4Addr, SocketAddr};

    use link2linux_protocol::Ping;
    use link2linux_protocol::v1::{GoodbyeReason, SasCommit};

    use super::*;
    use crate::pairing::MemoryTrustStore;

    /// Plays back the phone's messages, then waits forever.
    #[derive(Default)]
    struct Script {
        incoming: VecDeque<Payload>,
        sent: Vec<Payload>,
    }

    impl Script {
        fn playing(incoming: impl IntoIterator<Item = Payload>) -> Self {
            Self {
                incoming: incoming.into_iter().collect(),
                sent: Vec::new(),
            }
        }

        fn outcomes(&self) -> Vec<Option<PairingOutcome>> {
            self.sent
                .iter()
                .map(|payload| match payload {
                    Payload::PairingResult(result) => Some(result.outcome()),
                    _ => None,
                })
                .collect()
        }
    }

    impl PairingChannel for Script {
        async fn send(&mut self, payload: Payload) -> Result<(), PairingError> {
            self.sent.push(payload);
            Ok(())
        }

        async fn recv(&mut self) -> Result<Payload, PairingError> {
            match self.incoming.pop_front() {
                Some(payload) => Ok(payload),
                None => std::future::pending().await,
            }
        }
    }

    /// A user who never answers.
    struct Silent;

    impl PairingPrompt for Silent {
        async fn confirm_code(
            &mut self,
            _code: &PairingCode,
            _phone: &DeviceInfo,
            _replaces: &[PairedDevice],
        ) -> SasDecision {
            std::future::pending().await
        }

        async fn confirm_replace(
            &mut self,
            _phone: &DeviceInfo,
            _replaces: &[PairedDevice],
        ) -> bool {
            std::future::pending().await
        }
    }

    const EKM: [u8; 32] = [3; 32];
    const PHONE_NONCE: [u8; 32] = [4; 32];

    fn binding(method: PairingMethod) -> PairingBinding {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 27380));
        let desktop = CertificateFingerprint([1; 32]);
        PairingBinding::from_parts(method, desktop, CertificateFingerprint([2; 32]), addr, EKM)
    }

    fn phone() -> DeviceInfo {
        DeviceInfo::new("phone-1", "Pixel", DeviceKind::Phone, [])
    }

    fn commit() -> Payload {
        Payload::SasCommit(SasCommit {
            commitment: crypto::sas_commitment(&EKM, &PHONE_NONCE).to_vec(),
        })
    }

    fn reveal() -> Payload {
        Payload::SasNonce(SasNonce {
            nonce: PHONE_NONCE.to_vec(),
        })
    }

    async fn qr(channel: &mut Script, phone: &DeviceInfo) -> Result<PairingSuccess, PairingError> {
        let offer = Mutex::new(QrOffer::new(
            CertificateFingerprint([1; 32]),
            "Desk",
            vec![],
            None,
        ));
        let store = Mutex::new(MemoryTrustStore::default());
        let binding = binding(PairingMethod::QrCode);
        let timeouts = PairingTimeouts::default();
        pair_by_qr(
            channel,
            &binding,
            phone,
            &offer,
            &store,
            &mut Silent,
            &timeouts,
        )
        .await
    }

    async fn sas(channel: &mut Script) -> Result<PairingSuccess, PairingError> {
        let store = Mutex::new(MemoryTrustStore::default());
        let binding = binding(PairingMethod::Sas);
        let timeouts = PairingTimeouts::default();
        pair_by_sas(channel, &binding, &phone(), &store, &mut Silent, &timeouts).await
    }

    #[tokio::test(start_paused = true)]
    async fn qr_pairing_gives_up_on_a_silent_phone() {
        let mut channel = Script::default();
        let result = qr(&mut channel, &phone()).await;
        assert!(matches!(result, Err(PairingError::TimedOut)), "{result:?}");
        assert!(matches!(channel.sent[0], Payload::PairingProof(_)));
        assert_eq!(channel.outcomes()[1], Some(PairingOutcome::TimedOut));
    }

    #[tokio::test]
    async fn feature_messages_during_pairing_are_protocol_errors() {
        let mut channel = Script::playing([Payload::Ping(Ping { nonce: 1 })]);
        let result = qr(&mut channel, &phone()).await;
        let error = result.as_ref().unwrap_err();
        assert_eq!(error.goodbye_reason(), Some(GoodbyeReason::ProtocolError));
        assert!(
            matches!(result, Err(PairingError::Protocol(_))),
            "{result:?}"
        );
        // The session sends the Goodbye; the driver sent only its proof.
        assert!(matches!(channel.sent[..], [Payload::PairingProof(_)]));
    }

    #[tokio::test]
    async fn the_peer_must_announce_a_phone() {
        let mut channel = Script::default();
        let desktop = DeviceInfo::new("desktop-2", "Other", DeviceKind::Desktop, []);
        let result = qr(&mut channel, &desktop).await;
        assert!(
            matches!(result, Err(PairingError::Protocol(_))),
            "{result:?}"
        );
        assert!(channel.sent.is_empty());
    }

    #[tokio::test]
    async fn the_desktop_never_sends_its_nonce_before_the_commitment() {
        let mut channel = Script::playing([reveal()]);
        let result = sas(&mut channel).await;
        assert!(
            matches!(result, Err(PairingError::OutOfOrder("SasNonce"))),
            "{result:?}"
        );
        assert_eq!(
            channel.outcomes(),
            [Some(PairingOutcome::VerificationFailed)]
        );
    }

    #[tokio::test]
    async fn a_second_commitment_fails_the_pairing() {
        let mut channel = Script::playing([commit(), reveal(), commit()]);
        let result = sas(&mut channel).await;
        assert!(
            matches!(result, Err(PairingError::OutOfOrder("SasCommit"))),
            "{result:?}"
        );
        assert!(matches!(channel.sent[0], Payload::SasNonce(_)));
        assert_eq!(
            channel.outcomes()[1],
            Some(PairingOutcome::VerificationFailed)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn each_sas_step_has_ten_seconds() {
        let start = Instant::now();
        let mut channel = Script::playing([commit()]);
        let result = sas(&mut channel).await;
        assert!(matches!(result, Err(PairingError::TimedOut)), "{result:?}");
        assert_eq!(start.elapsed(), Duration::from_secs(10));
        assert_eq!(channel.outcomes()[1], Some(PairingOutcome::TimedOut));
    }

    #[tokio::test(start_paused = true)]
    async fn the_user_has_sixty_seconds_to_compare_codes() {
        let start = Instant::now();
        let mut channel = Script::playing([commit(), reveal()]);
        let result = sas(&mut channel).await;
        assert!(matches!(result, Err(PairingError::TimedOut)), "{result:?}");
        assert_eq!(start.elapsed(), Duration::from_secs(60));
        assert_eq!(channel.outcomes()[1], Some(PairingOutcome::TimedOut));
    }
}
