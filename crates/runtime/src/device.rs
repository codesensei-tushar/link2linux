//! The task that keeps a session with one paired phone: dials when the
//! reconnect policy says so, runs the session, and reports the state.

use std::sync::Arc;
use std::time::Instant;

use link2linux_core::discovery::reconnect::{
    Poll, RECONNECTING_FOR, Reconnector, SessionEnd, Status,
};
use link2linux_core::identity::CertificateFingerprint;
use link2linux_core::pairing::TrustStore;
use link2linux_core::pairing::records::{self, Rejection};
use link2linux_core::session::{SessionConfig, SessionError};
use link2linux_core::transport::Connection;
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time;
use tokio_util::sync::CancellationToken;

use crate::dial::race;
use crate::link::{self, Command, Ending, LinkContext, lock};
use crate::notify::{NotificationsDriver, PhoneSession};
use crate::shared::Core;
use crate::view::DeviceStatus;

/// What the owner of the task can send it and wake it with.
pub(crate) struct Inbox {
    pub commands: UnboundedReceiver<Command>,
    /// Discovery saw the phone.
    pub wake: Arc<Notify>,
    pub cancel: tokio_util::sync::CancellationToken,
}

/// A session that is already up, with the key the phone presented.
pub struct Established {
    /// The session.
    pub session: PhoneSession,
    /// The key the phone presented.
    pub fingerprint: CertificateFingerprint,
}

/// What came of one dial attempt.
enum Attempt {
    Connected(Box<Established>),
    /// Try again later: the policy has been told.
    Failed,
    /// The record is gone: stop.
    Gone,
}

struct Supervisor {
    core: Arc<Core>,
    id: String,
    reconnector: Reconnector,
    driver: Box<dyn NotificationsDriver>,
    commands: UnboundedReceiver<Command>,
    wake: Arc<Notify>,
    cancel: CancellationToken,
    /// The phone refused this computer's key but the record stays until the
    /// user removes it.
    revoked: bool,
    down_since: Instant,
}

/// Keeps a session with the phone `id` until its record is removed or the
/// runtime stops. `initial` is a session that is already up.
pub(crate) async fn supervise(
    core: Arc<Core>,
    id: String,
    driver: Box<dyn NotificationsDriver>,
    inbox: Inbox,
    initial: Option<Established>,
) {
    let now = Instant::now();
    let mut supervisor = Supervisor {
        core,
        id,
        reconnector: Reconnector::new(now),
        driver,
        commands: inbox.commands,
        wake: inbox.wake,
        cancel: inbox.cancel,
        revoked: false,
        down_since: now,
    };
    supervisor.run(initial).await;
}

impl Supervisor {
    async fn run(&mut self, mut pending: Option<Established>) {
        loop {
            if self.cancel.is_cancelled() {
                return;
            }
            let established = if let Some(established) = pending.take() {
                established
            } else if self.revoked {
                if !self.wait(None).await {
                    return;
                }
                continue;
            } else {
                match self.reconnector.poll(Instant::now()) {
                    Poll::Dial => match self.attempt().await {
                        Attempt::Connected(established) => *established,
                        Attempt::Failed => {
                            self.publish();
                            continue;
                        }
                        Attempt::Gone => return,
                    },
                    Poll::WaitUntil(at) => {
                        if !self.wait(Some(at)).await {
                            return;
                        }
                        continue;
                    }
                    Poll::Idle => {
                        if !self.wait(None).await {
                            return;
                        }
                        continue;
                    }
                }
            };
            if !self.session(established).await {
                return;
            }
        }
    }

    /// Runs one session. Returns whether the task goes on.
    async fn session(&mut self, established: Established) -> bool {
        let core = self.core.clone();
        self.reconnector.connected(Instant::now());
        core.set_status(&self.id, DeviceStatus::Connected, true);
        let context = LinkContext {
            store: &core.store,
            fingerprint: established.fingerprint,
            active: core.is_active(),
            trust_changed: &|| core.publish_keys(),
        };
        let ending = link::run(
            established.session,
            &mut *self.driver,
            &mut self.commands,
            &self.cancel,
            &context,
        )
        .await;
        let now = Instant::now();
        self.down_since = now;
        match ending {
            Ending::Shutdown | Ending::Removed => false,
            Ending::PhoneBrokeRules => {
                self.reconnector.session_ended(now, SessionEnd::Backoff);
                self.publish();
                true
            }
            Ending::Peer(end) => {
                self.reconnector.session_ended(now, end);
                if end == SessionEnd::Unpaired {
                    // The phone removed the pairing and the session layer
                    // deleted the record.
                    core.forget_device(&self.id, true);
                    return false;
                }
                self.publish();
                true
            }
        }
    }

    /// Waits until `until`, new evidence, a command or the end of the
    /// "reconnecting" period, and applies what happened. Returns whether
    /// the task goes on.
    async fn wait(&mut self, until: Option<Instant>) -> bool {
        let flip = matches!(
            self.reconnector.status(Instant::now()),
            Status::Reconnecting
        )
        .then_some(self.down_since + RECONNECTING_FOR);
        let deadline = [until, flip].into_iter().flatten().min();
        let sleep = async {
            match deadline {
                Some(at) => time::sleep_until(at.into()).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = self.cancel.cancelled() => return false,
            () = self.wake.notified() => self.reconnector.evidence(),
            command = self.commands.recv() => match command {
                None | Some(Command::Unpair) => return false,
                Some(Command::Reconnect) => {
                    self.revoked = false;
                    self.reconnector.user_reconnect(Instant::now());
                }
                Some(Command::NetworkChanged) => self.reconnector.network_changed(Instant::now()),
                Some(Command::SetActive(_)) => {}
                Some(Command::Dismiss { key }) => {
                    if let Err(error) = self.driver.dismiss(None, &key).await {
                        tracing::warn!(%error, "could not queue a dismissal");
                    }
                }
                Some(Command::Action { key, action_id, reply }) => {
                    let queued = self
                        .driver
                        .run_action(None, &key, &action_id, reply.as_deref())
                        .await;
                    if let Err(error) = queued {
                        tracing::warn!(%error, "could not queue a notification action");
                    }
                }
                Some(Command::Response(response)) => {
                    if let Err(error) = self.driver.handle_response(None, response).await {
                        tracing::warn!(%error, "notification response failed");
                    }
                }
            },
            () = sleep => {}
        }
        self.publish();
        true
    }

    /// Reports the state the policy shows.
    fn publish(&self) {
        let status = if self.revoked {
            DeviceStatus::Revoked
        } else {
            match self.reconnector.status(Instant::now()) {
                Status::Connected => DeviceStatus::Connected,
                Status::Reconnecting => DeviceStatus::Reconnecting,
                Status::Disconnected(_) => DeviceStatus::Disconnected,
            }
        };
        self.core.set_status(&self.id, status, false);
    }

    /// One dial attempt: race the candidates, handshake on the winner.
    async fn attempt(&mut self) -> Attempt {
        let core = self.core.clone();
        let record = lock(&core.store)
            .list()
            .into_iter()
            .find(|record| record.id == self.id);
        let Some(record) = record else {
            return Attempt::Gone;
        };
        let plan = lock(&core.evidence).plan(
            &record.id,
            &record.last_addresses,
            &core.locals(),
            Instant::now(),
        );
        let dialed = tokio::select! {
            () = self.cancel.cancelled() => return Attempt::Gone,
            dialed = race(&core.dialer, plan, record.pin()) => dialed,
        };
        let now = Instant::now();
        let (connection, address) = match dialed {
            Ok(winner) => winner,
            Err(error) if error.is_rejected_by_peer() => {
                return self.rejected(&record.fingerprint);
            }
            Err(error) => {
                tracing::debug!(%error, phone = %record.name, "dial attempt failed");
                self.reconnector.attempt_failed(now);
                return Attempt::Failed;
            }
        };
        let fingerprint = connection.peer_fingerprint();
        let mut start = core.session_start();
        start.expected_peer_id = Some(record.id.clone());
        match link_up(connection, start).await {
            Ok(session) => {
                if let Err(error) =
                    records::note_connected(&mut *lock(&core.store), &fingerprint, address)
                {
                    tracing::warn!(%error, "could not note the address");
                }
                Attempt::Connected(Box::new(Established {
                    session,
                    fingerprint,
                }))
            }
            Err(SessionError::VersionMismatch { .. }) => {
                self.reconnector
                    .session_ended(now, SessionEnd::VersionMismatch);
                Attempt::Failed
            }
            Err(error) => {
                tracing::debug!(%error, phone = %record.name, "handshake failed");
                self.reconnector.attempt_failed(now);
                Attempt::Failed
            }
        }
    }

    /// The phone refused this computer's certificate.
    fn rejected(&mut self, fingerprint: &CertificateFingerprint) -> Attempt {
        let outcome = records::certificate_rejected(&mut *lock(&self.core.store), fingerprint);
        match outcome {
            Ok(Rejection::DeletedUnconfirmed(_)) => {
                // The pairing never finished on the phone.
                self.core.forget_device(&self.id, true);
                Attempt::Gone
            }
            Ok(Rejection::RemovedByPhone(_)) => {
                self.revoked = true;
                self.reconnector
                    .session_ended(Instant::now(), SessionEnd::Unpaired);
                Attempt::Failed
            }
            Err(error) => {
                tracing::warn!(%error, "could not apply the phone's refusal");
                self.reconnector.attempt_failed(Instant::now());
                Attempt::Failed
            }
        }
    }
}

async fn link_up(
    connection: link2linux_core::transport::TlsConnection,
    start: link2linux_core::session::SessionStart,
) -> Result<PhoneSession, SessionError> {
    PhoneSession::handshake(connection, start, SessionConfig::default()).await
}
