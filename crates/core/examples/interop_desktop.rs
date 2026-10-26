//! The desktop's side of the interop test with the phone app's TLS stack
//! (scripts/interop.sh). It pairs with the phone's real listener, which runs
//! Conscrypt on the JVM, so both TLS stacks, the exporter values and the
//! pairing constructions are checked against each other.
//!
//! A driver runs it and talks to it in lines:
//!
//! - `interop_desktop qr`: prints `qr <uri>`, reads the phone's port from
//!   stdin, pairs by QR code, prints `paired <phone key> <phone id>`, dials
//!   again with the stored key pins, prints `reconnected`, then unpairs with
//!   a `Goodbye` and prints `done`.
//! - `interop_desktop sas`: reads the port, pairs by comparing codes and
//!   prints `code <six digits>` when the code is shown, then `paired ...`
//!   and `done`. It confirms the code itself; the driver checks that the
//!   phone shows the same one.
//!
//! Failures go to stderr with a non-zero exit status.

use std::error::Error;
use std::io::{BufRead, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::Duration;

use link2linux_core::device::{DeviceInfo, DeviceKind};
use link2linux_core::identity::Identity;
use link2linux_core::pairing::{
    MemoryTrustStore, PairedDevice, PairingBinding, PairingCode, PairingPrompt, PairingSuccess,
    PairingTimeouts, QrOffer, SasDecision, TrustStore, pair_by_qr, pair_by_sas, records,
};
use link2linux_core::session::{Session, SessionConfig, SessionEvent, SessionStart};
use link2linux_core::transport::{PeerPin, TlsConnection, TlsDialer};
use link2linux_protocol::v1::{GoodbyeReason, PairingMethod};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// The whole run, so a stuck peer fails the test instead of hanging it.
const DEADLINE: Duration = Duration::from_secs(60);

/// A name that needs percent-encoding in the QR code.
const DESKTOP_NAME: &str = "Interop desk ü";

fn main() -> Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    match mode.as_str() {
        "qr" => runtime.block_on(with_deadline(qr())),
        "sas" => runtime.block_on(with_deadline(sas())),
        _ => Err("usage: interop_desktop qr|sas".into()),
    }
}

async fn with_deadline(run: impl Future<Output = Result<()>>) -> Result<()> {
    tokio::time::timeout(DEADLINE, run)
        .await
        .map_err(|_| "the interop run did not finish in time")?
}

async fn qr() -> Result<()> {
    let identity = Identity::generate()?;
    let dialer = TlsDialer::new(&identity)?;
    let store = Mutex::new(MemoryTrustStore::default());
    let beacons = SocketAddr::new(Ipv4Addr::new(192, 0, 2, 7).into(), 27381);
    let offer = Mutex::new(QrOffer::new(
        identity.fingerprint(),
        DESKTOP_NAME,
        vec![beacons],
        Some([0x0c, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]),
    ));
    say(&format!("qr {}", offer.lock().unwrap().uri()));
    let phone = phone_addr()?;

    let connection = dialer.dial(phone, PeerPin::Pairing).await?;
    let binding = PairingBinding::from_connection(&connection, PairingMethod::QrCode)?;
    let mut session = handshake(connection, Some(PairingMethod::QrCode), None).await?;
    let peer = session.peer().clone();
    let success = pair_by_qr(
        &mut session,
        &binding,
        &peer,
        &offer,
        &store,
        &mut Confirm,
        &PairingTimeouts::default(),
    )
    .await?;
    let record = paired(session, &store, &success).await?;

    // The phone now knows this desktop's key, and the record pins the
    // phone's.
    let connection = dialer.dial(phone, record.pin()).await?;
    let mut session = handshake(connection, None, Some(record.id.clone())).await?;
    session_start(&mut session).await?;
    say("reconnected");
    session
        .goodbye(GoodbyeReason::Unpaired, "interop test done")
        .await?;
    say("done");
    Ok(())
}

async fn sas() -> Result<()> {
    let identity = Identity::generate()?;
    let dialer = TlsDialer::new(&identity)?;
    let store = Mutex::new(MemoryTrustStore::default());
    let phone = phone_addr()?;

    let connection = dialer.dial(phone, PeerPin::Pairing).await?;
    let binding = PairingBinding::from_connection(&connection, PairingMethod::Sas)?;
    let mut session = handshake(connection, Some(PairingMethod::Sas), None).await?;
    let peer = session.peer().clone();
    let success = pair_by_sas(
        &mut session,
        &binding,
        &peer,
        &store,
        &mut Confirm,
        &PairingTimeouts::default(),
    )
    .await?;
    paired(session, &store, &success).await?;
    say("done");
    Ok(())
}

/// After a pairing: the session proper starts, and the phone's first
/// `FeatureStatus` and `PresenceUpdate` complete the record.
async fn paired(
    mut session: Session<TlsConnection>,
    store: &Mutex<MemoryTrustStore>,
    success: &PairingSuccess,
) -> Result<PairedDevice> {
    session.finish_pairing().await?;
    let fingerprint = success.device.fingerprint;
    let presence_key = session_start(&mut session).await?;
    {
        let mut store = store.lock().unwrap();
        records::confirm(&mut *store, &fingerprint)?;
        records::set_presence_key(&mut *store, &fingerprint, &presence_key)?;
    }
    session.close().await?;
    let record = store
        .lock()
        .unwrap()
        .get(&fingerprint)
        .ok_or("the pairing was not stored")?;
    say(&format!(
        "paired {} {}",
        fingerprint.to_base64url(),
        record.id
    ));
    Ok(record)
}

async fn handshake(
    connection: TlsConnection,
    pairing: Option<PairingMethod>,
    expected_peer_id: Option<String>,
) -> Result<Session<TlsConnection>> {
    let mut start = SessionStart::new(DeviceInfo::new(
        "interop-desktop",
        DESKTOP_NAME,
        DeviceKind::Desktop,
        [],
    ));
    start.pairing = pairing;
    start.expected_peer_id = expected_peer_id;
    Ok(Session::handshake(connection, start, SessionConfig::default()).await?)
}

/// Waits for the phone's `FeatureStatus` and then its `PresenceUpdate`, and
/// returns the presence key.
async fn session_start(session: &mut Session<TlsConnection>) -> Result<Vec<u8>> {
    let mut status = false;
    loop {
        match session.next_event().await? {
            Some(SessionEvent::FeatureStatus(_)) => status = true,
            Some(SessionEvent::Presence(presence)) if status => return Ok(presence.presence_key),
            Some(SessionEvent::Presence(_)) => {
                return Err("PresenceUpdate before FeatureStatus".into());
            }
            Some(_) => {}
            None => return Err("the phone ended the session".into()),
        }
    }
}

/// Confirms every question, and shows the SAS code to the driver.
struct Confirm;

impl PairingPrompt for Confirm {
    async fn confirm_code(
        &mut self,
        code: &PairingCode,
        _phone: &DeviceInfo,
        _replaces: &[PairedDevice],
    ) -> SasDecision {
        say(&format!("code {}", code.as_str()));
        SasDecision::Confirmed
    }

    async fn confirm_replace(&mut self, _phone: &DeviceInfo, _replaces: &[PairedDevice]) -> bool {
        true
    }
}

/// The phone's listener on loopback, from the port the driver writes.
fn phone_addr() -> Result<SocketAddr> {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let port: u16 = line.trim().parse()?;
    Ok(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port))
}

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}
