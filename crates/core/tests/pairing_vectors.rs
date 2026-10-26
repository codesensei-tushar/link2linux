//! Checks every construction against `proto/testdata/v1/pairing.txt`, the
//! vector the Android app checks too.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;

use link2linux_core::CertificateFingerprint;
use link2linux_core::pairing::crypto::{self, Role};
use link2linux_core::pairing::{PairingSecret, QrPayload};

const VECTOR: &str = include_str!("../../../proto/testdata/v1/pairing.txt");

/// The vector's `name = value` lines.
fn vector() -> BTreeMap<&'static str, &'static str> {
    VECTOR
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split_once(" = ").expect("name = value"))
        .collect()
}

fn hex(text: &str) -> Vec<u8> {
    assert!(text.len().is_multiple_of(2), "odd hex length");
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex digit"))
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn array<const N: usize>(text: &str) -> [u8; N] {
    hex(text).try_into().expect("length of the vector value")
}

#[test]
fn fingerprints_and_exporter_context() {
    let v = vector();
    let desktop = CertificateFingerprint::of_spki(&hex(v["spki_desktop"])).unwrap();
    let phone = CertificateFingerprint::of_spki(&hex(v["spki_phone"])).unwrap();
    assert_eq!(desktop.to_string(), v["fp_desktop"]);
    assert_eq!(phone.to_string(), v["fp_phone"]);
    assert_eq!(
        to_hex(&crypto::exporter_context(&desktop, &phone)),
        v["exporter_context"]
    );
}

/// The QR code of the vector: its secret, the vector's desktop key, and
/// the name and addresses `pairing.sh` writes into the URI.
fn qr_payload(v: &BTreeMap<&str, &str>) -> QrPayload {
    QrPayload {
        desktop: CertificateFingerprint::of_spki(&hex(v["spki_desktop"])).unwrap(),
        secret: PairingSecret::from_bytes(array(v["secret"])),
        name: "Büro workstation".into(),
        addresses: ["192.168.1.20:27381", "[fd00::20]:27381"]
            .iter()
            .map(|address| address.parse::<SocketAddr>().unwrap())
            .collect(),
        bluetooth_address: Some(array(v["bt_address"])),
    }
}

#[test]
fn qr_code_uri() {
    let v = vector();
    let payload = qr_payload(&v);
    assert_eq!(payload.to_uri(), v["qr_uri"]);
    assert_eq!(QrPayload::parse(v["qr_uri"]).unwrap(), payload);
}

#[test]
fn qr_pairing_proofs() {
    let v = vector();
    let secret = PairingSecret::from_bytes(array(v["secret"]));
    let ekm: [u8; 32] = array(v["ekm_pair"]);

    let desktop = crypto::pairing_proof(&secret, Role::Desktop, &ekm);
    let phone = crypto::pairing_proof(&secret, Role::Phone, &ekm);
    assert_eq!(to_hex(&desktop), v["proof_desktop"]);
    assert_eq!(to_hex(&phone), v["proof_phone"]);

    assert!(crypto::verify_pairing_proof(
        &secret,
        Role::Phone,
        &ekm,
        &hex(v["proof_phone"])
    ));
    assert!(!crypto::verify_pairing_proof(
        &secret,
        Role::Phone,
        &ekm,
        &hex(v["bad_proof_phone"])
    ));
    // A proof is never accepted for the other role.
    assert!(!crypto::verify_pairing_proof(
        &secret,
        Role::Desktop,
        &ekm,
        &phone
    ));
}

#[test]
fn sas_commitment_and_codes() {
    let v = vector();
    let ekm: [u8; 32] = array(v["ekm_sas"]);
    let phone_nonce: [u8; 32] = array(v["n_p"]);

    let commitment = crypto::sas_commitment(&ekm, &phone_nonce);
    assert_eq!(to_hex(&commitment), v["sas_commitment"]);
    assert!(crypto::verify_sas_commitment(
        &ekm,
        &phone_nonce,
        &commitment
    ));

    for (nonce, hmac, code) in [
        ("n_d", "sas_hmac", "sas_code"),
        ("n_d_2", "sas_hmac_2", "sas_code_2"),
    ] {
        let desktop_nonce: [u8; 32] = array(v[nonce]);
        assert_eq!(
            to_hex(&crypto::sas_hmac(&ekm, &desktop_nonce, &phone_nonce)),
            v[hmac]
        );
        let derived = crypto::sas_code(&ekm, &desktop_nonce, &phone_nonce);
        assert_eq!(derived.as_str(), v[code]);
    }
    assert_eq!(v["sas_code_2"].len(), 6);
    assert!(v["sas_code_2"].starts_with('0'));
}

/// `pairing.sh` must still produce the checked-in file, so the script and
/// the vector cannot drift apart. Skipped where `openssl` or `xxd` is
/// missing.
#[test]
fn generator_reproduces_the_vector() {
    let tools_present = ["openssl", "xxd"].iter().all(|tool| {
        Command::new("sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .is_ok_and(|output| output.status.success())
    });
    if !tools_present {
        eprintln!("skipping: openssl or xxd is not installed");
        return;
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto/testdata/pairing.sh");
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("v1")).unwrap();
    let script = dir.path().join("pairing.sh");
    std::fs::copy(&source, &script).unwrap();

    let status = Command::new("bash").arg(&script).output().unwrap();
    assert!(status.status.success(), "{status:?}");
    let generated = std::fs::read_to_string(dir.path().join("v1/pairing.txt")).unwrap();
    assert_eq!(generated, VECTOR);
}
