//! The desktop's Bluetooth through BlueZ on the system bus.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, OnceCell};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Message, Proxy};

use super::agent::{AGENT_PATH, Agent, AgentShared, CAPABILITY};
use super::{
    CALL_TIMEOUT, Interfaces, Objects, Signals, bounded, describe, error_name, take, unavailable,
    wait_for_change,
};
use crate::{
    Bluetooth, BluetoothAdapter, BluetoothAddress, BluetoothDevice, HANDS_FREE_GATEWAY_UUID,
    PairingEvent, PlatformError,
};

pub(super) const BLUEZ: &str = "org.bluez";
pub(super) const DEVICE: &str = "org.bluez.Device1";
const ADAPTER: &str = "org.bluez.Adapter1";
const AGENT_MANAGER: &str = "org.bluez.AgentManager1";
const SERVICE: &str = "Bluetooth";
/// Connecting a profile waits for the phone, which may be out of reach.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// The major device class of phones, in bits 8 to 12 of `Class`.
const MAJOR_CLASS_PHONE: u32 = 0x02;

/// The adapter's state before pairing opened, put back when it closes.
struct Opened {
    adapter: OwnedObjectPath,
    discoverable: bool,
    discoverable_timeout: u32,
    pairable: bool,
    /// The earlier `PairableTimeout`, when opening pairing changed it.
    pairable_timeout: Option<u32>,
    /// The devices paired in any way when pairing opened.
    paired: HashSet<BluetoothAddress>,
    /// The devices bonded when pairing opened.
    bonded: HashSet<BluetoothAddress>,
}

impl Opened {
    /// Whether the device with `properties` paired, or bonded, since
    /// pairing opened without a code the user confirmed here (`confirmed`
    /// says whose they did), and nobody trusted it since. The kernel
    /// accepts "just works" pairing by itself, without asking any agent,
    /// so such a device paired with no prompt at all; the desktop's
    /// Bluetooth settings trust a device the user paired there.
    fn stranger(
        &self,
        properties: &Properties,
        confirmed: impl Fn(BluetoothAddress) -> bool,
    ) -> bool {
        let Some(address) = address(properties) else {
            return false;
        };
        let paired_since = (any_pairing(properties) && !self.paired.contains(&address))
            || (bonded(properties) && !self.bonded.contains(&address));
        paired_since && !confirmed(address) && !flag(properties, "Trusted")
    }
}

type Properties = HashMap<String, OwnedValue>;

/// [`Bluetooth`] through BlueZ.
///
/// While pairing is open, the desktop's agent is BlueZ's default one, so
/// it takes the requests of phones that pair with the desktop; the agent
/// that was the default before (the desktop's own Bluetooth settings, for
/// example) is the default again once pairing closes.
pub struct BluezBluetooth {
    /// The connection tests give; the system bus otherwise.
    given: Option<Connection>,
    connection: OnceCell<Connection>,
    signals: Mutex<Option<Signals>>,
    agent: Arc<AgentShared>,
    opened: Mutex<Option<Opened>>,
}

impl std::fmt::Debug for BluezBluetooth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BluezBluetooth").finish_non_exhaustive()
    }
}

impl Default for BluezBluetooth {
    fn default() -> Self {
        Self::new()
    }
}

impl BluezBluetooth {
    /// BlueZ on the system bus, which is opened on first use.
    #[must_use]
    pub fn new() -> Self {
        Self::with(None)
    }

    /// BlueZ on `connection`, for tests with a mock.
    #[must_use]
    pub fn with_connection(connection: Connection) -> Self {
        Self::with(Some(connection))
    }

    fn with(given: Option<Connection>) -> Self {
        Self {
            given,
            connection: OnceCell::new(),
            signals: Mutex::new(None),
            agent: Arc::new(AgentShared::new()),
            opened: Mutex::new(None),
        }
    }

    async fn connect(&self) -> Result<Connection, PlatformError> {
        if let Some(connection) = &self.given {
            return Ok(connection.clone());
        }
        self.connection
            .get_or_try_init(|| async {
                Connection::system()
                    .await
                    .map_err(|e| unavailable(SERVICE, format!("no system D-Bus: {e}")))
            })
            .await
            .cloned()
    }

    async fn objects(&self, connection: &Connection) -> Result<Objects, PlatformError> {
        let manager = Proxy::new(connection, BLUEZ, "/", "org.freedesktop.DBus.ObjectManager")
            .await
            .map_err(|e| unavailable(SERVICE, describe(&e)))?;
        bounded(CALL_TIMEOUT, manager.call("GetManagedObjects", &()))
            .await
            .map_err(|e| unavailable(SERVICE, format!("BlueZ: {}", describe(&e))))
    }

    /// The adapter and its devices' objects.
    async fn adapter_objects(
        &self,
        connection: &Connection,
    ) -> Result<Option<(OwnedObjectPath, Interfaces, Objects)>, PlatformError> {
        let mut objects = self.objects(connection).await?;
        let mut adapters: Vec<OwnedObjectPath> = objects
            .iter()
            .filter(|(_, interfaces)| interfaces.contains_key(ADAPTER))
            .map(|(path, _)| path.clone())
            .collect();
        adapters.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        let Some(path) = adapters.into_iter().next() else {
            return Ok(None);
        };
        let adapter = objects.remove(&path).unwrap_or_default();
        Ok(Some((path, adapter, objects)))
    }

    async fn device_path(
        &self,
        connection: &Connection,
        device: BluetoothAddress,
    ) -> Result<OwnedObjectPath, PlatformError> {
        Ok(self.device_object(connection, device).await?.0)
    }

    /// The object of `device` and its properties.
    async fn device_object(
        &self,
        connection: &Connection,
        device: BluetoothAddress,
    ) -> Result<(OwnedObjectPath, Properties), PlatformError> {
        let (adapter, _, objects) = self
            .adapter_objects(connection)
            .await?
            .ok_or_else(|| unavailable(SERVICE, "no Bluetooth adapter"))?;
        objects
            .into_iter()
            .filter_map(|(path, mut interfaces)| Some((path, interfaces.remove(DEVICE)?)))
            .find(|(_, properties)| {
                belongs_to(properties, &adapter) && address(properties) == Some(device)
            })
            .ok_or_else(|| unavailable(SERVICE, "the device is not known to the adapter"))
    }

    /// Restores what [`Bluetooth::open_pairing`] changed, drops the agent,
    /// and removes the devices that paired meanwhile without the user.
    async fn undo(&self, connection: &Connection, opened: Opened) {
        // Closing the agent forgets whom the user confirmed.
        let confirmed = self.agent.confirmed();
        self.agent.close();
        let adapter = opened.adapter.as_ref();
        // Setting the timeout restarts BlueZ's timer on a discoverable
        // adapter, which may have run out meanwhile, so Discoverable is set
        // after it when it was on, and before it when it was off.
        if opened.discoverable {
            set(
                connection,
                adapter.clone(),
                ADAPTER,
                "DiscoverableTimeout",
                opened.discoverable_timeout,
            )
            .await;
            set(connection, adapter.clone(), ADAPTER, "Discoverable", true).await;
        } else {
            set(connection, adapter.clone(), ADAPTER, "Discoverable", false).await;
            set(
                connection,
                adapter.clone(),
                ADAPTER,
                "DiscoverableTimeout",
                opened.discoverable_timeout,
            )
            .await;
        }
        if !opened.pairable {
            set(connection, adapter.clone(), ADAPTER, "Pairable", false).await;
        }
        if let Some(timeout) = opened.pairable_timeout {
            set(connection, adapter, ADAPTER, "PairableTimeout", timeout).await;
        }
        drop_agent(connection).await;
        self.remove_strangers(connection, &opened, &confirmed).await;
    }

    /// Removes the devices that paired while pairing was open without the
    /// user (see [`Opened::stranger`]).
    async fn remove_strangers(
        &self,
        connection: &Connection,
        opened: &Opened,
        confirmed: &HashSet<BluetoothAddress>,
    ) {
        let objects = match self.objects(connection).await {
            Ok(objects) => objects,
            Err(error) => {
                tracing::warn!(%error, "could not look for devices that paired unasked");
                return;
            }
        };
        let strangers: Vec<(OwnedObjectPath, BluetoothAddress)> = objects
            .into_iter()
            .filter_map(|(path, mut interfaces)| {
                let properties = interfaces.remove(DEVICE)?;
                (belongs_to(&properties, &opened.adapter)
                    && opened.stranger(&properties, |a| confirmed.contains(&a)))
                .then_some((path, address(&properties)?))
            })
            .collect();
        if strangers.is_empty() {
            return;
        }
        let adapter = match Proxy::new(connection, BLUEZ, opened.adapter.clone(), ADAPTER).await {
            Ok(adapter) => adapter,
            Err(error) => {
                tracing::warn!(error = %describe(&error), "could not remove devices that paired unasked");
                return;
            }
        };
        for (path, address) in strangers {
            tracing::warn!(%address, "removing a device that paired without a confirmed code");
            let removed = bounded(
                CALL_TIMEOUT,
                adapter.call::<_, _, ()>("RemoveDevice", &(&path,)),
            )
            .await;
            if let Err(error) = removed {
                tracing::warn!(%address, error = %describe(&error), "could not remove the device");
            }
        }
    }
}

/// The boolean property `key`, false when it is missing.
fn flag(properties: &Properties, key: &str) -> bool {
    properties
        .get(key)
        .and_then(|value| bool::try_from(value).ok())
        .unwrap_or(false)
}

/// Whether a device paired in any way: bonded, or for its current
/// connection only.
fn any_pairing(properties: &Properties) -> bool {
    flag(properties, "Paired") || flag(properties, "Bonded")
}

/// Whether a device is paired for good: bonded, where BlueZ reports that
/// apart from `Paired` (5.70 and later), which is also true for a pairing
/// that keeps no keys.
fn bonded(properties: &Properties) -> bool {
    flag(properties, "Paired")
        && properties
            .get("Bonded")
            .is_none_or(|value| bool::try_from(value).unwrap_or(false))
}

fn adapter_info(
    properties: &mut std::collections::HashMap<String, OwnedValue>,
) -> Option<BluetoothAdapter> {
    let address = address(properties)?;
    let alias: Option<String> = take(properties, "Alias");
    let name = alias
        .filter(|alias| !alias.is_empty())
        .or_else(|| take(properties, "Name"))
        .unwrap_or_default();
    Some(BluetoothAdapter {
        address,
        name,
        powered: take(properties, "Powered").unwrap_or(false),
    })
}

fn address(properties: &std::collections::HashMap<String, OwnedValue>) -> Option<BluetoothAddress> {
    let value = properties.get("Address")?;
    String::try_from(value.clone()).ok()?.parse().ok()
}

fn belongs_to(
    properties: &std::collections::HashMap<String, OwnedValue>,
    adapter: &OwnedObjectPath,
) -> bool {
    properties
        .get("Adapter")
        .and_then(|value| OwnedObjectPath::try_from(value.clone()).ok())
        .is_some_and(|path| &path == adapter)
}

fn device_info(
    mut properties: std::collections::HashMap<String, OwnedValue>,
) -> Option<BluetoothDevice> {
    let address = address(&properties)?;
    let paired = bonded(&properties);
    let alias: Option<String> = take(&mut properties, "Alias");
    let name = alias
        .filter(|alias| !alias.is_empty())
        .or_else(|| take(&mut properties, "Name"))
        .unwrap_or_default();
    let class: Option<u32> = take(&mut properties, "Class");
    let icon: Option<String> = take(&mut properties, "Icon");
    let uuids: Vec<String> = take(&mut properties, "UUIDs").unwrap_or_default();
    Some(BluetoothDevice {
        address,
        name,
        paired,
        trusted: take(&mut properties, "Trusted").unwrap_or(false),
        connected: take(&mut properties, "Connected").unwrap_or(false),
        phone: class.is_some_and(|class| (class >> 8) & 0x1f == MAJOR_CLASS_PHONE)
            || icon.as_deref() == Some("phone"),
        hands_free_gateway: uuids
            .iter()
            .any(|uuid| uuid.eq_ignore_ascii_case(HANDS_FREE_GATEWAY_UUID)),
    })
}

/// Sets a property, logging a failure: used where nothing better can be
/// done about one.
async fn set<'a>(
    connection: &Connection,
    path: ObjectPath<'_>,
    interface: &str,
    name: &str,
    value: impl Into<Value<'a>>,
) {
    if let Err(error) = try_set(connection, path, interface, name, value).await {
        tracing::debug!(name, error = %describe(&error), "could not set an adapter property");
    }
}

async fn try_set<'a>(
    connection: &Connection,
    path: ObjectPath<'_>,
    interface: &str,
    name: &str,
    value: impl Into<Value<'a>>,
) -> zbus::Result<()> {
    let properties = Proxy::new(
        connection,
        BLUEZ,
        path.to_owned(),
        "org.freedesktop.DBus.Properties",
    )
    .await?;
    let value = value.into();
    bounded(
        CALL_TIMEOUT,
        properties.call::<_, _, ()>("Set", &(interface, name, &value)),
    )
    .await
}

async fn agent_manager(connection: &Connection) -> zbus::Result<Proxy<'static>> {
    Proxy::new(connection, BLUEZ, "/org/bluez", AGENT_MANAGER).await
}

async fn drop_agent(connection: &Connection) {
    let unregistered = async {
        let manager = agent_manager(connection).await?;
        let path = ObjectPath::try_from(AGENT_PATH)?;
        bounded(
            CALL_TIMEOUT,
            manager.call::<_, _, ()>("UnregisterAgent", &(path,)),
        )
        .await
    }
    .await;
    // BlueZ may have dropped it already, as when bluetoothd restarted.
    if let Err(error) = unregistered {
        tracing::debug!(error = %describe(&error), "unregistering the pairing agent failed");
    }
    let _ = connection
        .object_server()
        .remove::<Agent, _>(AGENT_PATH)
        .await;
}

/// Whether a BlueZ signal may change what [`Bluetooth::adapter`] or
/// [`Bluetooth::devices`] report.
fn relevant(message: &Message) -> bool {
    const KEYS: [&str; 13] = [
        "Address",
        "Alias",
        "Name",
        "Paired",
        "Bonded",
        "Trusted",
        "Connected",
        "Class",
        "Icon",
        "UUIDs",
        "Powered",
        "Discoverable",
        "Pairable",
    ];
    let header = message.header();
    match header.member().map(|member| member.as_str()) {
        Some("InterfacesAdded" | "InterfacesRemoved") => true,
        Some("PropertiesChanged") => {
            let Ok((interface, changed, _)) = message.body().deserialize::<(
                String,
                std::collections::HashMap<String, OwnedValue>,
                Vec<String>,
            )>() else {
                return false;
            };
            (interface == DEVICE || interface == ADAPTER)
                && changed.keys().any(|key| KEYS.contains(&key.as_str()))
        }
        _ => false,
    }
}

impl Bluetooth for BluezBluetooth {
    async fn adapter(&self) -> Result<Option<BluetoothAdapter>, PlatformError> {
        let connection = self.connect().await?;
        Ok(self
            .adapter_objects(&connection)
            .await?
            .and_then(|(_, mut interfaces, _)| adapter_info(interfaces.get_mut(ADAPTER)?)))
    }

    async fn devices(&self) -> Result<Vec<BluetoothDevice>, PlatformError> {
        let connection = self.connect().await?;
        let Some((adapter, _, objects)) = self.adapter_objects(&connection).await? else {
            return Ok(Vec::new());
        };
        let opened = self.opened.lock().await;
        let mut devices: Vec<BluetoothDevice> = objects
            .into_values()
            .filter_map(|mut interfaces| interfaces.remove(DEVICE))
            .filter(|properties| belongs_to(properties, &adapter))
            .filter_map(|properties| {
                let stranger = opened.as_ref().is_some_and(|opened| {
                    opened.stranger(&properties, |a| self.agent.is_confirmed(a))
                });
                let mut device = device_info(properties)?;
                // Not a pairing the desktop accepts: it is removed when
                // pairing closes.
                device.paired &= !stranger;
                Some(device)
            })
            .collect();
        drop(opened);
        devices.sort_by_key(|device| device.address.0);
        Ok(devices)
    }

    async fn changed(&self) -> Result<(), PlatformError> {
        let connection = self.connect().await?;
        wait_for_change(&self.signals, &connection, BLUEZ, SERVICE, relevant).await
    }

    async fn open_pairing(&self, window: Duration) -> Result<BluetoothAdapter, PlatformError> {
        let connection = self.connect().await?;
        let mut opened = self.opened.lock().await;
        if let Some(earlier) = opened.take() {
            self.undo(&connection, earlier).await;
        }
        let (path, mut interfaces, objects) = self
            .adapter_objects(&connection)
            .await?
            .ok_or_else(|| unavailable(SERVICE, "no Bluetooth adapter"))?;
        let mut properties = interfaces.remove(ADAPTER).unwrap_or_default();
        let discoverable = take(&mut properties, "Discoverable").unwrap_or(false);
        let discoverable_timeout = take(&mut properties, "DiscoverableTimeout").unwrap_or(180);
        let pairable = take(&mut properties, "Pairable").unwrap_or(false);
        let pairable_timeout: u32 = take(&mut properties, "PairableTimeout").unwrap_or(0);
        let info = adapter_info(&mut properties)
            .ok_or_else(|| unavailable(SERVICE, "the adapter reports no address"))?;
        if !info.powered {
            return Err(unavailable(SERVICE, "Bluetooth is turned off"));
        }

        self.agent.open();
        let served = connection
            .object_server()
            .at(
                AGENT_PATH,
                Agent {
                    shared: self.agent.clone(),
                },
            )
            .await
            .map_err(|e| unavailable(SERVICE, describe(&e)))?;
        if !served {
            tracing::debug!("the pairing agent was still exported");
        }
        let devices = objects
            .values()
            .filter_map(|interfaces| interfaces.get(DEVICE))
            .filter(|properties| belongs_to(properties, &path));
        let (mut paired, mut bonded_now) = (HashSet::new(), HashSet::new());
        for properties in devices {
            if let Some(address) = address(properties) {
                if any_pairing(properties) {
                    paired.insert(address);
                }
                if bonded(properties) {
                    bonded_now.insert(address);
                }
            }
        }
        // A pairable adapter with a timeout may have a timer running that
        // would end pairing early, so the window's timeout replaces it.
        let limit_pairable = !pairable || pairable_timeout != 0;
        let earlier = Opened {
            adapter: path.clone(),
            discoverable,
            discoverable_timeout,
            pairable,
            pairable_timeout: limit_pairable.then_some(pairable_timeout),
            paired,
            bonded: bonded_now,
        };
        let seconds = u32::try_from(window.as_secs()).unwrap_or(u32::MAX).max(1);
        let started = async {
            let manager = agent_manager(&connection).await?;
            let agent = ObjectPath::try_from(AGENT_PATH)?;
            bounded(
                CALL_TIMEOUT,
                manager.call::<_, _, ()>("RegisterAgent", &(&agent, CAPABILITY)),
            )
            .await?;
            bounded(
                CALL_TIMEOUT,
                manager.call::<_, _, ()>("RequestDefaultAgent", &(&agent,)),
            )
            .await?;
            // BlueZ ends discoverability and pairing by itself when the
            // time is up, even if this app is gone by then.
            try_set(
                &connection,
                path.as_ref(),
                ADAPTER,
                "DiscoverableTimeout",
                seconds,
            )
            .await?;
            if limit_pairable {
                try_set(
                    &connection,
                    path.as_ref(),
                    ADAPTER,
                    "PairableTimeout",
                    seconds,
                )
                .await?;
            }
            try_set(&connection, path.as_ref(), ADAPTER, "Pairable", true).await?;
            try_set(&connection, path.as_ref(), ADAPTER, "Discoverable", true).await
        }
        .await;
        if let Err(error) = started {
            self.undo(&connection, earlier).await;
            return Err(unavailable(
                SERVICE,
                format!("could not open pairing: {}", describe(&error)),
            ));
        }
        tracing::info!(seconds, "Bluetooth pairing is open");
        *opened = Some(earlier);
        Ok(info)
    }

    async fn close_pairing(&self) -> Result<(), PlatformError> {
        let Some(earlier) = self.opened.lock().await.take() else {
            self.agent.close();
            return Ok(());
        };
        let connection = self.connect().await?;
        self.undo(&connection, earlier).await;
        tracing::info!("Bluetooth pairing is closed");
        Ok(())
    }

    async fn next_pairing_event(&self) -> PairingEvent {
        self.agent.next_event().await
    }

    fn answer_pairing(&self, id: u64, accept: bool) {
        self.agent.answer(id, accept);
    }

    async fn connect_hands_free(&self, device: BluetoothAddress) -> Result<(), PlatformError> {
        let connection = self.connect().await?;
        let (path, properties) = self.device_object(&connection, device).await?;
        if let Some(opened) = &*self.opened.lock().await
            && opened.stranger(&properties, |a| self.agent.is_confirmed(a))
        {
            // Trusted, it could connect any service unasked.
            return Err(unavailable(
                SERVICE,
                "the device paired without a code confirmed on this computer",
            ));
        }
        try_set(&connection, path.as_ref(), DEVICE, "Trusted", true)
            .await
            .map_err(|e| {
                unavailable(
                    SERVICE,
                    format!("could not trust the phone: {}", describe(&e)),
                )
            })?;
        let proxy = Proxy::new(&connection, BLUEZ, path, DEVICE)
            .await
            .map_err(|e| unavailable(SERVICE, describe(&e)))?;
        let connected = bounded(
            CONNECT_TIMEOUT,
            proxy.call::<_, _, ()>("ConnectProfile", &(HANDS_FREE_GATEWAY_UUID,)),
        )
        .await;
        match connected {
            Ok(()) => Ok(()),
            Err(error)
                if error_name(&error).as_deref() == Some("org.bluez.Error.AlreadyConnected") =>
            {
                Ok(())
            }
            Err(error) => Err(unavailable(
                SERVICE,
                format!("could not connect the phone's calls: {}", describe(&error)),
            )),
        }
    }

    async fn disconnect_hands_free(&self, device: BluetoothAddress) -> Result<(), PlatformError> {
        let connection = self.connect().await?;
        let path = self.device_path(&connection, device).await?;
        let proxy = Proxy::new(&connection, BLUEZ, path, DEVICE)
            .await
            .map_err(|e| unavailable(SERVICE, describe(&e)))?;
        let disconnected = bounded(
            CALL_TIMEOUT,
            proxy.call::<_, _, ()>("DisconnectProfile", &(HANDS_FREE_GATEWAY_UUID,)),
        )
        .await;
        match disconnected {
            Ok(()) => Ok(()),
            Err(error) if error_name(&error).as_deref() == Some("org.bluez.Error.NotConnected") => {
                Ok(())
            }
            Err(error) => Err(unavailable(
                SERVICE,
                format!(
                    "could not disconnect the phone's calls: {}",
                    describe(&error)
                ),
            )),
        }
    }
}

#[cfg(test)]
mod tests;
