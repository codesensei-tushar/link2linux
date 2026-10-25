//! Presence and pairing tags, and the TXT record that carries them.
//!
//! A phone announces `_link2linux._tcp` over mDNS with a TXT record that
//! says nothing stable about it. Instead it carries a random nonce `n` and,
//! for each of up to three paired desktops, a tag
//! `HMAC-SHA256(presence_key, "l2l-presence-v1" || n)[..8]` that only that
//! desktop can recognize (`t`, `t2`, `t3`). While waiting for the desktop
//! that showed a QR code, the phone adds the pairing tag `q`. See
//! `docs/protocol.md`, section "mDNS".
//!
//! Everything here is a hint that only ever leads to a dial attempt; trust
//! comes from TLS.

use aws_lc_rs::{constant_time, hmac};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// Length of a presence or pairing tag in the TXT record, in bytes.
pub const TAG_LEN: usize = 8;
/// Length of the presence nonce, in bytes.
pub const NONCE_LEN: usize = 8;

const PRESENCE_LABEL: &[u8] = b"l2l-presence-v1";
const PAIRING_LABEL: &[u8] = b"l2l-pairing-tag-v1";

/// A presence or pairing tag: 8 bytes.
pub type Tag = [u8; TAG_LEN];
/// The presence nonce `n`: 8 random bytes.
pub type Nonce = [u8; NONCE_LEN];

/// The presence tag of `nonce` under a desktop's `presence_key`.
#[must_use]
pub fn presence_tag(presence_key: &[u8], nonce: &Nonce) -> Tag {
    let mut context = hmac::Context::with_key(&hmac::Key::new(hmac::HMAC_SHA256, presence_key));
    context.update(PRESENCE_LABEL);
    context.update(nonce);
    truncate(context.sign().as_ref())
}

/// The pairing tag `q` of a QR code's secret.
#[must_use]
pub fn pairing_tag(secret: &[u8]) -> Tag {
    let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, secret), PAIRING_LABEL);
    truncate(tag.as_ref())
}

fn truncate(mac: &[u8]) -> Tag {
    let mut tag = [0; TAG_LEN];
    tag.copy_from_slice(&mac[..TAG_LEN]);
    tag
}

/// Compares two tags without leaking where they differ.
#[must_use]
pub fn tags_equal(a: &Tag, b: &Tag) -> bool {
    constant_time::verify_slices_are_equal(a, b).is_ok()
}

/// Encodes 8 bytes the way TXT values carry them: base64url, no padding.
#[must_use]
pub fn encode_value(bytes: &[u8; 8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decodes a TXT value of 8 bytes. Returns `None` unless it is exactly the
/// 11 canonical base64url characters of 8 bytes.
#[must_use]
pub fn decode_value(text: &str) -> Option<[u8; 8]> {
    let bytes = URL_SAFE_NO_PAD.decode(text).ok()?;
    bytes.try_into().ok()
}

/// What a phone's TXT record says. Fields that are missing or malformed
/// are left out, and unknown keys are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TxtRecord {
    /// `v`: the highest protocol major version the phone speaks. It does
    /// not filter records; the `Hello` exchange settles the version.
    pub version: Option<u32>,
    /// `n`: the presence nonce.
    pub nonce: Option<Nonce>,
    /// The tags of `t`, `t2` and `t3` that are present and well formed.
    /// Empty without a nonce.
    pub presence_tags: Vec<Tag>,
    /// `q`: the pairing tag of the QR code the phone scanned.
    pub pairing_tag: Option<Tag>,
    /// `pm=1`: the phone is in SAS pairing mode.
    pub pairing_mode: bool,
}

impl TxtRecord {
    /// Parses `(key, value)` pairs. DNS-SD keys are case-insensitive.
    pub fn parse<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut record = Self::default();
        let mut tags: [Option<Tag>; 3] = [None; 3];
        for (key, value) in pairs {
            let key = key.to_ascii_lowercase();
            match key.as_str() {
                "v" => record.version = value.parse().ok(),
                "n" => record.nonce = decode_value(value),
                "t" => tags[0] = decode_value(value),
                "t2" => tags[1] = decode_value(value),
                "t3" => tags[2] = decode_value(value),
                "q" => record.pairing_tag = decode_value(value),
                "pm" => record.pairing_mode = value == "1",
                _ => {}
            }
        }
        if record.nonce.is_some() {
            record.presence_tags = tags.into_iter().flatten().collect();
        }
        record
    }

    /// Whether any presence tag was made with `presence_key` for this
    /// record's nonce. Compares every tag in constant time.
    #[must_use]
    pub fn matches_presence(&self, presence_key: &[u8]) -> bool {
        let Some(nonce) = &self.nonce else {
            return false;
        };
        let expected = presence_tag(presence_key, nonce);
        // No early exit, so timing does not tell which tag matched.
        self.presence_tags
            .iter()
            .fold(false, |found, tag| tags_equal(tag, &expected) | found)
    }

    /// Whether the record carries the pairing tag of `secret`.
    #[must_use]
    pub fn matches_pairing(&self, secret: &[u8]) -> bool {
        self.pairing_tag
            .is_some_and(|tag| tags_equal(&tag, &pairing_tag(secret)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::testvec::Vector;

    #[test]
    fn tags_match_the_vector() {
        let v = Vector::pairing();
        let nonce: Nonce = v.array("presence_nonce");
        assert_eq!(
            presence_tag(&v.bytes("presence_key"), &nonce),
            v.array::<8>("presence_tag")
        );
        assert_eq!(pairing_tag(&v.bytes("secret")), v.array::<8>("pairing_tag"));
    }

    #[test]
    fn values_match_the_vector() {
        let v = Vector::pairing();
        assert_eq!(encode_value(&v.array("presence_nonce")), v.text("txt_n"));
        assert_eq!(encode_value(&v.array("presence_tag")), v.text("txt_t"));
        assert_eq!(encode_value(&v.array("pairing_tag")), v.text("txt_q"));
        assert_eq!(decode_value(v.text("txt_t")), Some(v.array("presence_tag")));
    }

    #[test]
    fn decode_rejects_wrong_lengths_padding_and_alphabet() {
        assert_eq!(decode_value(""), None);
        assert_eq!(decode_value("49rVs5f0TQ"), None); // 7 bytes
        assert_eq!(decode_value("49rVs5f0TXYA"), None); // 9 bytes
        assert_eq!(decode_value("49rVs5f0TXY="), None); // padded
        assert_eq!(decode_value("49rVs5f0+XY"), None); // standard alphabet
        assert_eq!(decode_value("49rVs5f0TXZ"), None); // non-canonical trailing bits
    }

    #[test]
    fn parses_a_record_and_ignores_unknown_keys() {
        let v = Vector::pairing();
        let record = TxtRecord::parse([
            ("v", "1"),
            ("N", v.text("txt_n")),
            ("t", "AAAAAAAAAAA"),
            ("t3", v.text("txt_t")),
            ("q", v.text("txt_q")),
            ("pm", "1"),
            ("future", "x"),
        ]);
        assert_eq!(record.version, Some(1));
        assert_eq!(record.presence_tags.len(), 2);
        assert!(record.pairing_mode);
        assert!(record.matches_presence(&v.bytes("presence_key")));
        assert!(!record.matches_presence(&[0; 32]));
        assert!(record.matches_pairing(&v.bytes("secret")));
        assert!(!record.matches_pairing(&[0; 16]));
    }

    #[test]
    fn tags_without_a_nonce_are_ignored() {
        let v = Vector::pairing();
        let record = TxtRecord::parse([("t", v.text("txt_t")), ("v", "x"), ("pm", "0")]);
        assert_eq!(record, TxtRecord::default());
        assert!(!record.matches_presence(&v.bytes("presence_key")));
    }

    #[test]
    fn malformed_values_are_dropped() {
        let record = TxtRecord::parse([("n", "short"), ("q", "!!!"), ("v", "-1")]);
        assert_eq!(record, TxtRecord::default());
    }
}
