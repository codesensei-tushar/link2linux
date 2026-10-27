//! A live session with a paired phone, from the handshake to its end.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use link2linux_core::discovery::reconnect::SessionEnd;
use link2linux_core::files::TransferId;
use link2linux_core::identity::CertificateFingerprint;
use link2linux_core::pairing::{FileTrustStore, records};
use link2linux_core::session::{Session, SessionEvent};
use link2linux_platform::NotificationResponse;
use link2linux_protocol::v1::{Goodbye, GoodbyeReason};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::clipboard::{Copies, Outgoing};
use crate::features::Features;
use crate::notify::PhoneSession;
use crate::phone::PhoneCommand;
use crate::screen::ScreenCommand;

/// What the owner of a session can ask of it.
#[derive(Debug)]
pub enum Command {
    /// The window gained or lost focus: ping often or rarely.
    SetActive(bool),
    /// The user did something with a banner.
    Response(NotificationResponse),
    /// The user dismissed a notification on the notifications page.
    Dismiss {
        /// The notification's key.
        key: String,
    },
    /// The user ran a notification's action on the notifications page.
    Action {
        /// The notification's key.
        key: String,
        /// The action's id.
        action_id: String,
        /// The text of a reply.
        reply: Option<String>,
    },
    /// The user turned clipboard sharing on or off on this desktop.
    ClipboardSharing,
    /// The user sent files to the phone. The shell checked them.
    SendFiles {
        /// The files, as one transfer.
        paths: Vec<PathBuf>,
    },
    /// The user cancelled a file transfer, in either direction.
    CancelTransfer {
        /// The transfer.
        id: TransferId,
    },
    /// The user removed a finished transfer from the files page.
    ForgetTransfer {
        /// The transfer.
        id: TransferId,
    },
    /// The user turned files on or off on this desktop.
    FilesSwitch,
    /// The user asked something of the phone's status or ringing.
    Phone(PhoneCommand),
    /// The user asked something of the phone's screen.
    Screen(ScreenCommand),
    /// The desktop's calls changed: its availability, whether it shows the
    /// phone's calls itself, or Bluetooth pairing with the phone.
    CallsChanged,
    /// The user removed the pairing: tell the phone and leave.
    Unpair,
    /// The network changed or the user asked to reconnect: no session
    /// is up to act on it, so the dial loop reads this.
    Reconnect,
    /// The network changed.
    NetworkChanged,
}

/// How a session ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ending {
    /// The connection ended, by the phone's `Goodbye` or by itself.
    Peer(SessionEnd),
    /// The phone broke a feature's rules: this side says goodbye with a
    /// protocol error.
    PhoneBrokeRules,
    /// This side removed the pairing and told the phone.
    Removed,
    /// The runtime is shutting down.
    Shutdown,
}

/// What a session needs from the runtime around it.
pub struct LinkContext<'a> {
    /// The paired phones.
    pub store: &'a Mutex<FileTrustStore>,
    /// The key the phone presented.
    pub fingerprint: CertificateFingerprint,
    /// Whether the window has focus.
    pub active: bool,
    /// Called when a record changed in a way discovery has to know about.
    pub trust_changed: &'a (dyn Fn() + Send + Sync),
}

/// Locks `mutex`, going on after a panic elsewhere: the data are plain
/// records that stay consistent between statements.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs `session` until it ends.
///
/// The features see every event first (see [`Features`]); the clipboard
/// also pushes each new copy from `copies`. A copy made
/// before the session started is not pushed from there: the clipboard
/// service pushes a recent one by itself. The loop itself keeps the pairing
/// record in step with the phone: it confirms the record on the phone's
/// first `FeatureStatus`, stores its presence key, and deletes the record
/// when the phone says `UNPAIRED`.
pub async fn run(
    mut session: PhoneSession,
    features: &mut Features,
    commands: &mut UnboundedReceiver<Command>,
    copies: &mut Copies,
    cancel: &CancellationToken,
    context: &LinkContext<'_>,
) -> Ending {
    copies.mark_unchanged();
    if context.active
        && let Err(error) = session.set_active(true).await
    {
        tracing::debug!(%error, "could not set the session active");
    }
    features.session_started(&mut session).await;
    let ending = drive(&mut session, features, commands, copies, cancel, context).await;
    features.session_ended();
    if ending.is_final() {
        // Before the goodbye, which may take a while: the app can exit
        // meanwhile, and the files must be gone by then.
        features.files.discard_all().await;
    }
    finish(session, &ending).await;
    ending
}

impl Ending {
    /// Whether nothing more will come from this phone's transfers: the
    /// pairing is removed or the app is closing.
    fn is_final(&self) -> bool {
        matches!(self, Self::Removed | Self::Shutdown)
    }

    /// How long the end of the session may take. A phone that reads gets
    /// the `Goodbye` and what was still queued, such as file chunks, well
    /// within it; one that vanished is not waited for. An unpair gets
    /// longer, as a phone that never hears it keeps a dead pairing, and
    /// nobody waits on this task.
    fn finish_within(&self) -> Duration {
        match self {
            Self::Removed => UNPAIR_FINISH_WITHIN,
            _ => FINISH_WITHIN,
        }
    }
}

/// How long the end of a session may take. A phone that reads gets the
/// `Goodbye` and what was still queued, such as file chunks, well within
/// it; one that vanished is not waited for.
const FINISH_WITHIN: Duration = Duration::from_secs(2);

/// How long the end of a session may take when the user removed the pairing,
/// so that the `Goodbye` outlasts a slow send's queued bytes.
const UNPAIR_FINISH_WITHIN: Duration = Duration::from_secs(10);

/// Ends `session` as `ending` says, within [`Ending::finish_within`].
async fn finish<S>(session: Session<S>, ending: &Ending)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let ended = async move {
        match ending {
            Ending::Shutdown => {
                session
                    .goodbye(GoodbyeReason::Shutdown, "the desktop is closing")
                    .await
            }
            Ending::Removed => {
                session
                    .goodbye(GoodbyeReason::Unpaired, "the user removed the pairing")
                    .await
            }
            Ending::PhoneBrokeRules => {
                session
                    .goodbye(
                        GoodbyeReason::ProtocolError,
                        "a feature's rules were broken",
                    )
                    .await
            }
            Ending::Peer(_) => session.close().await,
        }
    };
    if tokio::time::timeout(ending.finish_within(), ended)
        .await
        .is_err()
    {
        tracing::debug!("the phone did not take the end of the session in time");
    }
}

async fn drive(
    session: &mut PhoneSession,
    features: &mut Features,
    commands: &mut UnboundedReceiver<Command>,
    copies: &mut Copies,
    cancel: &CancellationToken,
    context: &LinkContext<'_>,
) -> Ending {
    // The sender goes with the phone's slot, when the phone is forgotten.
    let mut copying = true;
    let mut window_active = context.active;
    loop {
        tokio::select! {
            () = cancel.cancelled() => return Ending::Shutdown,
            () = sleep_until(features.next_deadline()) => features.tick(Some(session)).await,
            changed = copies.changed(), if copying => {
                let copy = copies.borrow_and_update().clone();
                match (changed, copy) {
                    (Err(_), _) => copying = false,
                    (Ok(()), Some(Outgoing { clip, origin })) => {
                        if let Err(error) = features.clipboard.share(session, clip, origin).await {
                            tracing::debug!(%error, "could not push the clipboard");
                        }
                    }
                    (Ok(()), None) => {}
                }
            }
            command = commands.recv() => match command {
                None => return Ending::Shutdown,
                Some(Command::Unpair) => return Ending::Removed,
                Some(Command::SetActive(active)) => window_active = active,
                Some(Command::Response(response)) => {
                    if features.handle_response(Some(session), response).await.is_err() {
                        return Ending::PhoneBrokeRules;
                    }
                }
                Some(Command::Dismiss { key }) => {
                    if let Err(error) = features.notifications.dismiss(Some(session), &key).await {
                        tracing::warn!(%error, "could not dismiss a notification");
                    }
                }
                Some(Command::Action { key, action_id, reply }) => {
                    let sent = features
                        .notifications
                        .run_action(Some(session), &key, &action_id, reply.as_deref())
                        .await;
                    if let Err(error) = sent {
                        tracing::warn!(%error, "could not run a notification action");
                    }
                }
                Some(Command::ClipboardSharing) => {
                    if let Err(error) = features.clipboard.sharing_changed(session).await {
                        tracing::debug!(%error, "could not tell the phone about clipboard sharing");
                    }
                }
                Some(Command::SendFiles { paths }) => {
                    if let Err(error) = features.files.send(Some(session), paths).await {
                        tracing::warn!(%error, "could not send files");
                    }
                }
                Some(Command::CancelTransfer { id }) => features.files.cancel(Some(session), id).await,
                Some(Command::ForgetTransfer { id }) => features.files.forget(id),
                Some(Command::FilesSwitch) => {
                    if let Err(error) = features.files.switched(session).await {
                        tracing::debug!(%error, "could not tell the phone about files");
                    }
                }
                Some(Command::Phone(command)) => features.phone_command(Some(session), command).await,
                Some(Command::Screen(command)) => features.screen_command(Some(session), command),
                Some(Command::CallsChanged) => {
                    if let Err(error) = features.calls_changed(Some(session)).await {
                        tracing::debug!(%error, "could not tell the phone about calls");
                    }
                }
                Some(Command::Reconnect | Command::NetworkChanged) => {}
            },
            event = session.next_event() => match event {
                Ok(Some(event)) => {
                    let Ok(rest) = features.handle_event(session, event).await else {
                        return Ending::PhoneBrokeRules;
                    };
                    if let Some(end) = rest.and_then(|event| handle(&event, context)) {
                        return Ending::Peer(end);
                    }
                }
                Ok(None) => return Ending::Peer(SessionEnd::Lost),
                Err(error) => {
                    tracing::debug!(%error, "the session failed");
                    return Ending::Peer(if error.is_protocol_error() {
                        SessionEnd::Backoff
                    } else {
                        SessionEnd::Lost
                    });
                }
            },
        }
        // Ping often while the user looks, or while files move.
        let active = window_active || features.is_busy();
        if session.is_active() != active
            && let Err(error) = session.set_active(active).await
        {
            tracing::debug!(%error, "could not set the session active");
        }
    }
}

/// Sleeps until `deadline`, or forever without one.
pub(crate) async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Handles an event the driver left. Returns how the session ended when
/// the event is a `Goodbye`.
fn handle(event: &SessionEvent, context: &LinkContext<'_>) -> Option<SessionEnd> {
    match event {
        SessionEvent::FeatureStatus(_) => {
            let confirmed = records::confirm(&mut *lock(context.store), &context.fingerprint);
            match confirmed {
                Ok(true) => (context.trust_changed)(),
                Ok(false) => {}
                Err(error) => tracing::warn!(%error, "could not confirm the pairing"),
            }
            None
        }
        SessionEvent::Presence(update) => {
            if !update.presence_key.is_empty() {
                let stored = records::set_presence_key(
                    &mut *lock(context.store),
                    &context.fingerprint,
                    &update.presence_key,
                );
                match stored {
                    Ok(()) => (context.trust_changed)(),
                    Err(error) => tracing::warn!(%error, "could not store the presence key"),
                }
            }
            None
        }
        SessionEvent::Goodbye(goodbye) => {
            let wire = Goodbye {
                reason: goodbye.reason.into(),
                detail: goodbye.detail.clone(),
            };
            let handled =
                records::handle_goodbye(&mut *lock(context.store), &context.fingerprint, &wire);
            match handled {
                Ok(true) => (context.trust_changed)(),
                Ok(false) => {}
                Err(error) => tracing::warn!(%error, "could not apply the goodbye"),
            }
            Some(SessionEnd::from_goodbye(Some(goodbye.reason)))
        }
        other => {
            tracing::trace!(event = other.name(), "an event nobody handles yet");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use link2linux_core::device::{DeviceInfo, DeviceKind, Feature};
    use link2linux_core::session::SessionConfig;
    use link2linux_protocol::Payload;
    use link2linux_protocol::v1::FileChunk;

    use super::*;

    fn device(id: &str, kind: DeviceKind) -> DeviceInfo {
        DeviceInfo::new(id, id, kind, [Feature::Files])
    }

    #[tokio::test(start_paused = true)]
    async fn a_phone_that_stopped_reading_is_not_waited_for_at_the_end() {
        let (here, there) = tokio::io::duplex(64 * 1024);
        let (desktop, phone) = tokio::join!(
            Session::handshake(
                here,
                device("desk", DeviceKind::Desktop),
                SessionConfig::default()
            ),
            Session::handshake(
                there,
                device("phone", DeviceKind::Phone),
                SessionConfig::phone()
            ),
        );
        let (mut desktop, _phone) = (desktop.unwrap(), phone.unwrap());
        while !desktop.is_ready(Feature::Files) {
            desktop.next_event().await.unwrap();
        }
        // A megabyte of chunks the phone never reads.
        for n in 0..4_u64 {
            let chunk = FileChunk {
                transfer_id: vec![1; 16],
                offset: n * 256 * 1024,
                data: vec![0; 256 * 1024],
                ..FileChunk::default()
            };
            desktop.queue_request(Payload::FileChunk(chunk)).unwrap();
        }

        let started = tokio::time::Instant::now();
        finish(desktop, &Ending::Shutdown).await;
        assert_eq!(started.elapsed(), FINISH_WITHIN);
    }

    #[tokio::test(start_paused = true)]
    async fn an_unpair_waits_longer_for_the_phone_to_take_the_goodbye() {
        let (here, there) = tokio::io::duplex(64 * 1024);
        let (desktop, phone) = tokio::join!(
            Session::handshake(
                here,
                device("desk", DeviceKind::Desktop),
                SessionConfig::default()
            ),
            Session::handshake(
                there,
                device("phone", DeviceKind::Phone),
                SessionConfig::phone()
            ),
        );
        let (mut desktop, _phone) = (desktop.unwrap(), phone.unwrap());
        while !desktop.is_ready(Feature::Files) {
            desktop.next_event().await.unwrap();
        }
        for n in 0..4_u64 {
            let chunk = FileChunk {
                transfer_id: vec![1; 16],
                offset: n * 256 * 1024,
                data: vec![0; 256 * 1024],
                ..FileChunk::default()
            };
            desktop.queue_request(Payload::FileChunk(chunk)).unwrap();
        }

        let started = tokio::time::Instant::now();
        finish(desktop, &Ending::Removed).await;
        assert_eq!(started.elapsed(), UNPAIR_FINISH_WITHIN);
    }

    #[test]
    fn only_a_removal_or_a_shutdown_discards_the_unfinished_files() {
        assert!(Ending::Removed.is_final());
        assert!(Ending::Shutdown.is_final());
        assert!(!Ending::PhoneBrokeRules.is_final());
        assert!(!Ending::Peer(SessionEnd::Lost).is_final());
    }
}
