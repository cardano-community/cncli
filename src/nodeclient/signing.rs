use std::fmt::Display;
use std::io::stdout;
use std::path::Path;

use blake2b_simd::Params;
use log::debug;
use rand::{thread_rng, Rng};
use serde::Serialize;

use crate::nodeclient::leaderlog::libsodium::{
    sodium_crypto_vrf_proof_to_hash, sodium_crypto_vrf_prove, sodium_crypto_vrf_verify,
};
use crate::nodeclient::leaderlog::read_vrf_key;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SignVerifyError {
    status: String,
    error_message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChallengeSuccess {
    status: String,
    domain: String,
    nonce: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SignSuccess {
    status: String,
    signature: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifySuccess {
    status: String,
}

pub(crate) fn create_challenge(domain: &str) {
    let mut nonce_seed = [0u8; 64];
    thread_rng().fill(&mut nonce_seed);
    let nonce = hex::encode(nonce_seed);
    match hex::decode(hex::encode("cip-0022".as_bytes()) + &*hex::encode(domain.as_bytes()) + &*nonce) {
        Ok(challenge_seed) => {
            let challenge = Params::new()
                .hash_length(32)
                .to_state()
                .update(&challenge_seed)
                .finalize()
                .as_bytes()
                .to_owned();
            debug!("challenge: {}", hex::encode(challenge));
            serde_json::ser::to_writer_pretty(
                &mut stdout(),
                &ChallengeSuccess {
                    status: "ok".to_string(),
                    domain: domain.to_string(),
                    nonce,
                },
            )
            .unwrap();
        }
        Err(error) => handle_error(error),
    }
}

pub(crate) fn sign_challenge(pool_vrf_skey: &Path, domain: &str, nonce: &str) {
    let challenge_seed = hex::encode("cip-0022".as_bytes()) + &*hex::encode(domain.as_bytes()) + nonce;
    match hex::decode(challenge_seed) {
        Ok(challenge_seed_bytes) => {
            let challenge_bytes = Params::new()
                .hash_length(32)
                .to_state()
                .update(&challenge_seed_bytes)
                .finalize()
                .as_bytes()
                .to_owned();
            debug!("challenge: {}", hex::encode(&challenge_bytes));
            match read_vrf_key(pool_vrf_skey) {
                Ok(vrf_skey) => {
                    if vrf_skey.key_type != "VrfSigningKey_PraosVRF" {
                        handle_error("Pool VRF Skey must be of type: VrfSigningKey_PraosVRF");
                        return;
                    }
                    match sodium_crypto_vrf_prove(&vrf_skey.key, &challenge_bytes) {
                        Ok(signature) => {
                            debug!("signature: {}", hex::encode(&signature));
                            serde_json::ser::to_writer_pretty(
                                &mut stdout(),
                                &SignSuccess {
                                    status: "ok".to_string(),
                                    signature: hex::encode(&signature),
                                },
                            )
                            .unwrap();
                        }
                        Err(error) => handle_error(error),
                    }
                }
                Err(error) => handle_error(error),
            }
        }
        Err(error) => handle_error(error),
    }
}

pub(crate) fn verify_challenge(
    pool_vrf_vkey: &Path,
    pool_vrf_vkey_hash: &str,
    domain: &str,
    nonce: &str,
    signature: &str,
) {
    let challenge_seed = hex::encode("cip-0022".as_bytes()) + &*hex::encode(domain.as_bytes()) + nonce;
    match hex::decode(challenge_seed) {
        Ok(challenge_seed_bytes) => {
            let challenge_bytes = Params::new()
                .hash_length(32)
                .to_state()
                .update(&challenge_seed_bytes)
                .finalize()
                .as_bytes()
                .to_owned();
            debug!("challenge: {}", hex::encode(&challenge_bytes));
            match read_vrf_key(pool_vrf_vkey) {
                Ok(vrf_vkey) => {
                    if vrf_vkey.key_type != "VrfVerificationKey_PraosVRF" {
                        handle_error("Pool VRF Vkey must be of type: VrfVerificationKey_PraosVRF");
                        return;
                    }
                    // Verify that the vkey the client supplied is the same as the one on-chain
                    let vkey_hash_verify = hex::encode(
                        Params::new()
                            .hash_length(32)
                            .to_state()
                            .update(&vrf_vkey.key)
                            .finalize()
                            .as_bytes(),
                    );
                    debug!("vkey_hash_verify: {}", &vkey_hash_verify);

                    if pool_vrf_vkey_hash != vkey_hash_verify {
                        handle_error(format!(
                            "Hash of pool-vrf-vkey({vkey_hash_verify}) did not match supplied pool-vrf-vkey-hash({pool_vrf_vkey_hash})"
                        ));
                        return;
                    }

                    // Verify that the signature is a valid format. This will fail if the signature is mal-formed
                    match hex::decode(signature) {
                        Ok(signature_bytes) => {
                            match sodium_crypto_vrf_proof_to_hash(&signature_bytes) {
                                Ok(signature_hash) => {
                                    debug!("signature_hash: {}", hex::encode(&signature_hash));
                                    // Verify that the signature matches
                                    match sodium_crypto_vrf_verify(&vrf_vkey.key, &signature_bytes, &challenge_bytes) {
                                        Ok(verification) => {
                                            debug!("verification: {}", hex::encode(&verification));
                                            if verification != signature_hash {
                                                handle_error("Signature failed to match!");
                                                return;
                                            }
                                            serde_json::ser::to_writer_pretty(
                                                &mut stdout(),
                                                &VerifySuccess {
                                                    status: "ok".to_string(),
                                                },
                                            )
                                            .unwrap();
                                        }
                                        Err(error) => handle_error(error),
                                    }
                                }
                                Err(error) => handle_error(error),
                            }
                        }
                        Err(error) => handle_error(error),
                    }
                }
                Err(error) => handle_error(error),
            }
        }
        Err(error) => handle_error(error),
    }
}

fn handle_error<T: Display>(error_message: T) {
    serde_json::ser::to_writer_pretty(
        &mut stdout(),
        &SignVerifyError {
            status: "error".to_string(),
            error_message: format!("{error_message}"),
        },
    )
    .unwrap();
}
