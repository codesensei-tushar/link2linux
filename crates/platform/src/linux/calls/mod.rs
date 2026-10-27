//! Calls on Linux: BlueZ for the adapter, pairing and the hands-free
//! profile ([`BluezBluetooth`]), and PipeWire's telephony service for the
//! calls themselves ([`PipewireTelephony`]).
//!
//! Both are reached over D-Bus with hand-written zbus proxies. PipeWire's
//! Bluetooth plugin carries the audio (its native backend runs the
//! hands-free side of HFP), so this module never opens an RFCOMM or SCO
//! socket, nor registers a Bluetooth profile of its own.

mod agent;
mod bluez;
mod telephony;

use std::collections::HashMap;
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::{Select, select};
use tokio::sync::Mutex;
use zbus::message::Type;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, DBusError as _, MatchRule, Message, MessageStream};

pub use bluez::BluezBluetooth;
pub use telephony::PipewireTelephony;

use crate::PlatformError;

/// An object's interfaces and their properties, as `GetManagedObjects`
/// lists them.
type Interfaces = HashMap<String, HashMap<String, OwnedValue>>;
type Objects = HashMap<OwnedObjectPath, Interfaces>;

/// How long one call to a service may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Changes that come in a burst, such as a call's properties one by one,
/// are reported once they settle for this long.
const SETTLE: Duration = Duration::from_millis(50);

/// Takes property `key` out of `properties`, if it has the type `T`.
fn take<T: TryFrom<OwnedValue>>(
    properties: &mut HashMap<String, OwnedValue>,
    key: &str,
) -> Option<T> {
    properties
        .remove(key)
        .and_then(|value| T::try_from(value).ok())
}

/// What a failed call says, for logs and errors. A service's own message
/// may repeat what was sent, such as a number, so only the error's name is
/// kept.
fn describe(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
        zbus::Error::FDO(error) => match error.as_ref() {
            zbus::fdo::Error::ServiceUnknown(_) => "the service is not running".to_owned(),
            other => other.name().to_string(),
        },
        other => other.to_string(),
    }
}

/// The name of the D-Bus error `error` is, if it is one.
fn error_name(error: &zbus::Error) -> Option<String> {
    match error {
        zbus::Error::MethodError(name, _, _) => Some(name.to_string()),
        zbus::Error::FDO(error) => Some(error.name().to_string()),
        _ => None,
    }
}

/// Runs a call to a service, failing after `limit`.
async fn bounded<T>(
    limit: Duration,
    call: impl Future<Output = zbus::Result<T>>,
) -> Result<T, zbus::Error> {
    tokio::time::timeout(limit, call)
        .await
        .map_err(|_| zbus::Error::Failure("no answer in time".to_owned()))?
}

/// The signals of one service, and the bus's word when its name changes
/// owner, so a service that starts or restarts is noticed.
struct Signals {
    stream: Select<MessageStream, MessageStream>,
}

impl Signals {
    async fn new(connection: &Connection, service: &'static str) -> zbus::Result<Self> {
        let from_service = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(service)?
            .build();
        let owner = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender("org.freedesktop.DBus")?
            .interface("org.freedesktop.DBus")?
            .member("NameOwnerChanged")?
            .arg(0, service)?
            .build();
        let from_service =
            MessageStream::for_match_rule(from_service, connection, Some(64)).await?;
        let owner = MessageStream::for_match_rule(owner, connection, Some(8)).await?;
        Ok(Self {
            stream: select(from_service, owner),
        })
    }

    /// Waits for a signal that `relevant` keeps, then for the burst it
    /// starts to settle.
    async fn changed(&mut self, relevant: impl Fn(&Message) -> bool) -> Result<(), String> {
        loop {
            match self.stream.next().await {
                Some(Ok(message)) if is_owner_change(&message) || relevant(&message) => break,
                Some(_) => {}
                None => return Err("the D-Bus connection closed".to_owned()),
            }
        }
        while let Ok(Some(_)) = tokio::time::timeout(SETTLE, self.stream.next()).await {}
        Ok(())
    }
}

fn is_owner_change(message: &Message) -> bool {
    message
        .header()
        .member()
        .is_some_and(|member| member.as_str() == "NameOwnerChanged")
}

/// Waits on the [`Signals`] in `slot`, made on first use, for a change
/// `relevant` keeps. `service` names the service in errors.
async fn wait_for_change(
    slot: &Mutex<Option<Signals>>,
    connection: &Connection,
    bus_name: &'static str,
    service: &'static str,
    relevant: impl Fn(&Message) -> bool,
) -> Result<(), PlatformError> {
    let mut slot = slot.lock().await;
    let signals = match &mut *slot {
        Some(signals) => signals,
        None => slot.insert(
            Signals::new(connection, bus_name)
                .await
                .map_err(|e| unavailable(service, describe(&e)))?,
        ),
    };
    let result = signals.changed(relevant).await;
    if result.is_err() {
        *slot = None;
    }
    result.map_err(|reason| unavailable(service, reason))
}

fn unavailable(service: &'static str, reason: impl Into<String>) -> PlatformError {
    PlatformError::Unavailable {
        service,
        reason: reason.into(),
    }
}

/// `paths` in the order the service made them: `call2` before `call10`.
fn sort_paths<T>(objects: &mut [(OwnedObjectPath, T)]) {
    objects.sort_by(|a, b| {
        let (a, b) = (a.0.as_str(), b.0.as_str());
        (a.len(), a).cmp(&(b.len(), b))
    });
}

#[cfg(test)]
pub(crate) mod testing {
    //! A peer-to-peer D-Bus link between a mock service and the code under
    //! test, so no bus and no real service is involved.

    use tokio::net::UnixStream;
    use zbus::Connection;
    use zbus::connection::Builder;

    /// Connects a client to a server that `serve` sets up.
    pub async fn link(
        serve: impl FnOnce(Builder<'static>) -> zbus::Result<Builder<'static>>,
    ) -> (Connection, Connection) {
        let (a, b) = UnixStream::pair().unwrap();
        let server = serve(
            Builder::unix_stream(a)
                .server(zbus::Guid::generate())
                .unwrap()
                .p2p(),
        )
        .unwrap()
        .build();
        let client = Builder::unix_stream(b).p2p().build();
        tokio::try_join!(server, client).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_sort_in_creation_order() {
        let mut paths: Vec<(OwnedObjectPath, ())> = ["/a/call10", "/a/call2", "/a/call1"]
            .into_iter()
            .map(|p| (OwnedObjectPath::try_from(p).unwrap(), ()))
            .collect();
        sort_paths(&mut paths);
        let sorted: Vec<&str> = paths.iter().map(|(p, ())| p.as_str()).collect();
        assert_eq!(sorted, ["/a/call1", "/a/call2", "/a/call10"]);
    }

    #[test]
    fn method_errors_keep_only_their_name() {
        let error = zbus::Error::FDO(Box::new(zbus::fdo::Error::InvalidArgs(
            "+15550100 is not a number".into(),
        )));
        assert_eq!(describe(&error), "org.freedesktop.DBus.Error.InvalidArgs");
    }
}
