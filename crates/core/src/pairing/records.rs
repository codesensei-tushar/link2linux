//! The life cycle of a pairing record after pairing: what the session layer
//! calls when a paired phone connects, confirms, rotates its key or
//! unpairs. Each function maps one rule of `docs/protocol.md` onto the
//! [`TrustStore`].
//!
//! - [`check_peer`] when a pinned connection's `Hello` arrives;
//! - [`note_connected`] once that session is up;
//! - [`confirm`] on the phone's first `FeatureStatus`;
//! - [`set_presence_key`] on its `PresenceUpdate`;
//! - [`accept_key_rotation`] on its `KeyRotation`, before answering `Ack`;
//! - [`handle_goodbye`] on its `Goodbye`, and [`unpair`] when the user
//!   removes the phone here;
//! - [`certificate_rejected`] when the phone refuses this desktop's key.

use std::net::SocketAddr;

use link2linux_protocol::v1::{Goodbye, GoodbyeReason, KeyRotation};

use super::{CertificateFingerprint, PairedDevice, PairingError, PresenceKey, TrustStore};
use crate::identity::Identity;

/// How many past addresses a record keeps.
pub const MAX_LAST_ADDRESSES: usize = 4;

/// Finds the record for a pinned connection's peer and checks the device
/// id its `Hello` claims.
///
/// # Errors
///
/// Fails with [`PairingError::NotPaired`] when no record has this key, and
/// with [`PairingError::Protocol`] when the `Hello` names another device
/// id than the record: the session must then end with a protocol error.
pub fn check_peer<S: TrustStore + ?Sized>(
    store: &S,
    fingerprint: &CertificateFingerprint,
    hello_device_id: &str,
) -> Result<PairedDevice, PairingError> {
    let device = store
        .get(fingerprint)
        .ok_or(PairingError::NotPaired(*fingerprint))?;
    if device.id != hello_device_id {
        return Err(PairingError::Protocol(format!(
            "Hello device id {hello_device_id:?} does not match the pairing record"
        )));
    }
    Ok(device)
}

/// Records a session that came up with a paired device presenting
/// `fingerprint` from `address`.
///
/// When the device presented its announced next key, that key becomes the
/// current one and the old key is forgotten. The address moves to the front
/// of the record's last known addresses.
///
/// # Errors
///
/// Fails when no record has this key, or the store cannot be written.
pub fn note_connected<S: TrustStore + ?Sized>(
    store: &mut S,
    fingerprint: &CertificateFingerprint,
    address: SocketAddr,
) -> Result<PairedDevice, PairingError> {
    let mut device = store
        .get(fingerprint)
        .ok_or(PairingError::NotPaired(*fingerprint))?;
    let old = device.fingerprint;
    if device.fingerprint != *fingerprint {
        device.fingerprint = *fingerprint;
        device.next_fingerprint = None;
    }
    device.last_addresses.retain(|known| *known != address);
    device.last_addresses.insert(0, address);
    device.last_addresses.truncate(MAX_LAST_ADDRESSES);
    store.replace(&old, device.clone())?;
    Ok(device)
}

/// Marks the record as confirmed: the phone has stored the pairing too.
/// Returns whether it was unconfirmed before.
///
/// # Errors
///
/// Fails when no record has this key, or the store cannot be written.
pub fn confirm<S: TrustStore + ?Sized>(
    store: &mut S,
    fingerprint: &CertificateFingerprint,
) -> Result<bool, PairingError> {
    update(store, fingerprint, |device| {
        let was_unconfirmed = !device.confirmed;
        device.confirmed = true;
        was_unconfirmed
    })
}

/// Stores the presence key from the phone's `PresenceUpdate`.
///
/// # Errors
///
/// Fails when the key is not 32 bytes, when no record has this key, or the
/// store cannot be written.
pub fn set_presence_key<S: TrustStore + ?Sized>(
    store: &mut S,
    fingerprint: &CertificateFingerprint,
    presence_key: &[u8],
) -> Result<(), PairingError> {
    let key = PresenceKey::from_slice(presence_key)
        .ok_or_else(|| PairingError::Protocol("presence_key is not 32 bytes".into()))?;
    update(store, fingerprint, |device| device.presence_key = Some(key))
}

/// Accepts a paired device's `KeyRotation`: its next key is stored and
/// accepted from now on, next to the current one. The caller answers with
/// `Ack` once this returns.
///
/// # Errors
///
/// Fails with [`PairingError::Protocol`] when the fingerprint is not 32
/// bytes or already belongs to another record (a device cannot claim
/// another device's key), when no record has this key, or when the store
/// cannot be written.
pub fn accept_key_rotation<S: TrustStore + ?Sized>(
    store: &mut S,
    fingerprint: &CertificateFingerprint,
    rotation: &KeyRotation,
) -> Result<(), PairingError> {
    let next = CertificateFingerprint::from_slice(&rotation.next_spki_sha256)
        .ok_or_else(|| PairingError::Protocol("next_spki_sha256 is not 32 bytes".into()))?;
    let owner = store
        .get(fingerprint)
        .ok_or(PairingError::NotPaired(*fingerprint))?;
    let taken = store
        .list()
        .iter()
        .any(|device| device.fingerprint != owner.fingerprint && device.accepts(&next));
    if taken {
        return Err(PairingError::Protocol(
            "the announced key belongs to another paired device".into(),
        ));
    }
    update(store, fingerprint, |device| {
        if device.fingerprint != next {
            device.next_fingerprint = Some(next);
        }
    })
}

/// The `KeyRotation` that announces `next` as this desktop's next key.
#[must_use]
pub fn key_rotation_for(next: &Identity) -> KeyRotation {
    KeyRotation {
        next_spki_sha256: next.fingerprint().as_bytes().to_vec(),
    }
}

/// Handles a `Goodbye` from a paired device: `UNPAIRED` deletes its record.
/// Returns whether a record was deleted.
///
/// # Errors
///
/// Fails when the store cannot be written.
pub fn handle_goodbye<S: TrustStore + ?Sized>(
    store: &mut S,
    fingerprint: &CertificateFingerprint,
    goodbye: &Goodbye,
) -> Result<bool, PairingError> {
    if goodbye.reason() != GoodbyeReason::Unpaired {
        return Ok(false);
    }
    Ok(store.remove(fingerprint)?.is_some())
}

/// Removes a pairing on the user's request. Returns the removed record and
/// the `Goodbye` to send if a session with the device is up; the session
/// sends it and closes the connection.
///
/// # Errors
///
/// Fails when the store cannot be written.
pub fn unpair<S: TrustStore + ?Sized>(
    store: &mut S,
    fingerprint: &CertificateFingerprint,
) -> Result<Option<(PairedDevice, Goodbye)>, PairingError> {
    let Some(device) = store.remove(fingerprint)? else {
        return Ok(None);
    };
    let goodbye = Goodbye {
        reason: GoodbyeReason::Unpaired.into(),
        detail: "the user removed the pairing".into(),
    };
    Ok(Some((device, goodbye)))
}

/// What a rejected certificate means for a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// The record was never confirmed, so the pairing did not finish on the
    /// phone. It was deleted; ask the user to pair again.
    DeletedUnconfirmed(PairedDevice),
    /// The record was confirmed: the phone removed the pairing. Offer to
    /// remove it here as well.
    RemovedByPhone(PairedDevice),
}

/// Handles a connection to a paired phone that refused this desktop's
/// certificate after the handshake checked the phone's key
/// ([`SessionError::is_rejected_by_peer`]). Never call it for an alert
/// during the handshake, which anyone can send.
///
/// [`SessionError::is_rejected_by_peer`]: crate::session::SessionError::is_rejected_by_peer
///
/// # Errors
///
/// Fails when no record has this key, or the store cannot be written.
pub fn certificate_rejected<S: TrustStore + ?Sized>(
    store: &mut S,
    fingerprint: &CertificateFingerprint,
) -> Result<Rejection, PairingError> {
    let device = store
        .get(fingerprint)
        .ok_or(PairingError::NotPaired(*fingerprint))?;
    if device.confirmed {
        return Ok(Rejection::RemovedByPhone(device));
    }
    store.remove(fingerprint)?;
    Ok(Rejection::DeletedUnconfirmed(device))
}

/// Changes the record found by `fingerprint` in place.
fn update<S, T>(
    store: &mut S,
    fingerprint: &CertificateFingerprint,
    change: impl FnOnce(&mut PairedDevice) -> T,
) -> Result<T, PairingError>
where
    S: TrustStore + ?Sized,
{
    let mut device = store
        .get(fingerprint)
        .ok_or(PairingError::NotPaired(*fingerprint))?;
    let key = device.fingerprint;
    let result = change(&mut device);
    store.replace(&key, device)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use super::*;
    use crate::pairing::MemoryTrustStore;
    use crate::pairing::tests::phone;

    fn fp(byte: u8) -> CertificateFingerprint {
        CertificateFingerprint([byte; 32])
    }

    fn rotation(byte: u8) -> KeyRotation {
        KeyRotation {
            next_spki_sha256: vec![byte; 32],
        }
    }

    #[test]
    fn hello_ids_must_match_the_record_found_by_key() {
        let mut store = MemoryTrustStore::default();
        store.insert(phone("phone-a", 1)).unwrap();
        store.insert(phone("phone-b", 2)).unwrap();

        assert_eq!(check_peer(&store, &fp(1), "phone-a").unwrap().id, "phone-a");
        // Key 2 claiming to be phone-a is refused.
        assert!(matches!(
            check_peer(&store, &fp(2), "phone-a"),
            Err(PairingError::Protocol(_))
        ));
        assert!(matches!(
            check_peer(&store, &fp(9), "phone-a"),
            Err(PairingError::NotPaired(_))
        ));
    }

    #[test]
    fn key_rotation_switches_on_the_first_connection_with_the_next_key() {
        let mut store = MemoryTrustStore::default();
        store.insert(phone("a", 1)).unwrap();
        let address = SocketAddr::from((Ipv4Addr::new(10, 0, 0, 5), 27380));

        accept_key_rotation(&mut store, &fp(1), &rotation(7)).unwrap();
        let device = store.get(&fp(1)).unwrap();
        assert!(device.accepts(&fp(1)) && device.accepts(&fp(7)));

        // A connection with the old key changes nothing.
        note_connected(&mut store, &fp(1), address).unwrap();
        assert_eq!(store.get(&fp(7)).unwrap().fingerprint, fp(1));

        // The first one with the new key makes it current.
        let device = note_connected(&mut store, &fp(7), address).unwrap();
        assert_eq!(device.fingerprint, fp(7));
        assert_eq!(device.next_fingerprint, None);
        assert_eq!(device.last_addresses, [address]);
        assert_eq!(store.get(&fp(1)), None);
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn a_device_cannot_announce_another_devices_key() {
        let mut store = MemoryTrustStore::default();
        store.insert(phone("a", 1)).unwrap();
        store.insert(phone("b", 2)).unwrap();

        assert!(matches!(
            accept_key_rotation(&mut store, &fp(1), &rotation(2)),
            Err(PairingError::Protocol(_))
        ));
        assert!(matches!(
            accept_key_rotation(&mut store, &fp(1), &KeyRotation::default()),
            Err(PairingError::Protocol(_))
        ));
        assert_eq!(store.get(&fp(1)).unwrap().next_fingerprint, None);
    }

    #[test]
    fn addresses_keep_the_most_recent_first() {
        let mut store = MemoryTrustStore::default();
        store.insert(phone("a", 1)).unwrap();
        let addresses: Vec<SocketAddr> = (1..=6)
            .map(|n| SocketAddr::from((Ipv4Addr::new(10, 0, 0, n), 27380)))
            .collect();
        for address in &addresses {
            note_connected(&mut store, &fp(1), *address).unwrap();
        }
        note_connected(&mut store, &fp(1), addresses[3]).unwrap();
        let device = store.get(&fp(1)).unwrap();
        assert_eq!(
            device.last_addresses,
            [addresses[3], addresses[5], addresses[4], addresses[2]]
        );
    }

    #[test]
    fn confirmation_presence_and_unpairing() {
        let mut store = MemoryTrustStore::default();
        store.insert(phone("a", 1)).unwrap();

        assert!(confirm(&mut store, &fp(1)).unwrap());
        assert!(!confirm(&mut store, &fp(1)).unwrap());
        set_presence_key(&mut store, &fp(1), &[3; 32]).unwrap();
        assert_eq!(
            store.get(&fp(1)).unwrap().presence_key,
            Some(PresenceKey([3; 32]))
        );
        assert!(set_presence_key(&mut store, &fp(1), &[3; 31]).is_err());

        let shutdown = Goodbye {
            reason: GoodbyeReason::Shutdown.into(),
            ..Goodbye::default()
        };
        assert!(!handle_goodbye(&mut store, &fp(1), &shutdown).unwrap());
        let (removed, goodbye) = unpair(&mut store, &fp(1)).unwrap().unwrap();
        assert_eq!(removed.id, "a");
        assert_eq!(goodbye.reason(), GoodbyeReason::Unpaired);
        assert_eq!(unpair(&mut store, &fp(1)).unwrap(), None);

        store.insert(phone("b", 2)).unwrap();
        assert!(handle_goodbye(&mut store, &fp(2), &goodbye).unwrap());
        assert!(store.list().is_empty());
    }

    #[test]
    fn rejected_certificates_delete_only_unconfirmed_records() {
        let mut store = MemoryTrustStore::default();
        store.insert(phone("a", 1)).unwrap();
        let mut confirmed = phone("b", 2);
        confirmed.confirmed = true;
        store.insert(confirmed).unwrap();

        assert!(matches!(
            certificate_rejected(&mut store, &fp(1)).unwrap(),
            Rejection::DeletedUnconfirmed(_)
        ));
        assert_eq!(store.get(&fp(1)), None);
        assert!(matches!(
            certificate_rejected(&mut store, &fp(2)).unwrap(),
            Rejection::RemovedByPhone(_)
        ));
        assert!(store.get(&fp(2)).is_some());
    }
}
