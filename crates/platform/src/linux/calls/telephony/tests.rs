//! [`PipewireTelephony`] against a mock service on a peer-to-peer link.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::super::testing::link;
use super::*;

const AG0: &str = "/org/pipewire/Telephony/ag0";
const AG1: &str = "/org/pipewire/Telephony/ag1";
const PHONE: &str = "0C:1A:2B:3C:4D:5E";

#[derive(Debug, Clone)]
struct MockCall {
    path: &'static str,
    number: &'static str,
    name: &'static str,
    state: &'static str,
}

#[derive(Debug, Default)]
struct State {
    calls: Vec<MockCall>,
    /// The manager lists the calls too, as well as each gateway.
    manager_lists_calls: bool,
    muted: bool,
    /// What the service was asked, in order.
    seen: Vec<String>,
}

type Shared = Arc<StdMutex<State>>;

fn value(v: impl Into<Value<'static>>) -> OwnedValue {
    v.into().try_into().unwrap()
}

fn call_objects(calls: &[MockCall], under: &str) -> HashMap<OwnedObjectPath, Interfaces> {
    calls
        .iter()
        .filter(|call| call.path.starts_with(under))
        .map(|call| {
            let mut properties = HashMap::new();
            properties.insert("LineIdentification".to_owned(), value(call.number));
            properties.insert("Name".to_owned(), value(call.name));
            properties.insert("State".to_owned(), value(call.state));
            properties.insert("Multiparty".to_owned(), value(false));
            let mut interfaces = Interfaces::new();
            interfaces.insert(CALL.to_owned(), properties);
            (OwnedObjectPath::try_from(call.path).unwrap(), interfaces)
        })
        .collect()
}

struct Manager(Shared);

#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl Manager {
    fn get_managed_objects(&self) -> HashMap<OwnedObjectPath, Interfaces> {
        let state = self.0.lock().unwrap();
        let mut objects = HashMap::new();
        let mut first = HashMap::new();
        first.insert("Address".to_owned(), value(PHONE));
        let mut interfaces = Interfaces::new();
        interfaces.insert(GATEWAY.to_owned(), first);
        objects.insert(OwnedObjectPath::try_from(AG0).unwrap(), interfaces);
        // An older service that does not say whose gateway it is.
        let mut interfaces = Interfaces::new();
        interfaces.insert(GATEWAY.to_owned(), HashMap::new());
        objects.insert(OwnedObjectPath::try_from(AG1).unwrap(), interfaces);
        if state.manager_lists_calls {
            objects.extend(call_objects(&state.calls, "/"));
        }
        objects
    }
}

struct GatewayCalls {
    state: Shared,
    path: &'static str,
}

#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl GatewayCalls {
    fn get_managed_objects(&self) -> HashMap<OwnedObjectPath, Interfaces> {
        call_objects(&self.state.lock().unwrap().calls, self.path)
    }
}

struct MockGateway(Shared);

#[zbus::interface(name = "org.pipewire.Telephony.AudioGateway1")]
impl MockGateway {
    fn dial(&self, number: String) -> zbus::fdo::Result<()> {
        if number == "999999" {
            return Err(zbus::fdo::Error::InvalidArgs(format!(
                "cannot dial {number}"
            )));
        }
        self.0.lock().unwrap().seen.push(format!("dial {number}"));
        Ok(())
    }

    fn send_tones(&self, tones: String) {
        self.0.lock().unwrap().seen.push(format!("tones {tones}"));
    }
}

struct MockVolume(Shared);

#[zbus::interface(name = "org.ofono.CallVolume")]
impl MockVolume {
    fn get_properties(&self) -> HashMap<String, OwnedValue> {
        let mut properties = HashMap::new();
        properties.insert("Muted".to_owned(), value(self.0.lock().unwrap().muted));
        properties.insert("SpeakerVolume".to_owned(), value(50u8));
        properties
    }

    fn set_property(&self, name: String, value: OwnedValue) -> zbus::fdo::Result<()> {
        let mut state = self.0.lock().unwrap();
        if name != "Muted" {
            return Err(zbus::fdo::Error::InvalidArgs(name));
        }
        state.muted =
            bool::try_from(value).map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        let seen = format!("muted {}", state.muted);
        state.seen.push(seen);
        Ok(())
    }
}

struct MockCallObject {
    state: Shared,
    path: &'static str,
}

#[zbus::interface(name = "org.pipewire.Telephony.Call1")]
impl MockCallObject {
    fn answer(&self) {
        self.state
            .lock()
            .unwrap()
            .seen
            .push(format!("answer {}", self.path));
    }

    fn hangup(&self) {
        self.state
            .lock()
            .unwrap()
            .seen
            .push(format!("hangup {}", self.path));
    }
}

struct Mock {
    client: Connection,
    server: Connection,
    state: Shared,
}

impl Mock {
    fn telephony(&self) -> PipewireTelephony {
        PipewireTelephony::with_connection(self.client.clone())
    }

    fn seen(&self) -> Vec<String> {
        self.state.lock().unwrap().seen.clone()
    }
}

fn calls() -> Vec<MockCall> {
    vec![
        MockCall {
            path: "/org/pipewire/Telephony/ag0/call10",
            number: "+15550111",
            name: "",
            state: "held",
        },
        MockCall {
            path: "/org/pipewire/Telephony/ag0/call2",
            number: "+15550100",
            name: "Anna",
            state: "incoming",
        },
        MockCall {
            path: "/org/pipewire/Telephony/ag1/call0",
            number: "",
            name: "",
            state: "dialing",
        },
        // A state this desktop does not know is left out.
        MockCall {
            path: "/org/pipewire/Telephony/ag1/call1",
            number: "1",
            name: "",
            state: "teleporting",
        },
    ]
}

async fn mock(state: State) -> Mock {
    let paths: Vec<&'static str> = state.calls.iter().map(|call| call.path).collect();
    let state = Arc::new(StdMutex::new(state));
    let (server, client) = link(|builder| {
        let mut builder = builder
            .serve_at(MANAGER_PATH, Manager(state.clone()))?
            .serve_at(
                AG0,
                GatewayCalls {
                    state: state.clone(),
                    path: "/org/pipewire/Telephony/ag0/",
                },
            )?
            .serve_at(AG0, MockGateway(state.clone()))?
            .serve_at(AG0, MockVolume(state.clone()))?
            .serve_at(
                AG1,
                GatewayCalls {
                    state: state.clone(),
                    path: "/org/pipewire/Telephony/ag1/",
                },
            )?;
        for path in paths {
            builder = builder.serve_at(
                path,
                MockCallObject {
                    state: state.clone(),
                    path,
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

/// A gateway as the tests compare it: id, address, muted, and its calls.
type Summary = (String, Option<String>, Option<bool>, Vec<String>);

fn summary(gateways: &[Gateway]) -> Vec<Summary> {
    gateways
        .iter()
        .map(|gateway| {
            (
                gateway.id.clone(),
                gateway.address.map(|a| a.to_string()),
                gateway.muted,
                gateway
                    .calls
                    .iter()
                    .map(|c| format!("{} {}|{}|{:?}", c.id, c.number, c.name, c.state))
                    .collect(),
            )
        })
        .collect()
}

fn expected() -> Vec<Summary> {
    vec![
        (
            AG0.to_owned(),
            Some(PHONE.to_owned()),
            Some(false),
            vec![
                format!("{AG0}/call2 +15550100|Anna|Incoming"),
                format!("{AG0}/call10 +15550111||Held"),
            ],
        ),
        (
            AG1.to_owned(),
            None,
            None,
            vec![format!("{AG1}/call0 ||Dialing")],
        ),
    ]
}

#[tokio::test]
async fn lists_each_gateway_with_its_calls() {
    let m = mock(State {
        calls: calls(),
        ..State::default()
    })
    .await;
    let gateways = m.telephony().gateways().await.unwrap();
    assert_eq!(summary(&gateways), expected());
}

#[tokio::test]
async fn calls_the_manager_lists_too_are_taken_once() {
    let m = mock(State {
        calls: calls(),
        manager_lists_calls: true,
        ..State::default()
    })
    .await;
    let gateways = m.telephony().gateways().await.unwrap();
    assert_eq!(summary(&gateways), expected());
}

#[tokio::test]
async fn commands_reach_the_service() {
    let m = mock(State {
        calls: calls(),
        ..State::default()
    })
    .await;
    let telephony = m.telephony();
    let call = format!("{AG0}/call2");
    telephony.answer(&call).await.unwrap();
    telephony.hang_up(&call).await.unwrap();
    telephony.dial(AG0, "+15550123").await.unwrap();
    telephony.send_tones(AG0, "1#").await.unwrap();
    telephony.set_muted(AG0, true).await.unwrap();
    assert_eq!(
        m.seen(),
        [
            format!("answer {call}"),
            format!("hangup {call}"),
            "dial +15550123".to_owned(),
            "tones 1#".to_owned(),
            "muted true".to_owned(),
        ]
    );
    let gateways = telephony.gateways().await.unwrap();
    assert_eq!(gateways[0].muted, Some(true));
}

#[tokio::test]
async fn a_refused_dial_does_not_repeat_the_number() {
    let m = mock(State::default()).await;
    let error = m.telephony().dial(AG0, "999999").await.unwrap_err();
    let text = error.to_string();
    assert!(text.contains("Dial failed"), "{text}");
    assert!(!text.contains("999999"), "{text}");
}

#[tokio::test]
async fn ids_outside_the_service_are_refused() {
    let m = mock(State {
        calls: calls(),
        ..State::default()
    })
    .await;
    let telephony = m.telephony();
    for id in [
        "/org/bluez/hci0",
        MANAGER_PATH,
        "/org/pipewire/TelephonyX/ag0",
        "not a path",
        "/org/pipewire/Telephony/ag0/../ag1",
    ] {
        assert!(telephony.answer(id).await.is_err(), "{id}");
    }
    assert!(m.seen().is_empty());
}

#[tokio::test]
async fn no_service_means_unavailable() {
    // A peer that serves something else, so it answers at once.
    let (_server, client) = link(|builder| {
        builder.serve_at(
            "/unrelated",
            GatewayCalls {
                state: Shared::default(),
                path: "/",
            },
        )
    })
    .await;
    let telephony = PipewireTelephony::with_connection(client);
    assert!(matches!(
        telephony.gateways().await,
        Err(PlatformError::Unavailable { .. })
    ));
}

#[tokio::test]
async fn reports_changes() {
    let m = mock(State::default()).await;
    let telephony = Arc::new(m.telephony());
    let waiting = tokio::spawn({
        let telephony = telephony.clone();
        async move { telephony.changed().await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let added: HashMap<String, HashMap<String, OwnedValue>> = HashMap::new();
    m.server
        .emit_signal(
            None::<()>,
            AG0,
            OBJECT_MANAGER,
            "InterfacesAdded",
            &(ObjectPath::try_from(format!("{AG0}/call3")).unwrap(), added),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

/// Needs PipeWire 1.4 or later on the session bus, with a phone connected
/// over the Hands-Free Profile. Opt in with `L2L_REAL_TELEPHONY=1`.
#[tokio::test]
#[ignore = "needs PipeWire's telephony service and a connected phone"]
async fn lists_the_real_gateways() {
    if std::env::var_os("L2L_REAL_TELEPHONY").is_none() {
        return;
    }
    let gateways = PipewireTelephony::new().gateways().await.unwrap();
    println!("{gateways:?}");
}
