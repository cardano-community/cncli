use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use vrf_dalek::vrf03::{VrfProof03, PROOF_SIZE};

const DOMAIN: &str = "pooltool.io";
const NONCE: &str = "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
const VKEY_HASH: &str = "f58bf0111f8e9b233c2dcbb72b5ad400330cf260c6fb556eb30cefd387e5364c";
const SKEY_CBOR: &str = "5840adb9c97bec60189aa90d01d113e3ef405f03477d82a94f81da926c90cd46a374e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda4381";
const VKEY_CBOR: &str = "5820e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda4381";

struct FixtureDir(PathBuf);

impl FixtureDir {
    fn new(test_name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("cncli-{}-{test_name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn key(&self, name: &str, key_type: &str, cbor_hex: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "type": key_type,
                "description": "test fixture",
                "cborHex": cbor_hex,
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[String]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_cncli"))
        .args(args)
        .env("RUST_LOG", "error")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cncli exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid JSON ({error}): {}", String::from_utf8_lossy(&output.stdout)))
}

fn sign(key: &Path, nonce: &str) -> Value {
    run(&[
        "sign".into(),
        "--pool-vrf-skey".into(),
        key.display().to_string(),
        "--domain".into(),
        DOMAIN.into(),
        "--nonce".into(),
        nonce.into(),
    ])
}

fn verify(key: &Path, key_hash: &str, nonce: &str, signature: &str) -> Value {
    run(&[
        "verify".into(),
        "--pool-vrf-vkey".into(),
        key.display().to_string(),
        "--pool-vrf-vkey-hash".into(),
        key_hash.into(),
        "--domain".into(),
        DOMAIN.into(),
        "--nonce".into(),
        nonce.into(),
        "--signature".into(),
        signature.into(),
    ])
}

fn assert_error(output: &Value) {
    assert_eq!(output["status"], "error");
    assert!(output["errorMessage"].is_string());
}

#[test]
fn sign_verify_round_trip() {
    let fixtures = FixtureDir::new("round-trip");
    let skey = fixtures.key("pool.vrf.skey", "VrfSigningKey_PraosVRF", SKEY_CBOR);
    let vkey = fixtures.key("pool.vrf.vkey", "VrfVerificationKey_PraosVRF", VKEY_CBOR);

    let signed = sign(&skey, NONCE);
    assert_eq!(signed["status"], "ok");
    let signature = signed["signature"].as_str().unwrap();
    assert_eq!(hex::decode(signature).unwrap().len(), PROOF_SIZE);

    assert_eq!(verify(&vkey, VKEY_HASH, NONCE, signature)["status"], "ok");
    assert_error(&verify(&vkey, VKEY_HASH, &"01".repeat(64), signature));
    assert_error(&verify(&vkey, &"00".repeat(32), NONCE, signature));
}

#[test]
fn sign_verify_rejects_malformed_input() {
    let fixtures = FixtureDir::new("malformed-input");
    let vkey = fixtures.key("pool.vrf.vkey", "VrfVerificationKey_PraosVRF", VKEY_CBOR);

    for (name, cbor_hex) in [
        ("invalid-hex", "gg".to_string()),
        ("invalid-cbor", "ff".to_string()),
        ("non-bytes", "00".to_string()),
        ("empty", "40".to_string()),
        ("short", format!("581f{}", "00".repeat(31))),
    ] {
        let skey = fixtures.key(&format!("{name}.skey"), "VrfSigningKey_PraosVRF", &cbor_hex);
        assert_error(&sign(&skey, NONCE));
    }

    for (name, cbor_hex) in [
        ("empty", "40".to_string()),
        ("short", format!("581f{}", "00".repeat(31))),
    ] {
        let short_vkey = fixtures.key(&format!("{name}.vkey"), "VrfVerificationKey_PraosVRF", &cbor_hex);
        assert_error(&verify(&short_vkey, VKEY_HASH, NONCE, &"00".repeat(PROOF_SIZE)));
    }

    assert_error(&verify(&vkey, VKEY_HASH, NONCE, "zz"));
    assert_error(&verify(&vkey, VKEY_HASH, NONCE, &"00".repeat(64)));

    let mut malformed_proof = [0; PROOF_SIZE];
    malformed_proof[0] = 2;
    assert!(VrfProof03::from_bytes(&malformed_proof).is_err());
    assert_error(&verify(&vkey, VKEY_HASH, NONCE, &hex::encode(malformed_proof)));

    let invalid_proof = [0; PROOF_SIZE];
    assert!(VrfProof03::from_bytes(&invalid_proof).is_ok());
    assert_error(&verify(&vkey, VKEY_HASH, NONCE, &hex::encode(invalid_proof)));
}
