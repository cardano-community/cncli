use pallas_crypto::hash::{Hash, Hasher};
use pallas_crypto::vrf::{
    VrfProof, VrfPublicKey, VrfSecretKey, VRF_PROOF_SIZE, VRF_PUBLIC_KEY_SIZE, VRF_SECRET_KEY_SIZE,
};
use rand::{thread_rng, Rng};
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
    thread_rng().fill(&mut nonce_seed);
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

                    let vrf_skey: &[u8; VRF_SECRET_KEY_SIZE] = vrf_skey.key[0..VRF_SECRET_KEY_SIZE]
                        .try_into()
                        .expect("Invalid VRF signing key length");
                    let vrf_skey = VrfSecretKey::from(vrf_skey);
                    let vrf_proof = vrf_skey.prove(challenge_bytes.as_slice());
                    let signature = vrf_proof.signature();
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
                    let vkey_hash_verify = hex::encode(Hasher::<224>::hash(&vrf_vkey.key[0..VRF_SECRET_KEY_SIZE]));
                    debug!("vkey_hash_verify: {}", &vkey_hash_verify);

                    if pool_vrf_vkey_hash != vkey_hash_verify {
                        handle_error(format!(
                            "Hash of pool-vrf-vkey({vkey_hash_verify}) did not match supplied pool-vrf-vkey-hash({pool_vrf_vkey_hash})"
                        ));
                        return;
                    }

                    let vrf_public_key_bytes: [u8; VRF_PUBLIC_KEY_SIZE] =
                        match vrf_vkey.key[0..VRF_PUBLIC_KEY_SIZE].try_into() {
                            Ok(slice) => slice,
                            Err(_) => {
                                handle_error("Invalid VRF public key length");
                                return;
                            }
                        };

                    // Verify that the signature is a valid format. This will fail if the signature is mal-formed
                    match hex::decode(signature) {
                        Ok(signature_bytes) => {
                            let signature_slice: [u8; VRF_PROOF_SIZE] = match signature_bytes.as_slice().try_into() {
                                Ok(slice) => slice,
                                Err(_) => {
                                    handle_error("Invalid signature length");
                                    return;
                                }
                            };
                            let vrf_public_key = VrfPublicKey::from(&vrf_public_key_bytes);
                            let vrf_proof: VrfProof = VrfProof::from(&signature_slice);
                            let signature_hash = vrf_proof.to_hash();
                            debug!("signature_hash: {}", hex::encode(signature_hash));
                            match vrf_proof.verify(&vrf_public_key, challenge_bytes.as_slice()) {
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
        let vrf_skey_bytes: [u8; VRF_SECRET_KEY_SIZE] = hex::decode("adb9c97bec60189aa90d01d113e3ef405f03477d82a94f81da926c90cd46a374e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda4381").unwrap().as_slice()[0..VRF_SECRET_KEY_SIZE].try_into().unwrap();
        let vrf_skey: VrfSecretKey = VrfSecretKey::from(&vrf_skey_bytes);
        let vrf_vkey_bytes: [u8; VRF_PUBLIC_KEY_SIZE] =
            hex::decode("e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda4381")
                .unwrap()
                .as_slice()[0..VRF_PUBLIC_KEY_SIZE]
                .try_into()
                .unwrap();
        let vrf_vkey: VrfPublicKey = VrfPublicKey::from(&vrf_vkey_bytes);

        let challenge = create_challenge("pooltool.io").unwrap();
        let proof = vrf_skey.prove(challenge.as_slice());
        let proof_signature_hash = proof.to_hash();
        let verification_signature_hash = proof.verify(&vrf_vkey, challenge.as_slice()).unwrap();

        assert_eq!(proof_signature_hash, verification_signature_hash);
    }
}
