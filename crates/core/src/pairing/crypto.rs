//! The byte-exact constructions of `docs/protocol.md`, "Pairing": the
//! exporter labels and context, the QR proofs, and the SAS commitment and
//! code. `proto/testdata/v1/pairing.txt` holds test vectors for every
//! function here, and the Android app checks the same file. The tags a
//! phone announces (the pairing tag, presence tags and beacons) are in
//! [`discovery`](crate::discovery).
//!
//! Comparisons of received values run in constant time.

use std::fmt;

use aws_lc_rs::{constant_time, digest, hmac};
use link2linux_protocol::v1::PairingMethod;
use zeroize::Zeroize;

use super::PairingCode;
use crate::identity::CertificateFingerprint;

/// TLS exporter label for QR code pairing.
pub const PAIR_EXPORTER_LABEL: &str = "EXPERIMENTAL-link2linux-pair-v1";
/// TLS exporter label for SAS pairing.
pub const SAS_EXPORTER_LABEL: &str = "EXPERIMENTAL-link2linux-sas-v1";
/// Length of the exporter value, `ekm`.
pub const EKM_LEN: usize = 32;
/// Length of an SAS nonce.
pub const NONCE_LEN: usize = 32;

const PROOF_LABEL: &[u8] = b"l2l-pair-v1";
const SAS_COMMIT_LABEL: &[u8] = b"l2l-sas-commit-v1";
const SAS_LABEL: &[u8] = b"l2l-sas-v1";

/// The one-time secret of a QR code: 16 random bytes. Zeroed on drop, and
/// never printed by `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingSecret([u8; 16]);

impl PairingSecret {
    /// A new secret from the system's cryptographic random generator.
    #[must_use]
    pub fn generate() -> Self {
        Self(random_bytes())
    }

    /// Wraps known bytes, for tests and for the phone side.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The raw bytes, the HMAC key of proofs, tags and pairing beacons.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl Drop for PairingSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for PairingSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingSecret(..)")
    }
}

/// Which side computed a [`pairing_proof`]. The role byte keeps a proof from
/// being reflected back to its sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The desktop, role byte `0x01`.
    Desktop,
    /// The phone, role byte `0x02`.
    Phone,
}

impl Role {
    const fn byte(self) -> u8 {
        match self {
            Self::Desktop => 0x01,
            Self::Phone => 0x02,
        }
    }
}

/// The exporter label for a pairing method, or `None` for
/// [`PairingMethod::Unspecified`].
#[must_use]
pub const fn exporter_label(method: PairingMethod) -> Option<&'static str> {
    match method {
        PairingMethod::QrCode => Some(PAIR_EXPORTER_LABEL),
        PairingMethod::Sas => Some(SAS_EXPORTER_LABEL),
        PairingMethod::Unspecified => None,
    }
}

/// The exporter context: `fp_D || fp_P`, always 64 bytes and always passed.
#[must_use]
pub fn exporter_context(
    desktop: &CertificateFingerprint,
    phone: &CertificateFingerprint,
) -> [u8; 64] {
    let mut context = [0u8; 64];
    context[..32].copy_from_slice(desktop.as_bytes());
    context[32..].copy_from_slice(phone.as_bytes());
    context
}

/// `HMAC-SHA256(secret, "l2l-pair-v1" || role || ekm)`, the `mac` of a
/// `PairingProof`.
#[must_use]
pub fn pairing_proof(secret: &PairingSecret, role: Role, ekm: &[u8; EKM_LEN]) -> [u8; 32] {
    to_array(hmac::sign(&proof_key(secret), &proof_input(role, ekm)).as_ref())
}

/// Whether `mac` is the proof `role` must send, compared in constant time.
#[must_use]
pub fn verify_pairing_proof(
    secret: &PairingSecret,
    role: Role,
    ekm: &[u8; EKM_LEN],
    mac: &[u8],
) -> bool {
    hmac::verify(&proof_key(secret), &proof_input(role, ekm), mac).is_ok()
}

/// `SHA-256("l2l-sas-commit-v1" || ekm || n_p)`, the phone's `SasCommit`.
#[must_use]
pub fn sas_commitment(ekm: &[u8; EKM_LEN], phone_nonce: &[u8; NONCE_LEN]) -> [u8; 32] {
    let mut context = digest::Context::new(&digest::SHA256);
    context.update(SAS_COMMIT_LABEL);
    context.update(ekm);
    context.update(phone_nonce);
    to_array(context.finish().as_ref())
}

/// Whether `commitment` commits to `phone_nonce`, compared in constant time.
#[must_use]
pub fn verify_sas_commitment(
    ekm: &[u8; EKM_LEN],
    phone_nonce: &[u8; NONCE_LEN],
    commitment: &[u8],
) -> bool {
    constant_time::verify_slices_are_equal(&sas_commitment(ekm, phone_nonce), commitment).is_ok()
}

/// `HMAC-SHA256(ekm, "l2l-sas-v1" || n_d || n_p)`, the value the SAS code
/// is cut from.
#[must_use]
pub fn sas_hmac(
    ekm: &[u8; EKM_LEN],
    desktop_nonce: &[u8; NONCE_LEN],
    phone_nonce: &[u8; NONCE_LEN],
) -> [u8; 32] {
    let mut context = hmac::Context::with_key(&hmac::Key::new(hmac::HMAC_SHA256, ekm));
    context.update(SAS_LABEL);
    context.update(desktop_nonce);
    context.update(phone_nonce);
    to_array(context.sign().as_ref())
}

/// The six-digit code: the first 4 bytes of [`sas_hmac`] as a big-endian
/// unsigned integer, mod 1 000 000, with leading zeros.
#[must_use]
pub fn sas_code(
    ekm: &[u8; EKM_LEN],
    desktop_nonce: &[u8; NONCE_LEN],
    phone_nonce: &[u8; NONCE_LEN],
) -> PairingCode {
    let h = sas_hmac(ekm, desktop_nonce, phone_nonce);
    let value = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) % 1_000_000;
    PairingCode::from_number(value)
}

/// Fresh random bytes from the system's cryptographic generator.
pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    aws_lc_rs::rand::fill(&mut bytes).expect("the system random generator failed");
    bytes
}

fn proof_key(secret: &PairingSecret) -> hmac::Key {
    hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes())
}

fn proof_input(role: Role, ekm: &[u8; EKM_LEN]) -> Vec<u8> {
    let mut input = Vec::with_capacity(PROOF_LABEL.len() + 1 + EKM_LEN);
    input.extend_from_slice(PROOF_LABEL);
    input.push(role.byte());
    input.extend_from_slice(ekm);
    input
}

fn to_array<const N: usize>(bytes: &[u8]) -> [u8; N] {
    bytes[..N].try_into().expect("the digest is long enough")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proofs_depend_on_role_secret_and_ekm() {
        let secret = PairingSecret::from_bytes([1; 16]);
        let ekm = [2; 32];
        let desktop = pairing_proof(&secret, Role::Desktop, &ekm);
        let phone = pairing_proof(&secret, Role::Phone, &ekm);

        assert_ne!(desktop, phone);
        assert!(verify_pairing_proof(&secret, Role::Desktop, &ekm, &desktop));
        assert!(!verify_pairing_proof(&secret, Role::Phone, &ekm, &desktop));
        assert!(!verify_pairing_proof(
            &secret,
            Role::Desktop,
            &[3; 32],
            &desktop
        ));
        let other = PairingSecret::from_bytes([9; 16]);
        assert!(!verify_pairing_proof(&other, Role::Desktop, &ekm, &desktop));
        assert!(!verify_pairing_proof(
            &secret,
            Role::Desktop,
            &ekm,
            &desktop[..31]
        ));
    }

    #[test]
    fn commitment_binds_the_nonce() {
        let ekm = [4; 32];
        let commitment = sas_commitment(&ekm, &[5; 32]);
        assert!(verify_sas_commitment(&ekm, &[5; 32], &commitment));
        assert!(!verify_sas_commitment(&ekm, &[6; 32], &commitment));
        assert!(!verify_sas_commitment(&[0; 32], &[5; 32], &commitment));
    }

    #[test]
    fn codes_keep_leading_zeros() {
        // Search a few nonces for a code below 100000.
        let small = (0u8..=255)
            .map(|n| sas_code(&[0; 32], &[n; 32], &[0; 32]))
            .find(|code| code.as_str().starts_with('0'))
            .expect("one of 256 codes starts with 0");
        assert_eq!(small.as_str().len(), 6);
    }

    #[test]
    fn labels_follow_the_method() {
        assert_eq!(
            exporter_label(PairingMethod::QrCode),
            Some(PAIR_EXPORTER_LABEL)
        );
        assert_eq!(exporter_label(PairingMethod::Sas), Some(SAS_EXPORTER_LABEL));
        assert_eq!(exporter_label(PairingMethod::Unspecified), None);
    }
}
