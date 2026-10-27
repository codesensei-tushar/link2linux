//! Hands-free call control through PipeWire's telephony service
//! (`org.pipewire.Telephony` on the session bus, PipeWire 1.4 and later).
//!
//! The service lists one audio gateway per phone connected over the
//! Hands-Free Profile at `/org/pipewire/Telephony/agN`, and each gateway's
//! calls below it at `.../callM`. Older PipeWire releases carry hands-free
//! audio but not this service, so calls are then unavailable.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::{Mutex, OnceCell};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Proxy};

use super::{
    CALL_TIMEOUT, Interfaces, Objects, Signals, bounded, describe, sort_paths, take, unavailable,
    wait_for_change,
};
use crate::{CallState, Gateway, GatewayCall, PlatformError, Telephony};

const TELEPHONY: &str = "org.pipewire.Telephony";
const MANAGER_PATH: &str = "/org/pipewire/Telephony";
const GATEWAY: &str = "org.pipewire.Telephony.AudioGateway1";
const CALL: &str = "org.pipewire.Telephony.Call1";
/// The oFono interface some PipeWire releases offer for the microphone.
const CALL_VOLUME: &str = "org.ofono.CallVolume";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const SERVICE: &str = "Hands-free call control";
/// The phone answers a dial once the call is set up on its side.
const DIAL_TIMEOUT: Duration = Duration::from_secs(30);

/// [`Telephony`] through PipeWire.
pub struct PipewireTelephony {
    /// The connection tests give; the session bus otherwise.
    given: Option<Connection>,
    connection: OnceCell<Connection>,
    signals: Mutex<Option<Signals>>,
}

impl std::fmt::Debug for PipewireTelephony {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipewireTelephony").finish_non_exhaustive()
    }
}

impl Default for PipewireTelephony {
    fn default() -> Self {
        Self::new()
    }
}

impl PipewireTelephony {
    /// The service on the session bus, which is opened on first use.
    #[must_use]
    pub fn new() -> Self {
        Self::with(None)
    }

    /// The service on `connection`, for tests with a mock.
    #[must_use]
    pub fn with_connection(connection: Connection) -> Self {
        Self::with(Some(connection))
    }

    fn with(given: Option<Connection>) -> Self {
        Self {
            given,
            connection: OnceCell::new(),
            signals: Mutex::new(None),
        }
    }

    async fn connect(&self) -> Result<Connection, PlatformError> {
        if let Some(connection) = &self.given {
            return Ok(connection.clone());
        }
        self.connection
            .get_or_try_init(|| async {
                Connection::session()
                    .await
                    .map_err(|e| unavailable(SERVICE, format!("no session D-Bus: {e}")))
            })
            .await
            .cloned()
    }

    /// Calls `method` on the service's object `path`, which the
    /// service's methods answer with nothing.
    async fn call<B>(
        &self,
        path: &str,
        interface: &'static str,
        method: &'static str,
        body: &B,
        limit: Duration,
    ) -> Result<(), PlatformError>
    where
        B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType + Sync,
    {
        let path = owned_path(path)?;
        let connection = self.connect().await?;
        let proxy = Proxy::new(&connection, TELEPHONY, path, interface)
            .await
            .map_err(|e| unavailable(SERVICE, describe(&e)))?;
        bounded(limit, proxy.call::<_, _, ()>(method, body))
            .await
            .map_err(|e| unavailable(SERVICE, format!("{method} failed: {}", describe(&e))))
    }
}

/// `id` as one of the service's object paths. Ids come from the UI, so
/// anything else is refused rather than called.
fn owned_path(id: &str) -> Result<OwnedObjectPath, PlatformError> {
    id.strip_prefix(MANAGER_PATH)
        .filter(|rest| rest.starts_with("/ag"))
        .and_then(|_| OwnedObjectPath::try_from(id).ok())
        .ok_or_else(|| unavailable(SERVICE, "not a gateway or call of the service"))
}

async fn managed_objects(connection: &Connection, path: ObjectPath<'_>) -> zbus::Result<Objects> {
    let manager = Proxy::new(connection, TELEPHONY, path.to_owned(), OBJECT_MANAGER).await?;
    bounded(CALL_TIMEOUT, manager.call("GetManagedObjects", &())).await
}

fn call_of(
    path: &OwnedObjectPath,
    mut properties: HashMap<String, OwnedValue>,
) -> Option<GatewayCall> {
    let state: String = take(&mut properties, "State")?;
    Some(GatewayCall {
        id: path.to_string(),
        number: take(&mut properties, "LineIdentification").unwrap_or_default(),
        name: take(&mut properties, "Name").unwrap_or_default(),
        state: CallState::from_name(&state)?,
        multiparty: take(&mut properties, "Multiparty").unwrap_or(false),
    })
}

/// Whether the microphone is muted for `gateway`'s calls, where the
/// service can say.
async fn muted(connection: &Connection, gateway: &OwnedObjectPath) -> Option<bool> {
    let read = async {
        let volume = Proxy::new(connection, TELEPHONY, gateway.clone(), CALL_VOLUME).await?;
        bounded(
            CALL_TIMEOUT,
            volume.call::<_, _, HashMap<String, OwnedValue>>("GetProperties", &()),
        )
        .await
    };
    let mut properties = read.await.ok()?;
    take(&mut properties, "Muted")
}

impl Telephony for PipewireTelephony {
    async fn gateways(&self) -> Result<Vec<Gateway>, PlatformError> {
        let connection = self.connect().await?;
        let manager =
            ObjectPath::try_from(MANAGER_PATH).map_err(|e| unavailable(SERVICE, e.to_string()))?;
        let objects = managed_objects(&connection, manager)
            .await
            .map_err(|e| unavailable(SERVICE, describe(&e)))?;
        let (mut gateways, mut calls): (Vec<(OwnedObjectPath, Interfaces)>, Vec<_>) = objects
            .into_iter()
            .filter(|(_, interfaces)| {
                interfaces.contains_key(GATEWAY) || interfaces.contains_key(CALL)
            })
            .partition(|(_, interfaces)| interfaces.contains_key(GATEWAY));
        sort_paths(&mut gateways);
        let mut found = Vec::with_capacity(gateways.len());
        for (path, mut interfaces) in gateways {
            let mut properties = interfaces.remove(GATEWAY).unwrap_or_default();
            let address: Option<String> = take(&mut properties, "Address");
            // Each gateway lists its own calls. The manager may list them
            // too; both are taken, once each.
            match managed_objects(&connection, path.as_ref()).await {
                Ok(own) => {
                    for (call, interfaces) in own {
                        if interfaces.contains_key(CALL) && !calls.iter().any(|(p, _)| p == &call) {
                            calls.push((call, interfaces));
                        }
                    }
                }
                Err(error) => {
                    tracing::debug!(error = %describe(&error), "a gateway did not list its calls");
                }
            }
            let prefix = format!("{}/", path.as_str());
            let mut own: Vec<(OwnedObjectPath, Interfaces)> = Vec::new();
            calls.retain(|(call, interfaces)| {
                if call.as_str().starts_with(&prefix) {
                    own.push((call.clone(), interfaces.clone()));
                    false
                } else {
                    true
                }
            });
            sort_paths(&mut own);
            found.push(Gateway {
                address: address.and_then(|address| address.parse().ok()),
                calls: own
                    .into_iter()
                    .filter_map(|(call, mut interfaces)| call_of(&call, interfaces.remove(CALL)?))
                    .collect(),
                muted: muted(&connection, &path).await,
                id: path.to_string(),
            });
        }
        Ok(found)
    }

    async fn changed(&self) -> Result<(), PlatformError> {
        let connection = self.connect().await?;
        // Every signal of the service is about its gateways or calls.
        wait_for_change(&self.signals, &connection, TELEPHONY, SERVICE, |_| true).await
    }

    async fn answer(&self, call: &str) -> Result<(), PlatformError> {
        self.call(call, CALL, "Answer", &(), CALL_TIMEOUT).await
    }

    async fn hang_up(&self, call: &str) -> Result<(), PlatformError> {
        self.call(call, CALL, "Hangup", &(), CALL_TIMEOUT).await
    }

    async fn dial(&self, gateway: &str, number: &str) -> Result<(), PlatformError> {
        self.call(gateway, GATEWAY, "Dial", &(number,), DIAL_TIMEOUT)
            .await
    }

    async fn send_tones(&self, gateway: &str, tones: &str) -> Result<(), PlatformError> {
        self.call(gateway, GATEWAY, "SendTones", &(tones,), CALL_TIMEOUT)
            .await
    }

    async fn set_muted(&self, gateway: &str, muted: bool) -> Result<(), PlatformError> {
        self.call(
            gateway,
            CALL_VOLUME,
            "SetProperty",
            &("Muted", Value::from(muted)),
            CALL_TIMEOUT,
        )
        .await
    }
}

#[cfg(test)]
mod tests;
