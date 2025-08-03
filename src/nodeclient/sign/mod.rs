use amaru_ouroboros::vrf::{Input, Proof, PublicKey, SecretKey};
use pallas_crypto::hash::{Hash, Hasher};
use rand::{rng, Rng};
use serde::Serialize;
use std::fmt::Display;
use std::io::stdout;
use std::path::Path;
use tracing::debug;

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

pub(crate) fn create_challenge(domain: &str) -> Option<Hash<32>> {
    let mut nonce_seed = [0u8; 64];
    rng().fill(&mut nonce_seed);
    let nonce = hex::encode(nonce_seed);
    match hex::decode(hex::encode("cip-0022".as_bytes()) + &*hex::encode(domain.as_bytes()) + &*nonce) {
        Ok(challenge_seed) => {
            let challenge = Hasher::<256>::hash(&challenge_seed);
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
            Some(challenge)
        }
        Err(error) => {
            handle_error(error);
            None
        }
    }
}

pub(crate) fn sign_challenge(pool_vrf_skey: &Path, domain: &str, nonce: &str) {
    let challenge_seed = hex::encode("cip-0022".as_bytes()) + &*hex::encode(domain.as_bytes()) + nonce;
    match hex::decode(challenge_seed) {
        Ok(challenge_seed_bytes) => {
            let challenge_bytes = Hasher::<256>::hash(&challenge_seed_bytes);
            debug!("challenge: {}", hex::encode(challenge_bytes));
            match read_vrf_key(pool_vrf_skey) {
                Ok(vrf_skey) => {
                    if vrf_skey.key_type != "VrfSigningKey_PraosVRF" {
                        handle_error("Pool VRF Skey must be of type: VrfSigningKey_PraosVRF");
                        return;
                    }

                    let vrf_skey: &[u8; SecretKey::SIZE] = vrf_skey.key[0..SecretKey::SIZE]
                        .try_into()
                        .expect("Invalid VRF signing key length");
                    let vrf_skey = SecretKey::from(vrf_skey);
                    let vrf_challenge_input: Input =
                        Input::try_from(challenge_bytes.as_ref()).expect("Failed to convert challenge bytes to Input");
                    let vrf_proof = vrf_skey.prove(&vrf_challenge_input);
                    let signature: Hash<{ Proof::HASH_SIZE }> = Hash::from(&vrf_proof);
                    debug!("signature: {}", hex::encode(signature));
                    serde_json::ser::to_writer_pretty(
                        &mut stdout(),
                        &SignSuccess {
                            status: "ok".to_string(),
                            signature: hex::encode(signature),
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
            let challenge_bytes = Hasher::<256>::hash(&challenge_seed_bytes);
            debug!("challenge: {}", hex::encode(challenge_bytes));
            match read_vrf_key(pool_vrf_vkey) {
                Ok(vrf_vkey) => {
                    if vrf_vkey.key_type != "VrfVerificationKey_PraosVRF" {
                        handle_error("Pool VRF Vkey must be of type: VrfVerificationKey_PraosVRF");
                        return;
                    }
                    // Verify that the vkey the client supplied is the same as the one on-chain
                    let vkey_hash_verify = hex::encode(Hasher::<224>::hash(&vrf_vkey.key[0..SecretKey::SIZE]));
                    debug!("vkey_hash_verify: {}", &vkey_hash_verify);

                    if pool_vrf_vkey_hash != vkey_hash_verify {
                        handle_error(format!(
                            "Hash of pool-vrf-vkey({vkey_hash_verify}) did not match supplied pool-vrf-vkey-hash({pool_vrf_vkey_hash})"
                        ));
                        return;
                    }

                    let vrf_public_key_bytes: [u8; PublicKey::SIZE] = match vrf_vkey.key[0..PublicKey::SIZE].try_into()
                    {
                        Ok(slice) => slice,
                        Err(_) => {
                            handle_error("Invalid VRF public key length");
                            return;
                        }
                    };

                    // Verify that the signature is a valid format. This will fail if the signature is mal-formed
                    match hex::decode(signature) {
                        Ok(signature_bytes) => {
                            let signature_slice: [u8; Proof::SIZE] = match signature_bytes.as_slice().try_into() {
                                Ok(slice) => slice,
                                Err(_) => {
                                    handle_error("Invalid signature length");
                                    return;
                                }
                            };
                            let vrf_public_key = PublicKey::from(&vrf_public_key_bytes);
                            let vrf_proof: Proof =
                                Proof::try_from(&signature_slice).expect("Failed to convert signature bytes to Proof");
                            let signature_hash: Hash<{ Proof::HASH_SIZE }> = Hash::from(&vrf_proof);
                            debug!("signature_hash: {}", hex::encode(signature_hash));
                            let vrf_challenge_input: Input = Input::try_from(challenge_bytes.as_ref())
                                .expect("Failed to convert challenge bytes to Input");
                            match vrf_proof.verify(&vrf_public_key, &vrf_challenge_input) {
                                Ok(verification) => {
                                    debug!("verification: {}", hex::encode(verification));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cip_0022_verification() {
        // Node operational VRF-Verification-Key: pool.vrf.vkey
        // {
        //    "type": "VrfVerificationKey_PraosVRF",
        //    "description": "VRF Verification Key",
        //    "cborHex": "5820e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda4381"
        // }
        //
        // Node operational VRF-Signing-Key: pool.vrf.skey
        // {
        //    "type": "VrfSigningKey_PraosVRF",
        //    "description": "VRF Signing Key",
        //    "cborHex": "5840adb9c97bec60189aa90d01d113e3ef405f03477d82a94f81da926c90cd46a374e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda4381"
        // }
        let vrf_skey_bytes: [u8; SecretKey::SIZE] = hex::decode("adb9c97bec60189aa90d01d113e3ef405f03477d82a94f81da926c90cd46a374e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda4381").unwrap().as_slice()[0..SecretKey::SIZE].try_into().unwrap();
        let vrf_skey: SecretKey = SecretKey::from(&vrf_skey_bytes);
        let vrf_vkey_bytes: [u8; PublicKey::SIZE] =
            hex::decode("e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda4381")
                .unwrap()
                .as_slice()[0..PublicKey::SIZE]
                .try_into()
                .unwrap();
        let vrf_vkey: PublicKey = PublicKey::from(&vrf_vkey_bytes);

        let challenge = create_challenge("pooltool.io").unwrap();
        let challenge_input: Input =
            Input::try_from(challenge.as_ref()).expect("Failed to convert challenge bytes to Input");
        let proof = vrf_skey.prove(&challenge_input);
        let proof_signature_hash: Hash<{ Proof::HASH_SIZE }> = Hash::from(&proof);
        let verification_signature_hash = proof.verify(&vrf_vkey, &challenge_input).unwrap();

        assert_eq!(proof_signature_hash, verification_signature_hash);
    }
}
