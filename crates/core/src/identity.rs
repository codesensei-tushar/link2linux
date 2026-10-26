//! The fingerprints devices know each other by.
//!
//! Peers recognize each other by [`CertificateFingerprint`] alone: the
//! SHA-256 of the key's SubjectPublicKeyInfo. Names, dates and extensions in
//! the certificate are ignored.
//!
//! Every key must be P-256 with its SubjectPublicKeyInfo in the one
//! canonical DER form (named curve, uncompressed point, 91 bytes). Both TLS
//! stacks produce that form; a key in any other encoding is refused rather
//! than hashed, so the two devices can never disagree about a fingerprint.

use std::fmt;
use std::str::FromStr;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rustls::pki_types::CertificateDer;

/// Length of a canonical P-256 SubjectPublicKeyInfo in DER.
pub const P256_SPKI_LEN: usize = 91;

/// The first 27 bytes of every canonical P-256 SubjectPublicKeyInfo: the
/// `id-ecPublicKey` algorithm with the `prime256v1` curve, the BIT STRING
/// header, and `0x04` for an uncompressed point. The 64 bytes of the point
/// follow.
pub const P256_SPKI_PREFIX: [u8; 27] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04,
];

/// SHA-256 of a device's public key, taken over the canonical DER
/// SubjectPublicKeyInfo of its TLS certificate (`fp` in
/// `docs/protocol.md`). Peers pin this, not the certificate, so a device can
/// reissue its certificate for the same key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CertificateFingerprint(pub [u8; 32]);

impl CertificateFingerprint {
    /// The fingerprint of a DER SubjectPublicKeyInfo.
    ///
    /// # Errors
    ///
    /// Fails unless `spki` is a P-256 key in the canonical form.
    pub fn of_spki(spki: &[u8]) -> Result<Self, IdentityError> {
        if spki.len() != P256_SPKI_LEN || !spki.starts_with(&P256_SPKI_PREFIX) {
            return Err(IdentityError::NotP256);
        }
        Ok(Self(sha256(spki)))
    }

    /// The fingerprint of the key in a DER X.509 certificate.
    ///
    /// # Errors
    ///
    /// Fails if the certificate does not parse or its key is not a P-256
    /// key in the canonical form.
    pub fn of_certificate(certificate: &CertificateDer<'_>) -> Result<Self, IdentityError> {
        let parsed = rustls::server::ParsedCertificate::try_from(certificate)
            .map_err(|error| IdentityError::Certificate(error.to_string()))?;
        Self::of_spki(parsed.subject_public_key_info().as_ref())
    }

    /// The fingerprint from its 32 raw bytes, as carried in messages.
    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }

    /// The raw bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// base64url without padding (43 characters), as in the QR code.
    #[must_use]
    pub fn to_base64url(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }

    /// Parses the base64url form.
    ///
    /// # Errors
    ///
    /// Fails unless `text` is base64url without padding for 32 bytes.
    pub fn from_base64url(text: &str) -> Result<Self, IdentityError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(text)
            .map_err(|_| IdentityError::Encoding)?;
        Self::from_slice(&bytes).ok_or(IdentityError::Encoding)
    }
}

impl fmt::Display for CertificateFingerprint {
    /// Lowercase hex, two digits per byte.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for CertificateFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CertificateFingerprint({self})")
    }
}

impl FromStr for CertificateFingerprint {
    type Err = IdentityError;

    /// Parses the lowercase or uppercase hex form that `Display` writes.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bytes = decode_hex(text).ok_or(IdentityError::Encoding)?;
        Self::from_slice(&bytes).ok_or(IdentityError::Encoding)
    }
}

/// Why an identity or a fingerprint could not be made or read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IdentityError {
    /// The key is not an ECDSA P-256 key, or its SubjectPublicKeyInfo is not
    /// in the canonical form.
    #[error("the key is not a P-256 key in canonical form")]
    NotP256,
    /// A certificate did not parse.
    #[error("invalid certificate: {0}")]
    Certificate(String),
    /// Generating or decoding the key failed.
    #[error("invalid private key: {0}")]
    Key(String),
    /// A fingerprint or key was not in the expected text form.
    #[error("malformed fingerprint or key encoding")]
    Encoding,
    /// Reading or writing the key file failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// SHA-256 of `data`.
pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, data);
    digest
        .as_ref()
        .try_into()
        .expect("SHA-256 digests are 32 bytes")
}

/// Decodes hex with an even number of digits, in either case.
pub(crate) fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use rcgen::{KeyPair, PublicKeyData};

    use super::*;

    #[test]
    fn other_key_types_and_encodings_are_refused() {
        let ed25519 = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
        assert!(matches!(
            CertificateFingerprint::of_spki(&ed25519.subject_public_key_info()),
            Err(IdentityError::NotP256)
        ));
        let p384 = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P384_SHA384).unwrap();
        assert!(matches!(
            CertificateFingerprint::of_spki(&p384.subject_public_key_info()),
            Err(IdentityError::NotP256)
        ));

        // A compressed point: same curve, different bytes.
        let mut compressed = P256_SPKI_PREFIX[..23].to_vec();
        compressed.extend_from_slice(&[0x03, 0x22, 0x00, 0x02]);
        compressed.extend_from_slice(&[7; 32]);
        compressed[1] = 0x39;
        assert!(matches!(
            CertificateFingerprint::of_spki(&compressed),
            Err(IdentityError::NotP256)
        ));
    }

    #[test]
    fn fingerprint_text_forms_round_trip() {
        let fingerprint =
            CertificateFingerprint(std::array::from_fn(|i| (i as u8).wrapping_mul(37)));
        let base64 = fingerprint.to_base64url();
        assert_eq!(base64.len(), 43);
        assert_eq!(
            CertificateFingerprint::from_base64url(&base64).unwrap(),
            fingerprint
        );
        assert_eq!(
            fingerprint
                .to_string()
                .parse::<CertificateFingerprint>()
                .unwrap(),
            fingerprint
        );
        assert!("abc".parse::<CertificateFingerprint>().is_err());
        assert!(CertificateFingerprint::from_base64url("AAAA").is_err());
    }
}
