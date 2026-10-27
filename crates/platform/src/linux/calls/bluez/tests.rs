//! [`BluezBluetooth`] against a mock BlueZ on a peer-to-peer link.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Proxy};

use super::super::testing::link;
use super::*;

const HCI0: &str = "/org/bluez/hci0";
const PHONE: &str = "0C:1A:2B:3C:4D:5E";
const HEADSET: &str = "00:11:22:33:44:55";

#[derive(Debug, Clone)]
struct MockDevice {
    address: &'static str,
    alias: &'static str,
    adapter: &'static str,
    paired: bool,
    /// `None` for a BlueZ that does not report `Bonded`.
    bonded: Option<bool>,
    trusted: bool,
    class: Option<u32>,
    icon: Option<&'static str>,
    uuids: Vec<&'static str>,
}

impl MockDevice {
    fn path(&self) -> String {
        format!("{}/dev_{}", self.adapter, self.address.replace(':', "_"))
    }
}

#[derive(Debug)]
struct State {
    adapter: bool,
    powered: bool,
    discoverable: bool,
    discoverable_timeout: u32,
    pairable: bool,
    pairable_timeout: u32,
    devices: Vec<MockDevice>,
    /// What the agent manager and the devices were asked, in order.
    calls: Vec<String>,
    /// The error name `ConnectProfile` answers with.
    connect_error: Option<&'static str>,
}

type Shared = Arc<StdMutex<State>>;

fn value(v: impl Into<Value<'static>>) -> OwnedValue {
    v.into().try_into().unwrap()
}

fn phone() -> MockDevice {
    MockDevice {
        address: PHONE,
        alias: "Pixel 8",
        adapter: HCI0,
        paired: true,
        bonded: Some(true),
        trusted: false,
        class: Some(0x5a020c),
        icon: None,
        uuids: vec!["0000111F-0000-1000-8000-00805F9B34FB"],
    }
}

fn headset() -> MockDevice {
    MockDevice {
        address: HEADSET,
        alias: "Headphones",
        adapter: HCI0,
        paired: true,
        bonded: None,
        trusted: true,
        class: Some(0x240404),
        icon: Some("audio-headset"),
        uuids: vec!["0000111e-0000-1000-8000-00805f9b34fb"],
    }
}

struct MockObjectManager(Shared);

#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl MockObjectManager {
    fn get_managed_objects(&self) -> HashMap<OwnedObjectPath, Interfaces> {
        let state = self.0.lock().unwrap();
        let mut objects = HashMap::new();
        if state.adapter {
            let mut adapter = HashMap::new();
            adapter.insert("Address".to_owned(), value("A0:B1:C2:D3:E4:F5"));
            adapter.insert("Alias".to_owned(), value("workstation"));
            adapter.insert("Powered".to_owned(), value(state.powered));
            adapter.insert("Discoverable".to_owned(), value(state.discoverable));
            adapter.insert(
                "DiscoverableTimeout".to_owned(),
                value(state.discoverable_timeout),
            );
            adapter.insert("Pairable".to_owned(), value(state.pairable));
            adapter.insert("PairableTimeout".to_owned(), value(state.pairable_timeout));
            let mut interfaces = Interfaces::new();
            interfaces.insert(ADAPTER.to_owned(), adapter);
            objects.insert(OwnedObjectPath::try_from(HCI0).unwrap(), interfaces);
        }
        for device in &state.devices {
            let mut properties = HashMap::new();
            properties.insert("Address".to_owned(), value(device.address));
            properties.insert("Alias".to_owned(), value(device.alias));
            properties.insert(
                "Adapter".to_owned(),
                value(ObjectPath::try_from(device.adapter).unwrap()),
            );
            properties.insert("Paired".to_owned(), value(device.paired));
            if let Some(bonded) = device.bonded {
                properties.insert("Bonded".to_owned(), value(bonded));
            }
            properties.insert("Trusted".to_owned(), value(device.trusted));
            properties.insert("Connected".to_owned(), value(false));
            if let Some(class) = device.class {
                properties.insert("Class".to_owned(), value(class));
            }
            if let Some(icon) = device.icon {
                properties.insert("Icon".to_owned(), value(icon));
            }
            let uuids: Vec<String> = device.uuids.iter().map(|u| (*u).to_owned()).collect();
            properties.insert("UUIDs".to_owned(), value(uuids));
            let mut interfaces = Interfaces::new();
            interfaces.insert(DEVICE.to_owned(), properties);
            objects.insert(
                OwnedObjectPath::try_from(device.path()).unwrap(),
                interfaces,
            );
        }
        objects
    }
}

struct MockAgentManager(Shared);

#[zbus::interface(name = "org.bluez.AgentManager1")]
impl MockAgentManager {
    fn register_agent(&self, agent: OwnedObjectPath, capability: String) {
        let mut state = self.0.lock().unwrap();
        state
            .calls
            .push(format!("register {} {capability}", agent.as_str()));
    }

    fn request_default_agent(&self, agent: OwnedObjectPath) {
        let mut state = self.0.lock().unwrap();
        state.calls.push(format!("default {}", agent.as_str()));
    }

    fn unregister_agent(&self, agent: OwnedObjectPath) {
        let mut state = self.0.lock().unwrap();
        state.calls.push(format!("unregister {}", agent.as_str()));
    }
}

struct MockAdapter(Shared);

#[zbus::interface(name = "org.bluez.Adapter1")]
impl MockAdapter {
    #[zbus(property)]
    fn discoverable(&self) -> bool {
        self.0.lock().unwrap().discoverable
    }

    #[zbus(property)]
    fn set_discoverable(&mut self, on: bool) {
        self.0.lock().unwrap().discoverable = on;
    }

    #[zbus(property)]
    fn discoverable_timeout(&self) -> u32 {
        self.0.lock().unwrap().discoverable_timeout
    }

    #[zbus(property)]
    fn set_discoverable_timeout(&mut self, seconds: u32) {
        self.0.lock().unwrap().discoverable_timeout = seconds;
    }

    #[zbus(property)]
    fn pairable(&self) -> bool {
        self.0.lock().unwrap().pairable
    }

    #[zbus(property)]
    fn set_pairable(&mut self, on: bool) {
        self.0.lock().unwrap().pairable = on;
    }

    #[zbus(property)]
    fn pairable_timeout(&self) -> u32 {
        self.0.lock().unwrap().pairable_timeout
    }

    #[zbus(property)]
    fn set_pairable_timeout(&mut self, seconds: u32) {
        self.0.lock().unwrap().pairable_timeout = seconds;
    }

    fn remove_device(&self, device: OwnedObjectPath) {
        let mut state = self.0.lock().unwrap();
        state.devices.retain(|d| d.path() != device.as_str());
        let address = device.as_str().rsplit("dev_").next().unwrap_or_default();
        state
            .calls
            .push(format!("remove {}", address.replace('_', ":")));
    }
}

struct MockDeviceObject {
    state: Shared,
    address: &'static str,
}

#[zbus::interface(name = "org.bluez.Device1")]
impl MockDeviceObject {
    #[zbus(property)]
    fn alias(&self) -> String {
        let state = self.state.lock().unwrap();
        let device = state.devices.iter().find(|d| d.address == self.address);
        device.map(|d| d.alias.to_owned()).unwrap_or_default()
    }

    #[zbus(property)]
    fn trusted(&self) -> bool {
        let state = self.state.lock().unwrap();
        state
            .devices
            .iter()
            .any(|d| d.address == self.address && d.trusted)
    }

    #[zbus(property)]
    fn set_trusted(&mut self, trusted: bool) {
        let mut state = self.state.lock().unwrap();
        for device in &mut state.devices {
            if device.address == self.address {
                device.trusted = trusted;
            }
        }
        state
            .calls
            .push(format!("trust {} {trusted}", self.address));
    }

    fn connect_profile(&self, uuid: String) -> zbus::fdo::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(format!("connect {} {uuid}", self.address));
        match state.connect_error {
            Some(name) => Err(zbus::fdo::Error::Failed(name.to_owned())),
            None => Ok(()),
        }
    }

    fn disconnect_profile(&self, uuid: String) {
        let mut state = self.state.lock().unwrap();
        state
            .calls
            .push(format!("disconnect {} {uuid}", self.address));
    }
}

struct Mock {
    client: Connection,
    server: Connection,
    state: Shared,
}

impl Mock {
    fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }

    fn bluetooth(&self) -> BluezBluetooth {
        BluezBluetooth::with_connection(self.client.clone())
    }

    /// Calls the desktop's agent the way BlueZ does.
    async fn agent(&self) -> Proxy<'static> {
        Proxy::new(&self.server, BLUEZ, AGENT_PATH, "org.bluez.Agent1")
            .await
            .unwrap()
    }
}

async fn mock_with(state: State) -> Mock {
    let devices = state.devices.clone();
    let state = Arc::new(StdMutex::new(state));
    let (server, client) = link(|builder| {
        let mut builder = builder
            .serve_at("/", MockObjectManager(state.clone()))?
            .serve_at("/org/bluez", MockAgentManager(state.clone()))?
            .serve_at(HCI0, MockAdapter(state.clone()))?;
        for device in &devices {
            builder = builder.serve_at(
                device.path(),
                MockDeviceObject {
                    state: state.clone(),
                    address: device.address,
                },
            )?;
        }
        Ok(builder)
    })
    .await;
    Mock {
        client,
        server,
        state,
    }
}

fn state(devices: Vec<MockDevice>) -> State {
    State {
        adapter: true,
        powered: true,
        discoverable: false,
        discoverable_timeout: 180,
        pairable: true,
        pairable_timeout: 0,
        devices,
        calls: Vec::new(),
        connect_error: None,
    }
}

async fn mock() -> Mock {
    let mut other_adapter = headset();
    other_adapter.address = "66:77:88:99:AA:BB";
    other_adapter.adapter = "/org/bluez/hci1";
    let mut by_icon = headset();
    by_icon.address = "12:34:56:78:9A:BC";
    by_icon.alias = "Old phone";
    by_icon.class = None;
    by_icon.icon = Some("phone");
    by_icon.uuids = Vec::new();
    mock_with(state(vec![phone(), headset(), other_adapter, by_icon])).await
}

#[tokio::test]
async fn reports_the_adapter_and_its_devices() {
    let m = mock().await;
    let bluetooth = m.bluetooth();
    let adapter = bluetooth.adapter().await.unwrap().unwrap();
    assert_eq!(adapter.address.to_string(), "A0:B1:C2:D3:E4:F5");
    assert_eq!(adapter.name, "workstation");
    assert!(adapter.powered);

    let devices = bluetooth.devices().await.unwrap();
    let summary: Vec<(String, &str, bool, bool)> = devices
        .iter()
        .map(|d| {
            (
                d.address.to_string(),
                d.name.as_str(),
                d.phone,
                d.hands_free_gateway,
            )
        })
        .collect();
    // The device of another adapter is left out.
    assert_eq!(
        summary,
        [
            (HEADSET.to_owned(), "Headphones", false, false),
            (PHONE.to_owned(), "Pixel 8", true, true),
            ("12:34:56:78:9A:BC".to_owned(), "Old phone", true, false),
        ]
    );
    assert!(devices.iter().all(|d| d.paired));
}

#[tokio::test]
async fn no_adapter_means_none_and_no_devices() {
    let mut without = state(vec![phone()]);
    without.adapter = false;
    let m = mock_with(without).await;
    let bluetooth = m.bluetooth();
    assert_eq!(bluetooth.adapter().await.unwrap(), None);
    assert!(bluetooth.devices().await.unwrap().is_empty());
    assert!(matches!(
        bluetooth.open_pairing(Duration::from_secs(120)).await,
        Err(PlatformError::Unavailable { .. })
    ));
}

#[tokio::test]
async fn pairing_opens_for_its_window_and_restores_the_adapter() {
    let m = mock().await;
    let bluetooth = m.bluetooth();
    let adapter = bluetooth
        .open_pairing(Duration::from_secs(120))
        .await
        .unwrap();
    assert_eq!(adapter.name, "workstation");
    {
        let state = m.state.lock().unwrap();
        assert!(state.discoverable);
        assert!(state.pairable);
        assert_eq!(state.discoverable_timeout, 120);
    }
    assert_eq!(
        m.calls(),
        [
            "register /org/link2linux/agent DisplayYesNo",
            "default /org/link2linux/agent",
        ]
    );

    bluetooth.close_pairing().await.unwrap();
    {
        let state = m.state.lock().unwrap();
        assert!(!state.discoverable);
        assert!(state.pairable);
        assert_eq!(state.discoverable_timeout, 180);
        // Pairable already, with no timer: left alone.
        assert_eq!(state.pairable_timeout, 0);
    }
    assert_eq!(
        m.calls().last().unwrap(),
        "unregister /org/link2linux/agent"
    );
    // Closing again does nothing.
    bluetooth.close_pairing().await.unwrap();
    assert_eq!(m.calls().len(), 3);
}

#[tokio::test]
async fn an_adapter_that_was_discoverable_stays_so() {
    // Discoverable for good, as while the desktop's Bluetooth settings are
    // open, and not pairable.
    let mut before = state(Vec::new());
    before.discoverable = true;
    before.discoverable_timeout = 0;
    before.pairable = false;
    let m = mock_with(before).await;
    let bluetooth = m.bluetooth();
    bluetooth
        .open_pairing(Duration::from_secs(120))
        .await
        .unwrap();
    // Opening again restarts the window.
    bluetooth
        .open_pairing(Duration::from_secs(120))
        .await
        .unwrap();
    {
        let mut state = m.state.lock().unwrap();
        // BlueZ ends both by itself when the window is up, even if the app
        // is gone by then.
        assert_eq!(state.discoverable_timeout, 120);
        assert_eq!(state.pairable_timeout, 120);
        assert!(state.pairable);
        // The window ran out on BlueZ's timer first.
        state.discoverable = false;
    }
    bluetooth.close_pairing().await.unwrap();
    let state = m.state.lock().unwrap();
    assert!(state.discoverable);
    assert_eq!(state.discoverable_timeout, 0);
    assert!(!state.pairable);
    assert_eq!(state.pairable_timeout, 0);
}

#[tokio::test]
async fn a_powered_off_adapter_does_not_open_pairing() {
    let mut off = state(Vec::new());
    off.powered = false;
    let m = mock_with(off).await;
    let error = m
        .bluetooth()
        .open_pairing(Duration::from_secs(120))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("turned off"), "{error}");
    assert!(m.calls().is_empty());
    assert!(!m.state.lock().unwrap().discoverable);
}

fn error_of<T: std::fmt::Debug>(result: zbus::Result<T>) -> String {
    error_name(&result.unwrap_err()).unwrap()
}

#[tokio::test]
async fn the_user_confirms_the_code_before_a_pairing_is_accepted() {
    let m = mock().await;
    let bluetooth = Arc::new(m.bluetooth());
    bluetooth
        .open_pairing(Duration::from_secs(120))
        .await
        .unwrap();
    let agent = m.agent().await;
    let phone_path = ObjectPath::try_from(phone().path()).unwrap();

    let asked = tokio::spawn({
        let agent = agent.clone();
        let path = phone_path.clone();
        async move {
            agent
                .call::<_, _, ()>("RequestConfirmation", &(path, 123_456u32))
                .await
        }
    });
    let PairingEvent::Confirm(prompt) = bluetooth.next_pairing_event().await else {
        panic!("expected a prompt");
    };
    assert_eq!(prompt.device.to_string(), PHONE);
    assert_eq!(prompt.device_name, "Pixel 8");
    assert_eq!(prompt.code, 123_456);
    bluetooth.answer_pairing(prompt.id, true);
    asked.await.unwrap().unwrap();

    // The confirmed phone may connect its services; another device may not.
    agent
        .call::<_, _, ()>("AuthorizeService", &(&phone_path, HANDS_FREE_GATEWAY_UUID))
        .await
        .unwrap();
    let headset_path = ObjectPath::try_from(headset().path()).unwrap();
    assert_eq!(
        error_of(
            agent
                .call::<_, _, ()>(
                    "AuthorizeService",
                    &(&headset_path, HANDS_FREE_GATEWAY_UUID)
                )
                .await
        ),
        "org.bluez.Error.Rejected"
    );

    // Once pairing closed, even the confirmed phone is not let in by the
    // agent, should BlueZ still ask it.
    bluetooth.agent.close();
    assert_eq!(
        error_of(
            agent
                .call::<_, _, ()>("AuthorizeService", &(&phone_path, HANDS_FREE_GATEWAY_UUID))
                .await
        ),
        "org.bluez.Error.Rejected"
    );
}

#[tokio::test]
async fn the_agent_refuses_everything_while_pairing_is_closed() {
    let m = mock().await;
    let bluetooth = m.bluetooth();
    bluetooth
        .open_pairing(Duration::from_secs(120))
        .await
        .unwrap();
    // Pairing closed, while BlueZ still calls the agent.
    bluetooth.agent.close();
    let agent = m.agent().await;
    let path = ObjectPath::try_from(phone().path()).unwrap();
    for refused in [
        agent
            .call::<_, _, ()>("RequestConfirmation", &(&path, 1u32))
            .await,
        agent
            .call::<_, _, ()>("AuthorizeService", &(&path, HANDS_FREE_GATEWAY_UUID))
            .await,
        agent
            .call::<_, _, ()>("DisplayPasskey", &(&path, 1u32, 0u16))
            .await,
    ] {
        assert_eq!(error_of(refused), "org.bluez.Error.Rejected");
    }
}

/// A phone-like device that pairs while pairing is open, by itself.
fn stranger() -> MockDevice {
    MockDevice {
        address: "0A:0B:0C:0D:0E:0F",
        ..phone()
    }
}

#[tokio::test]
async fn devices_that_pair_without_a_confirmed_code_are_ignored_and_removed() {
    const STRANGER: &str = "0A:0B:0C:0D:0E:0F";
    const SETTINGS: &str = "0A:0B:0C:0D:0E:10";
    let m = mock().await;
    let bluetooth = m.bluetooth();
    bluetooth
        .open_pairing(Duration::from_secs(120))
        .await
        .unwrap();
    // One device pairs with no prompt ("just works"); the user pairs
    // another in the desktop's Bluetooth settings, which trust it.
    let settings = MockDevice {
        address: SETTINGS,
        trusted: true,
        ..phone()
    };
    m.state
        .lock()
        .unwrap()
        .devices
        .extend([stranger(), settings]);

    let paired = |devices: &[BluetoothDevice], address: &str| {
        devices
            .iter()
            .find(|d| d.address.to_string() == address)
            .map(|d| d.paired)
    };
    let devices = bluetooth.devices().await.unwrap();
    assert_eq!(paired(&devices, STRANGER), Some(false));
    assert_eq!(paired(&devices, SETTINGS), Some(true));
    // Paired before pairing opened, though not trusted: still paired.
    assert_eq!(paired(&devices, PHONE), Some(true));
    assert!(
        bluetooth
            .connect_hands_free(STRANGER.parse().unwrap())
            .await
            .is_err()
    );

    bluetooth.close_pairing().await.unwrap();
    let changed: Vec<String> = m
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("remove") || c.starts_with("trust"))
        .collect();
    assert_eq!(changed, [format!("remove {STRANGER}")]);
    let devices = bluetooth.devices().await.unwrap();
    assert_eq!(paired(&devices, STRANGER), None);
    assert_eq!(paired(&devices, SETTINGS), Some(true));
}

#[tokio::test]
async fn the_device_whose_code_the_user_confirmed_is_trusted_and_kept() {
    let mut not_yet = phone();
    not_yet.paired = false;
    not_yet.bonded = Some(false);
    let m = mock_with(state(vec![not_yet])).await;
    let bluetooth = Arc::new(m.bluetooth());
    bluetooth
        .open_pairing(Duration::from_secs(120))
        .await
        .unwrap();
    let agent = m.agent().await;
    let asked = tokio::spawn({
        let agent = agent.clone();
        let path = ObjectPath::try_from(phone().path()).unwrap();
        async move {
            agent
                .call::<_, _, ()>("RequestConfirmation", &(path, 123_456u32))
                .await
        }
    });
    let PairingEvent::Confirm(prompt) = bluetooth.next_pairing_event().await else {
        panic!("expected a prompt");
    };
    bluetooth.answer_pairing(prompt.id, true);
    asked.await.unwrap().unwrap();
    // BlueZ finishes pairing; the phone may connect just after.
    {
        let mut state = m.state.lock().unwrap();
        state.devices[0].paired = true;
        state.devices[0].bonded = Some(true);
    }
    let devices = bluetooth.devices().await.unwrap();
    assert!(devices[0].paired);
    let address: BluetoothAddress = PHONE.parse().unwrap();
    bluetooth.connect_hands_free(address).await.unwrap();
    bluetooth.close_pairing().await.unwrap();
    let calls = m.calls();
    assert!(calls.contains(&format!("trust {PHONE} true")), "{calls:?}");
    assert!(
        calls.contains(&format!("connect {PHONE} {HANDS_FREE_GATEWAY_UUID}")),
        "{calls:?}"
    );
    assert!(!calls.iter().any(|c| c.starts_with("remove")), "{calls:?}");
}

#[tokio::test]
async fn a_pairing_that_keeps_no_keys_does_not_count() {
    let mut passing = phone();
    passing.bonded = Some(false);
    // The headset's BlueZ reports no `Bonded`: `Paired` counts alone.
    let m = mock_with(state(vec![passing, headset()])).await;
    let devices = m.bluetooth().devices().await.unwrap();
    let paired: Vec<(String, bool)> = devices
        .iter()
        .map(|d| (d.address.to_string(), d.paired))
        .collect();
    assert_eq!(
        paired,
        [(HEADSET.to_owned(), true), (PHONE.to_owned(), false)]
    );
}

#[tokio::test]
async fn pairings_without_a_confirmed_code_are_refused() {
    let m = mock().await;
    let bluetooth = Arc::new(m.bluetooth());
    bluetooth
        .open_pairing(Duration::from_secs(120))
        .await
        .unwrap();
    let agent = m.agent().await;
    let path = ObjectPath::try_from(phone().path()).unwrap();

    let asked = tokio::spawn({
        let agent = agent.clone();
        let path = path.clone();
        async move {
            agent
                .call::<_, _, ()>("RequestConfirmation", &(path, 1u32))
                .await
        }
    });
    let PairingEvent::Confirm(prompt) = bluetooth.next_pairing_event().await else {
        panic!("expected a prompt");
    };
    bluetooth.answer_pairing(prompt.id, false);
    assert_eq!(error_of(asked.await.unwrap()), "org.bluez.Error.Rejected");

    for refused in [
        agent
            .call::<_, _, ()>("RequestAuthorization", &(&path,))
            .await,
        agent
            .call::<_, _, ()>("DisplayPinCode", &(&path, "0000"))
            .await,
        agent
            .call::<_, _, ()>("DisplayPasskey", &(&path, 123_456u32, 0u16))
            .await,
    ] {
        assert_eq!(error_of(refused), "org.bluez.Error.Rejected");
    }
    assert_eq!(
        error_of(
            agent
                .call::<_, _, String>("RequestPinCode", &(&path,))
                .await
        ),
        "org.bluez.Error.Rejected"
    );
    assert_eq!(
        error_of(agent.call::<_, _, u32>("RequestPasskey", &(&path,)).await),
        "org.bluez.Error.Rejected"
    );
}

#[tokio::test]
async fn a_prompt_open_when_pairing_closes_is_refused() {
    let m = mock().await;
    let bluetooth = Arc::new(m.bluetooth());
    bluetooth
        .open_pairing(Duration::from_secs(120))
        .await
        .unwrap();
    let agent = m.agent().await;
    let path = ObjectPath::try_from(phone().path()).unwrap();
    let asked = tokio::spawn({
        let agent = agent.clone();
        async move {
            agent
                .call::<_, _, ()>("RequestConfirmation", &(path, 1u32))
                .await
        }
    });
    let PairingEvent::Confirm(prompt) = bluetooth.next_pairing_event().await else {
        panic!("expected a prompt");
    };
    bluetooth.close_pairing().await.unwrap();
    assert_eq!(
        bluetooth.next_pairing_event().await,
        PairingEvent::Cancelled { id: prompt.id }
    );
    // A late "yes" changes nothing.
    bluetooth.answer_pairing(prompt.id, true);
    assert!(asked.await.unwrap().is_err());
}

#[tokio::test]
async fn bluez_cancelling_ends_the_prompt() {
    let m = mock().await;
    let bluetooth = Arc::new(m.bluetooth());
    bluetooth
        .open_pairing(Duration::from_secs(120))
        .await
        .unwrap();
    let agent = m.agent().await;
    let path = ObjectPath::try_from(phone().path()).unwrap();
    let asked = tokio::spawn({
        let agent = agent.clone();
        async move {
            agent
                .call::<_, _, ()>("RequestConfirmation", &(path, 1u32))
                .await
        }
    });
    let PairingEvent::Confirm(prompt) = bluetooth.next_pairing_event().await else {
        panic!("expected a prompt");
    };
    agent.call::<_, _, ()>("Cancel", &()).await.unwrap();
    assert_eq!(
        bluetooth.next_pairing_event().await,
        PairingEvent::Cancelled { id: prompt.id }
    );
    assert!(asked.await.unwrap().is_err());
}

#[tokio::test]
async fn connects_and_disconnects_only_the_hands_free_profile() {
    let m = mock().await;
    let bluetooth = m.bluetooth();
    let address: BluetoothAddress = PHONE.parse().unwrap();
    bluetooth.connect_hands_free(address).await.unwrap();
    bluetooth.disconnect_hands_free(address).await.unwrap();
    assert_eq!(
        m.calls(),
        [
            format!("trust {PHONE} true"),
            format!("connect {PHONE} {HANDS_FREE_GATEWAY_UUID}"),
            format!("disconnect {PHONE} {HANDS_FREE_GATEWAY_UUID}"),
        ]
    );
    let unknown: BluetoothAddress = "AA:AA:AA:AA:AA:AA".parse().unwrap();
    assert!(bluetooth.connect_hands_free(unknown).await.is_err());
}

#[tokio::test]
async fn a_failed_connection_is_reported() {
    let mut failing = state(vec![phone()]);
    failing.connect_error = Some("br-connection-page-timeout");
    let m = mock_with(failing).await;
    let error = m
        .bluetooth()
        .connect_hands_free(PHONE.parse().unwrap())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("connect"), "{error}");
}

async fn emit(server: &Connection, path: &str, interface: &str, key: &str, on: bool) {
    let mut changed: HashMap<&str, Value<'_>> = HashMap::new();
    changed.insert(key, Value::from(on));
    server
        .emit_signal(
            None::<()>,
            path,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(interface, changed, Vec::<String>::new()),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn reports_changes_that_matter() {
    let m = mock().await;
    let bluetooth = Arc::new(m.bluetooth());
    let path = phone().path();
    // The first wait sets up the subscription.
    let waiting = tokio::spawn({
        let bluetooth = bluetooth.clone();
        async move { bluetooth.changed().await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    // Signal strength comes and goes while scanning: not a change.
    emit(&m.server, &path, DEVICE, "RSSI", true).await;
    emit(
        &m.server,
        &path,
        "org.bluez.MediaControl1",
        "Connected",
        true,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!waiting.is_finished());
    emit(&m.server, &path, DEVICE, "Connected", true).await;
    tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn signals_that_keep_coming_do_not_hold_a_change_back() {
    let m = mock().await;
    let bluetooth = Arc::new(m.bluetooth());
    let path = phone().path();
    let waiting = tokio::spawn({
        let bluetooth = bluetooth.clone();
        async move { bluetooth.changed().await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    emit(&m.server, &path, DEVICE, "Connected", true).await;
    // Another app scans: signal strength changes keep coming, closer
    // together than the burst settles.
    let scanning = tokio::spawn({
        let server = m.server.clone();
        async move {
            loop {
                emit(&server, &path, DEVICE, "RSSI", true).await;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    });
    let reported = tokio::time::timeout(Duration::from_secs(3), waiting).await;
    scanning.abort();
    reported.unwrap().unwrap().unwrap();
}
