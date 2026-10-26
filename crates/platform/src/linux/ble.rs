//! Bluetooth LE presence advertisement through BlueZ.
//!
//! The desktop exports an `org.bluez.LEAdvertisement1` object (type
//! `peripheral`, the link2linux service UUID, the device name, general
//! discoverable) and registers it with the adapter's
//! `org.bluez.LEAdvertisingManager1`. No GATT service is offered.
//!
//! Which advertising PDU carries what is up to BlueZ: it builds the
//! advertising data (flags and the 128-bit UUID) and puts the name in the
//! scan response when it does not fit. BlueZ cannot be told to use a
//! throwaway address for one advertisement; while adapter privacy is off
//! (the default) a connectable advertisement uses the identity address, and
//! the adapter's `AddressType` says whether that is public. A random
//! address is refused, since the phone could not follow it.

use std::collections::HashMap;

use tokio::sync::Mutex;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Proxy};

use crate::{
    AddressKind, BluetoothAddress, PRESENCE_SERVICE_UUID, PlatformError, PresenceAdapter,
    PresenceAdvertiser, truncate_device_name,
};

const SERVICE: &str = "Bluetooth LE presence";
const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const ADVERTISING_MANAGER: &str = "org.bluez.LEAdvertisingManager1";
/// How long to wait for BlueZ to list its objects.
const BLUEZ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const ADVERTISEMENT_PATH: &str = "/org/link2linux/advertisement0";

type Interfaces = HashMap<String, HashMap<String, OwnedValue>>;

fn unavailable(reason: impl Into<String>) -> PlatformError {
    PlatformError::Unavailable {
        service: SERVICE,
        reason: reason.into(),
    }
}

fn dbus(error: impl std::fmt::Display) -> PlatformError {
    unavailable(error.to_string())
}

/// The object BlueZ reads to learn what to advertise.
struct Advertisement {
    local_name: String,
}

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl Advertisement {
    /// BlueZ calls this when it drops the advertisement. Nothing to clean
    /// up: the object stays exported until [`PresenceAdvertiser::stop`].
    fn release(&self) {
        tracing::debug!("BlueZ released the presence advertisement");
    }

    #[zbus(property, name = "Type")]
    fn advertisement_type(&self) -> String {
        "peripheral".to_owned()
    }

    #[zbus(property, name = "ServiceUUIDs")]
    fn service_uuids(&self) -> Vec<String> {
        vec![PRESENCE_SERVICE_UUID.to_owned()]
    }

    #[zbus(property, name = "LocalName")]
    fn local_name(&self) -> String {
        self.local_name.clone()
    }

    #[zbus(property, name = "Discoverable")]
    fn discoverable(&self) -> bool {
        true
    }
}

struct Active {
    connection: Connection,
    adapter_path: OwnedObjectPath,
}

/// Presence advertising through BlueZ over D-Bus.
pub struct BluezPresence {
    /// Where to reach BlueZ; the system bus unless given.
    connection: Option<Connection>,
    active: Mutex<Option<Active>>,
}

impl std::fmt::Debug for BluezPresence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BluezPresence").finish_non_exhaustive()
    }
}

impl Default for BluezPresence {
    fn default() -> Self {
        Self::new()
    }
}

impl BluezPresence {
    /// Advertises through BlueZ on the system bus, which is opened when
    /// advertising starts.
    #[must_use]
    pub fn new() -> Self {
        Self {
            connection: None,
            active: Mutex::new(None),
        }
    }

    /// Advertises through BlueZ on `connection`, for tests with a mock.
    #[must_use]
    pub fn with_connection(connection: Connection) -> Self {
        Self {
            connection: Some(connection),
            active: Mutex::new(None),
        }
    }

    async fn connect(&self) -> Result<Connection, PlatformError> {
        match &self.connection {
            Some(connection) => Ok(connection.clone()),
            None => Connection::system()
                .await
                .map_err(|e| unavailable(format!("no system D-Bus: {e}"))),
        }
    }
}

/// An adapter BlueZ manages.
struct FoundAdapter {
    path: OwnedObjectPath,
    info: PresenceAdapter,
    powered: bool,
}

async fn find_adapter(connection: &Connection) -> Result<FoundAdapter, PlatformError> {
    let manager = Proxy::new(connection, BLUEZ, "/", "org.freedesktop.DBus.ObjectManager")
        .await
        .map_err(dbus)?;
    let objects: HashMap<OwnedObjectPath, Interfaces> =
        tokio::time::timeout(BLUEZ_TIMEOUT, manager.call("GetManagedObjects", &()))
            .await
            .map_err(|_| unavailable("BlueZ did not answer"))?
            .map_err(|e| unavailable(format!("BlueZ is not running: {e}")))?;
    let mut adapters: Vec<_> = objects
        .into_iter()
        .filter(|(_, interfaces)| {
            interfaces.contains_key(ADAPTER) && interfaces.contains_key(ADVERTISING_MANAGER)
        })
        .collect();
    adapters.sort_by(|a, b| a.0.cmp(&b.0));
    let Some((path, mut interfaces)) = adapters.into_iter().next() else {
        return Err(unavailable("no Bluetooth adapter can advertise"));
    };
    let mut properties = interfaces.remove(ADAPTER).unwrap_or_default();
    let address: String = take(&mut properties, "Address")
        .ok_or_else(|| unavailable("the adapter reports no address"))?;
    let address: BluetoothAddress = address
        .parse()
        .map_err(|_| unavailable(format!("the adapter's address {address:?} is not valid")))?;
    let address_kind = match take::<String>(&mut properties, "AddressType").as_deref() {
        Some("public") => AddressKind::Public,
        Some("random") => AddressKind::Random,
        _ => AddressKind::Unknown,
    };
    let powered = take(&mut properties, "Powered").unwrap_or(false);
    let name = path.as_str().rsplit('/').next().unwrap_or("").to_owned();
    Ok(FoundAdapter {
        path,
        info: PresenceAdapter {
            address,
            address_kind,
            name,
        },
        powered,
    })
}

fn take<T: TryFrom<OwnedValue>>(
    properties: &mut HashMap<String, OwnedValue>,
    key: &str,
) -> Option<T> {
    properties
        .remove(key)
        .and_then(|value| T::try_from(value).ok())
}

impl PresenceAdvertiser for BluezPresence {
    async fn start(&self, device_name: &str) -> Result<PresenceAdapter, PlatformError> {
        self.stop().await?;
        let connection = self.connect().await?;
        let adapter = find_adapter(&connection).await?;
        if !adapter.powered {
            return Err(unavailable(format!(
                "the Bluetooth adapter {} is powered off",
                adapter.info.name
            )));
        }
        if adapter.info.address_kind == AddressKind::Random {
            return Err(unavailable(
                "the Bluetooth adapter uses a random address, which the phone cannot follow; \
                 turn off Bluetooth privacy for the adapter",
            ));
        }

        let advertisement = Advertisement {
            local_name: truncate_device_name(device_name).to_owned(),
        };
        let served = connection
            .object_server()
            .at(ADVERTISEMENT_PATH, advertisement)
            .await
            .map_err(dbus)?;
        if !served {
            return Err(unavailable("the advertisement object is already exported"));
        }
        let manager = Proxy::new(
            &connection,
            BLUEZ,
            adapter.path.clone(),
            ADVERTISING_MANAGER,
        )
        .await
        .map_err(dbus)?;
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        let registered: zbus::Result<()> = manager
            .call(
                "RegisterAdvertisement",
                &(
                    zbus::zvariant::ObjectPath::try_from(ADVERTISEMENT_PATH).map_err(dbus)?,
                    options,
                ),
            )
            .await;
        if let Err(e) = registered {
            remove_advertisement(&connection).await;
            return Err(unavailable(format!("BlueZ refused the advertisement: {e}")));
        }
        *self.active.lock().await = Some(Active {
            connection,
            adapter_path: adapter.path,
        });
        Ok(adapter.info)
    }

    async fn stop(&self) -> Result<(), PlatformError> {
        let Some(active) = self.active.lock().await.take() else {
            return Ok(());
        };
        let result = async {
            let manager = Proxy::new(
                &active.connection,
                BLUEZ,
                active.adapter_path.clone(),
                ADVERTISING_MANAGER,
            )
            .await?;
            let path = zbus::zvariant::ObjectPath::try_from(ADVERTISEMENT_PATH)?;
            manager
                .call::<_, _, ()>("UnregisterAdvertisement", &(path,))
                .await
        }
        .await;
        remove_advertisement(&active.connection).await;
        // BlueZ may already have dropped it (adapter removed, bluetoothd
        // restarted); the advertisement is gone either way.
        if let Err(e) = result {
            tracing::debug!(%e, "unregistering the presence advertisement failed");
        }
        Ok(())
    }
}

async fn remove_advertisement(connection: &Connection) {
    // Whether it was exported does not matter here.
    let _ = connection
        .object_server()
        .remove::<Advertisement, _>(ADVERTISEMENT_PATH)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::net::UnixStream;
    use zbus::connection::Builder;
    use zbus::object_server::SignalEmitter;

    /// What the mock BlueZ saw.
    #[derive(Debug, Default)]
    struct Seen {
        registered: Vec<String>,
        unregistered: Vec<String>,
        advertisement: HashMap<String, OwnedValue>,
    }

    struct MockObjectManager {
        address_type: Option<&'static str>,
        powered: bool,
        with_manager: bool,
    }

    #[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
    impl MockObjectManager {
        fn get_managed_objects(&self) -> HashMap<OwnedObjectPath, Interfaces> {
            let mut adapter: HashMap<String, OwnedValue> = HashMap::new();
            adapter.insert(
                "Address".into(),
                Value::from("0C:1A:2B:3C:4D:5E").try_into().unwrap(),
            );
            adapter.insert(
                "Powered".into(),
                Value::from(self.powered).try_into().unwrap(),
            );
            if let Some(kind) = self.address_type {
                adapter.insert("AddressType".into(), Value::from(kind).try_into().unwrap());
            }
            let mut interfaces: Interfaces = HashMap::new();
            interfaces.insert(ADAPTER.into(), adapter);
            if self.with_manager {
                interfaces.insert(ADVERTISING_MANAGER.into(), HashMap::new());
            }
            let mut objects = HashMap::new();
            objects.insert(
                OwnedObjectPath::try_from("/org/bluez/hci0").unwrap(),
                interfaces,
            );
            objects
        }
    }

    struct MockAdvertisingManager {
        seen: Arc<StdMutex<Seen>>,
        refuse: bool,
    }

    #[zbus::interface(name = "org.bluez.LEAdvertisingManager1")]
    impl MockAdvertisingManager {
        async fn register_advertisement(
            &self,
            path: zbus::zvariant::ObjectPath<'_>,
            _options: HashMap<String, OwnedValue>,
            #[zbus(connection)] connection: &Connection,
            #[zbus(signal_emitter)] _emitter: SignalEmitter<'_>,
        ) -> zbus::fdo::Result<()> {
            if self.refuse {
                return Err(zbus::fdo::Error::Failed(
                    "Maximum advertisements reached".into(),
                ));
            }
            // Read the advertisement back the way BlueZ does.
            let properties = Proxy::new(
                connection,
                BLUEZ,
                path.clone(),
                "org.freedesktop.DBus.Properties",
            )
            .await?;
            let all: HashMap<String, OwnedValue> = properties
                .call("GetAll", &("org.bluez.LEAdvertisement1",))
                .await?;
            let mut seen = self.seen.lock().unwrap();
            seen.registered.push(path.to_string());
            seen.advertisement = all;
            Ok(())
        }

        fn unregister_advertisement(&self, path: zbus::zvariant::ObjectPath<'_>) {
            self.seen
                .lock()
                .unwrap()
                .unregistered
                .push(path.to_string());
        }
    }

    struct Mock {
        client: Connection,
        _server: Connection,
        seen: Arc<StdMutex<Seen>>,
    }

    async fn mock(
        address_type: Option<&'static str>,
        powered: bool,
        with_manager: bool,
        refuse: bool,
    ) -> Mock {
        let seen = Arc::new(StdMutex::new(Seen::default()));
        let (a, b) = UnixStream::pair().unwrap();
        let server = Builder::unix_stream(a)
            .server(zbus::Guid::generate())
            .unwrap()
            .p2p()
            .serve_at(
                "/",
                MockObjectManager {
                    address_type,
                    powered,
                    with_manager,
                },
            )
            .unwrap()
            .serve_at(
                "/org/bluez/hci0",
                MockAdvertisingManager {
                    seen: seen.clone(),
                    refuse,
                },
            )
            .unwrap()
            .build();
        let client = Builder::unix_stream(b).p2p().build();
        let (server, client) = tokio::try_join!(server, client).unwrap();
        Mock {
            client,
            _server: server,
            seen,
        }
    }

    fn string(value: &OwnedValue) -> String {
        String::try_from(value.clone()).unwrap()
    }

    #[tokio::test]
    async fn registers_the_advertisement_bluez_reads() {
        let m = mock(Some("public"), true, true, false).await;
        let presence = BluezPresence::with_connection(m.client.clone());
        let adapter = presence.start("Büro workstation").await.unwrap();
        assert_eq!(adapter.address.to_hex(), "0C1A2B3C4D5E");
        assert_eq!(adapter.address_kind, AddressKind::Public);
        assert_eq!(adapter.name, "hci0");

        let seen = m.seen.lock().unwrap();
        assert_eq!(seen.registered, vec![ADVERTISEMENT_PATH]);
        assert_eq!(string(&seen.advertisement["Type"]), "peripheral");
        assert_eq!(string(&seen.advertisement["LocalName"]), "Büro workstation");
        assert_eq!(
            Vec::<String>::try_from(seen.advertisement["ServiceUUIDs"].clone()).unwrap(),
            vec![PRESENCE_SERVICE_UUID]
        );
        assert!(bool::try_from(seen.advertisement["Discoverable"].clone()).unwrap());
        assert!(!seen.advertisement.contains_key("ManufacturerData"));
    }

    #[tokio::test]
    async fn long_names_are_cut_for_the_scan_response() {
        let m = mock(Some("public"), true, true, false).await;
        let presence = BluezPresence::with_connection(m.client.clone());
        presence.start(&"ü".repeat(30)).await.unwrap();
        let seen = m.seen.lock().unwrap();
        assert_eq!(string(&seen.advertisement["LocalName"]), "ü".repeat(14));
    }

    #[tokio::test]
    async fn stop_unregisters_and_is_idempotent() {
        let m = mock(Some("public"), true, true, false).await;
        let presence = BluezPresence::with_connection(m.client.clone());
        presence.stop().await.unwrap();
        presence.start("x").await.unwrap();
        presence.stop().await.unwrap();
        presence.stop().await.unwrap();
        assert_eq!(
            m.seen.lock().unwrap().unregistered,
            vec![ADVERTISEMENT_PATH]
        );
        // It can be started again.
        presence.start("x").await.unwrap();
    }

    #[tokio::test]
    async fn an_unknown_address_type_is_reported_not_refused() {
        let m = mock(None, true, true, false).await;
        let presence = BluezPresence::with_connection(m.client.clone());
        let adapter = presence.start("x").await.unwrap();
        assert_eq!(adapter.address_kind, AddressKind::Unknown);
    }

    async fn reason(m: &Mock) -> String {
        let presence = BluezPresence::with_connection(m.client.clone());
        match presence.start("x").await {
            Err(PlatformError::Unavailable { reason, .. }) => reason,
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn refuses_when_it_cannot_advertise() {
        assert!(
            reason(&mock(Some("random"), true, true, false).await)
                .await
                .contains("random address")
        );
        assert!(
            reason(&mock(Some("public"), false, true, false).await)
                .await
                .contains("powered off")
        );
        assert!(
            reason(&mock(Some("public"), true, false, false).await)
                .await
                .contains("no Bluetooth adapter")
        );
        let refused = mock(Some("public"), true, true, true).await;
        assert!(reason(&refused).await.contains("refused"));
        // A refused registration leaves nothing exported: retrying works
        // the same way and does not fail with "already exported".
        assert!(reason(&refused).await.contains("refused"));
    }

    #[tokio::test]
    async fn no_bluez_on_the_bus_is_unavailable() {
        // A peer that serves nothing: no ObjectManager.
        let (a, b) = UnixStream::pair().unwrap();
        let server = Builder::unix_stream(a)
            .server(zbus::Guid::generate())
            .unwrap()
            .p2p()
            .serve_at(
                "/unrelated",
                MockAdvertisingManager {
                    seen: Arc::default(),
                    refuse: false,
                },
            )
            .unwrap()
            .build();
        let client = Builder::unix_stream(b).p2p().build();
        let (_server, client) = tokio::try_join!(server, client).unwrap();
        let presence = BluezPresence::with_connection(client);
        assert!(matches!(
            presence.start("x").await,
            Err(PlatformError::Unavailable { .. })
        ));
    }

    /// Needs a powered Bluetooth adapter and BlueZ on the system bus; it
    /// advertises for a moment.
    #[tokio::test]
    #[ignore = "needs a powered Bluetooth adapter"]
    async fn advertises_on_the_real_adapter() {
        let presence = BluezPresence::new();
        let adapter = presence.start("link2linux test").await.unwrap();
        println!("advertising from {adapter:?}");
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        presence.stop().await.unwrap();
    }
}
