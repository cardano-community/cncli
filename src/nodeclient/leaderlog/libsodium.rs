#[link(name = "sodium", kind = "static")]
extern "C" {
    // int crypto_vrf_ietfdraft03_prove(unsigned char *proof, const unsigned char *sk, const unsigned char *m, unsigned long long mlen);
    fn crypto_vrf_ietfdraft03_prove(proof: *mut u8, sk: *const u8, m: *const u8, mlen: u64) -> i32;

    // int crypto_vrf_ietfdraft03_proof_to_hash(unsigned char *hash, const unsigned char *proof);
    fn crypto_vrf_ietfdraft03_proof_to_hash(hash: *mut u8, proof: *const u8) -> i32;

    // int crypto_vrf_ietfdraft03_verify(unsigned char *output, const unsigned char *pk, const unsigned char *proof, const unsigned char *m, unsigned long long mlen)
    fn crypto_vrf_ietfdraft03_verify(output: *mut u8, pk: *const u8, proof: *const u8, m: *const u8, mlen: u64) -> i32;
}

pub(crate) fn sodium_crypto_vrf_prove(secret_key: &[u8], seed: &[u8]) -> Result<Vec<u8>, String> {
    let mut proof: Vec<u8> = Vec::with_capacity(80);
    unsafe {
        let rc = crypto_vrf_ietfdraft03_prove(
            proof.as_mut_ptr(),
            secret_key.as_ptr(),
            seed.as_ptr(),
            seed.len() as u64,
        );
        if rc != 0 {
            Err(format!(
                "libsodium crypto_vrf_ietfdraft03_prove() failed, returned {rc}, expected 0"
            ))
        } else {
            proof.set_len(80);
            Ok(proof)
        }
    }
}

pub(crate) fn sodium_crypto_vrf_proof_to_hash(proof: &[u8]) -> Result<Vec<u8>, String> {
    let mut hash: Vec<u8> = Vec::with_capacity(64);
    unsafe {
        let rc = crypto_vrf_ietfdraft03_proof_to_hash(hash.as_mut_ptr(), proof.as_ptr());
        if rc != 0 {
            Err(format!(
                "libsodium crypto_vrf_ietfdraft03_proof_to_hash() failed, returned {rc}, expected 0"
            ))
        } else {
            hash.set_len(64);
            Ok(hash)
        }
    }
}

pub(crate) fn sodium_crypto_vrf_verify(public_key: &[u8], signature: &[u8], seed: &[u8]) -> Result<Vec<u8>, String> {
    let mut verification: Vec<u8> = Vec::with_capacity(64);
    unsafe {
        let rc = crypto_vrf_ietfdraft03_verify(
            verification.as_mut_ptr(),
            public_key.as_ptr(),
            signature.as_ptr(),
            seed.as_ptr(),
            seed.len() as u64,
        );
        if rc != 0 {
            Err(format!(
                "libsodium crypto_vrf_ietfdraft03_verify() failed, returned {rc}, expected 0"
            ))
        } else {
            verification.set_len(64);
            Ok(verification)
        }
    }
}

#[test]
fn test_sodium_crypto_vrf() {
    let public_key: Vec<u8> = vec![
        0x1d, 0x70, 0xc4, 0xa7, 0xfb, 0x6d, 0x7f, 0x1e, 0xeb, 0x5d, 0x5a, 0x47, 0x05, 0xae, 0xa8, 0xac, 0x0f, 0xa8, 0x80, 0x84, 0x13, 0x60, 0x97, 0x2c, 0x38, 0x4f, 0x29, 0x9b, 0x57, 0x55, 0xb7, 0x38,
    ];
    let secret_key: Vec<u8> = vec![
        0x28, 0xd4, 0x40, 0x5b, 0x8d, 0x32, 0xd9, 0xe2, 0x4e, 0x54, 0xf4, 0x4d, 0xb2, 0xdb, 0x1a, 0xf3, 0x78, 0x67, 0xb5, 0x85, 0x3b, 0x51, 0xc4, 0x3f, 0x5e, 0xe7, 0x53, 0x98, 0x9a, 0xec, 0xd4, 0x5c, 0x1d, 0x70, 0xc4, 0xa7, 0xfb, 0x6d, 0x7f, 0x1e, 0xeb, 0x5d, 0x5a, 0x47, 0x05, 0xae, 0xa8, 0xac, 0x0f, 0xa8, 0x80, 0x84, 0x13, 0x60, 0x97, 0x2c, 0x38, 0x4f, 0x29, 0x9b, 0x57, 0x55, 0xb7, 0x38,
    ];
    let seed: Vec<u8> = vec![
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16,
    ];
    let signature: Vec<u8> = sodium_crypto_vrf_prove(&secret_key, &seed).unwrap();
    assert_eq!(signature.len(), 80);
    let signature_hex = hex::encode(&signature);
    assert_eq!(signature_hex, "af80b562dbcd73ddab05a918e86a8170ac1e5be5378e436b34bece41c2958616002273fb9bfa1d7b2274b326343bdcb0b44b0a04b11d1c216c26ecb5fdf94c5afa18263a7fd1baec0413a70428807b0b");

    let signature_hash: Vec<u8> = sodium_crypto_vrf_proof_to_hash(&signature).unwrap();
    assert_eq!(signature_hash.len(), 64);
    let signature_hash_hex = hex::encode(&signature_hash);
    assert_eq!(signature_hash_hex, "1df57dbd0bfe52d71d010c0126c80dcfa271878f577712c4ae7124d95c7698372857ea1dbd16e4e4b6424a802465107950fe1f3db12535579fa9816915b9a69e");

    let verification: Vec<u8> = sodium_crypto_vrf_verify(&public_key, &signature, &seed).unwrap();
    assert_eq!(verification.len(), 64);
    let verification_hex = hex::encode(&verification);

    assert_eq!(verification_hex, signature_hash_hex);
}
