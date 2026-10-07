use pallas_crypto::hash::{Hash, Hasher};
use rand::{rng, RngExt};
use serde::Serialize;
use std::io::stdout;
use std::path::Path;
use tracing::debug;
use vrf_dalek::vrf03::{PublicKey03, SecretKey03, VrfProof03, PROOF_SIZE};

use crate::nodeclient::leaderlog::read_vrf_key;

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

pub(crate) fn create_challenge(domain: &str) -> Result<Hash<32>, Box<dyn std::error::Error + Send + Sync>> {
    let mut nonce_seed = [0u8; 64];
    rng().fill(&mut nonce_seed);
    let nonce = hex::encode(nonce_seed);
    let challenge_seed = hex::decode(hex::encode("cip-0022".as_bytes()) + &hex::encode(domain.as_bytes()) + &nonce)?;
    let challenge = Hasher::<256>::hash(&challenge_seed);
    debug!("challenge: {}", hex::encode(challenge));
    crate::write_json(
        &mut stdout().lock(),
        &ChallengeSuccess {
            status: "ok".to_string(),
            domain: domain.to_string(),
            nonce,
        },
    )?;
    Ok(challenge)
}

pub(crate) fn sign_challenge(
    pool_vrf_skey: &Path,
    domain: &str,
    nonce: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let challenge_seed = hex::encode("cip-0022".as_bytes()) + &hex::encode(domain.as_bytes()) + nonce;
    let challenge_bytes = Hasher::<256>::hash(&hex::decode(challenge_seed)?);
    debug!("challenge: {}", hex::encode(challenge_bytes));
    let vrf_skey = read_vrf_key(pool_vrf_skey)?;
    if vrf_skey.key_type != "VrfSigningKey_PraosVRF" {
        return Err("Pool VRF Skey must be of type: VrfSigningKey_PraosVRF".into());
    }
    let vrf_skey = vrf_skey
        .key
        .get(..32)
        .and_then(|bytes| <&[u8; 32]>::try_from(bytes).ok())
        .ok_or("Invalid VRF signing key length")?;
    let vrf_skey = SecretKey03::from_bytes(vrf_skey);
    let vrf_public_key = PublicKey03::from(&vrf_skey);
    let vrf_proof = VrfProof03::generate(&vrf_public_key, &vrf_skey, challenge_bytes.as_ref());
    let signature = vrf_proof.to_bytes();
    debug!("signature: {}", hex::encode(signature));
    crate::write_json(
        &mut stdout().lock(),
        &SignSuccess {
            status: "ok".to_string(),
            signature: hex::encode(signature),
        },
    )?;
    Ok(())
}

pub(crate) fn verify_challenge(
    pool_vrf_vkey: &Path,
    pool_vrf_vkey_hash: &str,
    domain: &str,
    nonce: &str,
    signature: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let challenge_seed = hex::encode("cip-0022".as_bytes()) + &hex::encode(domain.as_bytes()) + nonce;
    let challenge_bytes = Hasher::<256>::hash(&hex::decode(challenge_seed)?);
    debug!("challenge: {}", hex::encode(challenge_bytes));
    let vrf_vkey = read_vrf_key(pool_vrf_vkey)?;
    if vrf_vkey.key_type != "VrfVerificationKey_PraosVRF" {
        return Err("Pool VRF Vkey must be of type: VrfVerificationKey_PraosVRF".into());
    }
    let vrf_public_key_bytes = vrf_vkey
        .key
        .get(..32)
        .and_then(|bytes| <&[u8; 32]>::try_from(bytes).ok())
        .ok_or("Invalid VRF public key length")?;
    // Verify that the vkey the client supplied is the same as the one on-chain.
    let vkey_hash_verify = hex::encode(Hasher::<256>::hash(vrf_public_key_bytes));
    debug!("vkey_hash_verify: {}", &vkey_hash_verify);
    if pool_vrf_vkey_hash != vkey_hash_verify {
        return Err(format!(
            "Hash of pool-vrf-vkey({vkey_hash_verify}) did not match supplied pool-vrf-vkey-hash({pool_vrf_vkey_hash})"
        )
        .into());
    }
    let signature_bytes = hex::decode(signature)?;
    let signature_slice: [u8; PROOF_SIZE] = signature_bytes
        .as_slice()
        .try_into()
        .map_err(|_| "Invalid signature length")?;
    let vrf_public_key = PublicKey03::from_bytes(vrf_public_key_bytes);
    let vrf_proof =
        VrfProof03::from_bytes(&signature_slice).map_err(|error| format!("VRF proof decoding failed: {error:?}"))?;
    let signature_hash = Hash::<64>::from(vrf_proof.proof_to_hash());
    debug!("signature_hash: {}", hex::encode(signature_hash));
    let verification = vrf_proof
        .verify(&vrf_public_key, challenge_bytes.as_ref())
        .map_err(|error| format!("VRF proof verification failed: {error:?}"))?;
    let verification = Hash::<64>::from(verification);
    debug!("verification: {}", hex::encode(verification));
    if verification != signature_hash {
        return Err("Signature failed to match!".into());
    }
    crate::write_json(
        &mut stdout().lock(),
        &VerifySuccess {
            status: "ok".to_string(),
        },
    )?;
    Ok(())
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
        let vrf_skey_bytes: [u8; 32] = hex::decode("adb9c97bec60189aa90d01d113e3ef405f03477d82a94f81da926c90cd46a374e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda4381").unwrap().as_slice()[0..32].try_into().unwrap();
        let vrf_skey = SecretKey03::from_bytes(&vrf_skey_bytes);
        let vrf_vkey_bytes: [u8; 32] = hex::decode("e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda4381")
            .unwrap()
            .as_slice()[0..32]
            .try_into()
            .unwrap();
        let vrf_vkey = PublicKey03::from_bytes(&vrf_vkey_bytes);

        let challenge = create_challenge("pooltool.io").unwrap();
        let proof = VrfProof03::generate(&vrf_vkey, &vrf_skey, challenge.as_ref());
        let proof_signature_hash = Hash::<64>::from(proof.proof_to_hash());
        let verification_signature_hash = Hash::<64>::from(proof.verify(&vrf_vkey, challenge.as_ref()).unwrap());

        assert_eq!(proof_signature_hash, verification_signature_hash);
    }
}
