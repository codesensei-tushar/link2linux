//! A live session with a paired phone, from the handshake to its end.

use std::sync::{Mutex, MutexGuard, PoisonError};

use link2linux_core::discovery::reconnect::SessionEnd;
use link2linux_core::identity::CertificateFingerprint;
use link2linux_core::pairing::{FileTrustStore, records};
use link2linux_core::session::SessionEvent;
use link2linux_platform::NotificationResponse;
use link2linux_protocol::v1::{Goodbye, GoodbyeReason};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

use crate::clipboard::{Copies, Outgoing};
use crate::features::Features;
use crate::notify::{DriverError, PhoneSession};

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
    match &ending {
        Ending::Shutdown => {
            let _ = session
                .goodbye(GoodbyeReason::Shutdown, "the desktop is closing")
                .await;
        }
        Ending::Removed => {
            let _ = session
                .goodbye(GoodbyeReason::Unpaired, "the user removed the pairing")
                .await;
        }
        Ending::PhoneBrokeRules => {
            let _ = session
                .goodbye(
                    GoodbyeReason::ProtocolError,
                    "a feature's rules were broken",
                )
                .await;
        }
        Ending::Peer(_) => {
            let _ = session.close().await;
        }
    }
    ending
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
    loop {
        tokio::select! {
            () = cancel.cancelled() => return Ending::Shutdown,
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
                Some(Command::SetActive(active)) => {
                    if let Err(error) = session.set_active(active).await {
                        tracing::debug!(%error, "could not set the session active");
                    }
                }
                Some(Command::Response(response)) => {
                    match features.notifications.handle_response(Some(session), response).await {
                        Ok(Some(answer)) => tracing::debug!(?answer, "a banner asks to open the app"),
                        Ok(None) => {}
                        Err(DriverError::PhoneBrokeRules(error)) => {
                            tracing::warn!(%error, "the phone broke the notification rules");
                            return Ending::PhoneBrokeRules;
                        }
                        Err(error) => tracing::warn!(%error, "notification response failed"),
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
            tracing::trace!(?other, "an event nobody handles yet");
            None
        }
    }
}
