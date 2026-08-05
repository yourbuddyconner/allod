use allod_core::keys::{FileBackend, Signer};
use allod_core::policy::{attach_decider, build_decision_record, decision_payload};
use allod_core::sign::{verify, Keypair};
use serde_yaml::Value;

const SECRET: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const TS: &str = "2026-08-04T12:00:00Z";

fn fixture_record() -> (Value, String) {
    let policy: Value = serde_yaml::from_str("rules: []").unwrap();
    let rec = build_decision_record(&policy, "git:deadbeef", "approve", TS).unwrap();
    let payload = decision_payload(&rec).unwrap();
    (rec, payload)
}

fn signed_via(signer: &Signer, rec: &Value, payload: &str) -> Value {
    let mut rec = rec.clone();
    attach_decider(&mut rec, signer.name(), &signer.sign(payload).unwrap());
    rec
}

#[test]
fn all_backends_produce_identical_records() {
    let (rec, payload) = fixture_record();
    let kp = Keypair::from_secret_hex("parity", SECRET).unwrap();
    let public = kp.public_hex();

    // Reference: in-memory keypair (what wasm-internal signing uses).
    let reference = signed_via(&Signer::local(Keypair::from_secret_hex("parity", SECRET).unwrap()), &rec, &payload);

    // File backend.
    let tmp = std::env::temp_dir().join(format!("allod-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let be = FileBackend { create_dir: tmp.clone(), fallbacks: vec![] };
    let h = be.store("gid", &kp).unwrap();
    let via_file = signed_via(&Signer::from_backend(&be, h), &rec, &payload);
    assert_eq!(reference, via_file, "file backend must match in-memory signing byte for byte");

    // Keychain backend (macOS, opt-in).
    #[cfg(target_os = "macos")]
    if std::env::var("ALLOD_KEYCHAIN_TESTS").ok().as_deref() == Some("1") {
        use allod_core::keys_keychain::KeychainBackend;
        let kb = KeychainBackend { service: format!("allod-parity-{}", std::process::id()) };
        let kp2 = Keypair::from_secret_hex("parity", SECRET).unwrap();
        let h = kb.store("gid", &kp2).unwrap();
        let via_kc = signed_via(&Signer::from_backend(&kb, h), &rec, &payload);
        assert_eq!(reference, via_kc, "keychain backend must match");
        kb.delete("gid", "parity").unwrap();
    }

    // Every signature verifies against the public key.
    let sig = reference.get("deciders").unwrap().as_sequence().unwrap()[0]
        .get("signature").unwrap().as_str().unwrap();
    assert!(verify(&public, &payload, sig).is_ok());
    let _ = std::fs::remove_dir_all(&tmp);
}
