use super::{rational::ExactRatio, Error};
use curve25519_dalek::{constants::ED25519_BASEPOINT_POINT, edwards::EdwardsPoint, scalar::Scalar};
use dashu_int::{IBig, UBig};
use pallas_crypto::hash::{Hash, Hasher};
use pallas_math::math::{ExpOrdering, FixedDecimal, FixedPrecision};
use rayon::prelude::*;
use sha2::{Digest, Sha512};
use zeroize::Zeroize;

pub(super) struct LeaderThreshold {
    width: usize,
    maximum: IBig,
    maximum_fixed: FixedDecimal,
    sigma: FixedDecimal,
    c: Option<FixedDecimal>,
    x: Option<FixedDecimal>,
}

impl LeaderThreshold {
    pub(super) fn new(pool_stake: u64, active_stake: u64, f: &ExactRatio, width: usize) -> Result<Self, Error> {
        if !matches!(width, 32 | 64) {
            return Err(Error::Leaderlog("VRF width must be 32 or 64 bytes".to_string()));
        }
        if active_stake == 0
            || pool_stake > active_stake
            || f.numerator == 0
            || f.denominator == 0
            || f.numerator > f.denominator
        {
            return Err(Error::Leaderlog(
                "Invalid leader schedule stake or probability parameters".to_string(),
            ));
        }
        let maximum = IBig::ONE << (8 * width);
        let maximum_fixed = FixedDecimal::from(&maximum);
        let sigma = ExactRatio {
            numerator: pool_stake,
            denominator: active_stake,
        }
        .to_fixed();
        let (c, x) = if f.numerator == f.denominator {
            (None, None)
        } else {
            let c = (FixedDecimal::from(1_u64) - f.to_fixed()).ln();
            let x = -(&sigma * &c);
            (Some(c), Some(x))
        };
        Ok(Self {
            width,
            maximum,
            maximum_fixed,
            sigma,
            c,
            x,
        })
    }

    // The inclusive consensus bound is a special case in checkLeaderNatValue,
    // not an infinite reciprocal or a division by zero.
    fn reciprocal_q(&self, cert_nat: &IBig) -> FixedDecimal {
        if cert_nat == &self.maximum {
            self.maximum_fixed.clone()
        } else {
            let denominator = FixedDecimal::from(&self.maximum - cert_nat);
            &self.maximum_fixed / &denominator
        }
    }

    pub(super) fn is_leader(&self, cert_nat: &IBig) -> Result<bool, Error> {
        if cert_nat < &IBig::ZERO || cert_nat > &self.maximum {
            return Err(Error::Leaderlog(
                "VRF natural is outside its consensus bound".to_string(),
            ));
        }
        match &self.x {
            None => Ok(true),
            Some(x) => Ok(matches!(
                x.exp_cmp(1000, 3, &self.reciprocal_q(cert_nat)).estimation,
                ExpOrdering::LT
            )),
        }
    }
}

// IETF VRF draft-03 (ECVRF-ED25519-SHA512-Elligator2) domain constants.
const VRF03_SUITE: u8 = 0x04;
const VRF03_ONE: u8 = 0x01;
const VRF03_THREE: u8 = 0x03;

struct PreparedVrf {
    scalar: Scalar,
    public: [u8; 32],
}

impl PreparedVrf {
    fn new(key: &[u8]) -> Result<Self, Error> {
        let bytes: &[u8; 32] = key
            .get(..32)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| Error::Leaderlog("VRF signing key must contain at least 32 bytes".to_string()))?;
        let mut digest = Sha512::digest(bytes);
        let mut secret_bytes = [0u8; 32];
        secret_bytes.copy_from_slice(&digest[..32]);
        digest[..].zeroize();
        secret_bytes[0] &= 248;
        secret_bytes[31] &= 127;
        secret_bytes[31] |= 64;
        let scalar = Scalar::from_bits(secret_bytes);
        secret_bytes.zeroize();
        let public = (scalar * ED25519_BASEPOINT_POINT).compress().to_bytes();
        Ok(Self { scalar, public })
    }

    fn evaluate(&self, seed: &[u8; 32]) -> Hash<64> {
        let mut hash_input = [0u8; 66];
        hash_input[0] = VRF03_SUITE;
        hash_input[1] = VRF03_ONE;
        hash_input[2..34].copy_from_slice(&self.public);
        hash_input[34..66].copy_from_slice(seed);
        let h = EdwardsPoint::hash_from_bytes::<Sha512>(&hash_input);
        let gamma_cofac = (self.scalar * h).mul_by_cofactor().compress();

        let mut output_input = [0u8; 34];
        output_input[0] = VRF03_SUITE;
        output_input[1] = VRF03_THREE;
        output_input[2..34].copy_from_slice(gamma_cofac.as_bytes());
        let mut output = [0u8; 64];
        output.copy_from_slice(&Sha512::digest(&output_input));
        Hash::from(output)
    }
}

impl Drop for PreparedVrf {
    fn drop(&mut self) {
        self.scalar.zeroize();
    }
}

// seedL: Blake2b-256 of the big-endian u64 value 1.
const UC_NONCE: [u8; 32] = [
    0x12, 0xdd, 0x0a, 0x6a, 0x7d, 0x0e, 0x22, 0x2a, 0x97, 0x92, 0x6d, 0xa0, 0x3a, 0xdb, 0x5a, 0x77, 0x68, 0xd3, 0x1c,
    0xc7, 0xc5, 0xc2, 0xbd, 0x68, 0x28, 0xe1, 0x4a, 0x7d, 0x25, 0xfa, 0x3a, 0x60,
];

fn mk_input_vrf(slot: u64, nonce: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Hasher::<256>::new();
    hasher.input(&slot.to_be_bytes());
    hasher.input(nonce);
    *hasher.finalize()
}

fn mk_seed(slot: u64, nonce: &[u8; 32]) -> [u8; 32] {
    let mut seed = mk_input_vrf(slot, nonce);
    for (byte, universal) in seed.iter_mut().zip(UC_NONCE) {
        *byte ^= universal;
    }
    seed
}

pub(super) fn calculate_schedule(
    first_slot: u64,
    slot_count: u64,
    mode: &str,
    d: &ExactRatio,
    epoch_nonce: &[u8; 32],
    vrf_key: &[u8],
    threshold: &LeaderThreshold,
) -> Result<Vec<u64>, Error> {
    let width = match mode {
        "tpraos" => 64,
        "praos" | "cpraos" => 32,
        _ => return Err(Error::Leaderlog("Invalid consensus mode".to_string())),
    };
    if threshold.width != width {
        return Err(Error::Leaderlog(
            "Leader threshold width does not match consensus mode".to_string(),
        ));
    }
    if slot_count != 0 && first_slot.checked_add(slot_count - 1).is_none() {
        return Err(Error::Leaderlog(
            "Leader schedule arithmetic exceeds supported slot range".to_string(),
        ));
    }
    let prepared = PreparedVrf::new(vrf_key)?;
    let full_coefficient = threshold.c.is_none();
    if !full_coefficient && threshold.sigma == FixedDecimal::from(0_u64) {
        return Ok(Vec::new());
    }

    const CHUNK_SLOTS: u64 = 1024;
    let chunks = slot_count.div_ceil(CHUNK_SLOTS);
    let workers = rayon::current_num_threads() as u64;
    let mut window_start = 0;
    let mut slots = Vec::new();
    while window_start < chunks {
        let window_len = (chunks - window_start).min(workers) as usize;
        // This usize range is indexed: both collection and append preserve order.
        let window = (0..window_len)
            .into_par_iter()
            .map(|index| {
                let offset_start = (window_start + index as u64) * CHUNK_SLOTS;
                let offset_end = offset_start + (slot_count - offset_start).min(CHUNK_SLOTS);
                let mut elected = Vec::new();
                for offset in offset_start..offset_end {
                    if mode == "tpraos" && super::is_overlay_slot(offset, d) {
                        continue;
                    }
                    let slot = first_slot + offset;
                    if full_coefficient {
                        elected.push(slot);
                        continue;
                    }
                    let seed = if mode == "tpraos" {
                        mk_seed(slot, epoch_nonce)
                    } else {
                        mk_input_vrf(slot, epoch_nonce)
                    };
                    let raw = prepared.evaluate(&seed);
                    let natural = if mode == "tpraos" {
                        IBig::from(UBig::from_be_bytes(raw.as_slice()))
                    } else {
                        let mut hasher = Hasher::<256>::new();
                        hasher.input(b"L");
                        hasher.input(raw.as_slice());
                        IBig::from(UBig::from_be_bytes(hasher.finalize().as_slice()))
                    };
                    if threshold.is_leader(&natural)? {
                        elected.push(slot);
                    }
                }
                Ok(elected)
            })
            .collect::<Result<Vec<Vec<u64>>, Error>>()?;
        for chunk in window {
            slots.extend(chunk);
        }
        window_start += window_len as u64;
    }
    Ok(slots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::str::FromStr;
    use vrf_dalek::vrf03::{PublicKey03, SecretKey03, VrfProof03};

    #[test]
    fn prepared_vrf_preserves_compatibility_and_short_key_errors() {
        assert!(matches!(PreparedVrf::new(&[0; 31]), Err(Error::Leaderlog(_))));
        let prepared = PreparedVrf::new(&[0; 32]).unwrap();
        assert_eq!(
            hex::encode(prepared.evaluate(&[0; 32])),
            "2f9e929479cb32192477b6908a57e6ad748f13a96152ebd67cb6345b2c66b5377ebe42828160bd98d29a710f4b4efe3d7a6a1ed49ae9433f5b06c172c11d04a0"
        );
    }

    #[test]
    fn prepared_vrf_matches_ietf_vrf03_vectors_and_slot_seeds() {
        for short_len in [0, 1, 16, 31] {
            let err = PreparedVrf::new(&vec![0u8; short_len]).err().unwrap();
            assert_eq!(
                err.to_string(),
                "Leaderlog error: VRF signing key must contain at least 32 bytes"
            );
        }

        // Secret keys, public keys, and alpha prefixes from IETF draft-irtf-cfrg-vrf-03
        // Section A.3 (ECVRF-ED25519-SHA512-Elligator2) and libsodium vrf03 vectors,
        // plus Cardano extended 64-byte operational signing keys.
        let vectors = [
            (
                "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
                "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
                "",
            ),
            (
                "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
                "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
                "72",
            ),
            (
                "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
                "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
                "af82",
            ),
            (
                "ac21b0303ed8b5123ae3bf13f2c627dc9b9c2e04cf1c732364cfe472a5f145c2",
                "5b14fde6db37a5302f0150a73f6ada5a05a810a24c656482cbdeb51020e92fe7",
                "230dd4c855c133c5b3c24a72af9bbbc48205984ea4f2045feaac17fe1af9",
            ),
            (
                "adb9c97bec60189aa90d01d113e3ef405f03477d82a94f81da926c90cd46a374e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda4381",
                "e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda4381",
                "5ac2fd05d52fbcb6",
            ),
        ];

        let nonces = [[0u8; 32], [0xffu8; 32], UC_NONCE];

        for (sk_hex, pk_hex, alpha_prefix_hex) in vectors {
            let sk_raw = hex::decode(sk_hex).unwrap();
            let expected_pk: [u8; 32] = hex::decode(pk_hex).unwrap().try_into().unwrap();

            let prepared = PreparedVrf::new(&sk_raw).unwrap();
            assert_eq!(prepared.public, expected_pk);

            let sk_seed: [u8; 32] = sk_raw[..32].try_into().unwrap();
            let sk03 = SecretKey03::from_bytes(&sk_seed);
            let pk03 = PublicKey03::from(&sk03);
            assert_eq!(prepared.public, pk03.to_bytes());

            let mut padded_alpha = [0u8; 32];
            let alpha_bytes = hex::decode(alpha_prefix_hex).unwrap();
            padded_alpha[..alpha_bytes.len()].copy_from_slice(&alpha_bytes);

            let mut seeds = vec![[0u8; 32], [0xffu8; 32], padded_alpha];
            for nonce in &nonces {
                for slot in [0_u64, 1, 42, 431_999, 10_000_000, 138_240_000, u64::MAX] {
                    seeds.push(mk_seed(slot, nonce));
                    seeds.push(mk_input_vrf(slot, nonce));
                }
            }

            for seed in &seeds {
                let proof = VrfProof03::generate(&pk03, &sk03, seed);
                let expected = Hash::<64>::from(proof.proof_to_hash());
                let verified = Hash::<64>::from(proof.verify(&pk03, seed).unwrap());
                assert_eq!(expected, verified);
                assert_eq!(prepared.evaluate(seed), expected);
            }
        }

        let mut uninit = std::mem::MaybeUninit::new(PreparedVrf::new(&[0x42; 32]).unwrap());
        unsafe {
            assert_ne!(uninit.assume_init_ref().scalar.as_bytes(), &[0u8; 32]);
            std::ptr::drop_in_place(uninit.as_mut_ptr());
            let scalar_ptr = std::ptr::addr_of!((*uninit.as_ptr()).scalar).cast::<u8>();
            let wiped: [u8; 32] = std::array::from_fn(|i| std::ptr::read_volatile(scalar_ptr.add(i)));
            assert_eq!(wiped, [0u8; 32]);
        }
    }

    fn ratio(numerator: u64, denominator: u64) -> ExactRatio {
        ExactRatio::from_parts(IBig::from(numerator), IBig::from(denominator)).unwrap()
    }

    fn raw(text: &str) -> FixedDecimal {
        FixedDecimal::from_str(text, 34).unwrap()
    }

    fn assert_fields(threshold: &LeaderThreshold, cert: &IBig, fields: &[&str]) {
        assert_eq!(fields.len(), 5);
        assert_eq!(threshold.is_leader(cert).unwrap(), fields[0].parse::<bool>().unwrap());
        assert_eq!(threshold.sigma, raw(fields[1]));
        for (actual, expected) in [(&threshold.c, fields[2]), (&threshold.x, fields[4])] {
            if expected == "none" {
                assert!(actual.is_none());
            } else {
                assert_eq!(actual.as_ref().unwrap(), &raw(expected));
            }
        }
        assert_eq!(threshold.reciprocal_q(cert), raw(fields[3]));
    }

    #[test]
    fn node_signed_floor_primitives() {
        for (index, line) in include_str!("../../../tests/data/leaderlog/node-primitives.tsv")
            .lines()
            .enumerate()
        {
            let fields: Vec<_> = line.split_whitespace().collect();
            let result = match fields[0] {
                "multiply" => raw(fields[1]) * raw(fields[2]),
                "divide" => raw(fields[1]) / raw(fields[2]),
                "ratio" => {
                    FixedDecimal::from(IBig::from_str(fields[1]).unwrap())
                        / FixedDecimal::from(IBig::from_str(fields[2]).unwrap())
                }
                operation => panic!("unknown node primitive: {operation}"),
            };
            assert_eq!(result, raw(fields[3]), "node primitive row {index}");
        }
    }

    #[test]
    fn node_comparator_boundaries_and_termination() {
        for (index, line) in include_str!("../../../tests/data/leaderlog/node-comparisons.tsv")
            .lines()
            .enumerate()
        {
            let fields: Vec<_> = line.split_whitespace().collect();
            let result = raw(fields[1]).exp_cmp(1000, 3, &raw(fields[0]));
            let expected = match fields[2] {
                "BELOW" => ExpOrdering::LT,
                "ABOVE" => ExpOrdering::GT,
                "MAX_REACHED" => ExpOrdering::UNKNOWN,
                classification => panic!("unknown node classification: {classification}"),
            };
            assert_eq!(result.estimation, expected, "node comparison row {index}");
            assert_eq!(
                result.iterations,
                fields[3].parse::<u64>().unwrap(),
                "node comparison iterations row {index}"
            );
        }
    }

    #[test]
    fn every_node_value_field() {
        for (index, line) in include_str!("../../../tests/data/leaderlog/node-vectors.tsv")
            .lines()
            .enumerate()
        {
            let fields: Vec<_> = line.split_whitespace().collect();
            assert_eq!(fields.len(), 11, "row {index}");
            let threshold = LeaderThreshold::new(
                fields[2].parse().unwrap(),
                fields[3].parse().unwrap(),
                &ratio(fields[4].parse().unwrap(), fields[5].parse().unwrap()),
                fields[0].parse().unwrap(),
            )
            .unwrap();
            assert_fields(&threshold, &IBig::from_str(fields[1]).unwrap(), &fields[6..]);
        }
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Schedule {
        mode: String,
        first_slot: u64,
        slot_count: u64,
        pool_stake: String,
        active_stake: String,
        f_numerator: String,
        f_denominator: String,
        d_numerator: String,
        d_denominator: String,
        nonce_hex: String,
        vrf_skey_hex: String,
        assigned_slots: Vec<u64>,
    }

    fn replay(case: &Schedule, mode: &str, d: &ExactRatio) -> Vec<u64> {
        let threshold = LeaderThreshold::new(
            case.pool_stake.parse().unwrap(),
            case.active_stake.parse().unwrap(),
            &ratio(case.f_numerator.parse().unwrap(), case.f_denominator.parse().unwrap()),
            if mode == "tpraos" { 64 } else { 32 },
        )
        .unwrap();
        calculate_schedule(
            case.first_slot,
            case.slot_count,
            mode,
            d,
            &hex::decode(&case.nonce_hex).unwrap().try_into().unwrap(),
            &hex::decode(&case.vrf_skey_hex).unwrap(),
            &threshold,
        )
        .unwrap()
    }

    #[test]
    fn every_node_schedule_including_cpraos() {
        #[derive(Deserialize)]
        struct Schedules {
            cases: Vec<Schedule>,
        }
        let fixtures: Schedules =
            serde_json::from_str(include_str!("../../../tests/data/leaderlog/node-schedules.json")).unwrap();
        for case in fixtures.cases {
            let d = ratio(case.d_numerator.parse().unwrap(), case.d_denominator.parse().unwrap());
            assert_eq!(
                replay(&case, &case.mode, &d),
                case.assigned_slots,
                "{} {}",
                case.mode,
                case.first_slot
            );
            if case.mode == "praos" {
                assert_eq!(replay(&case, "cpraos", &d), case.assigned_slots);
            }
        }
    }

    #[test]
    fn schedule_chunk_boundaries_preserve_oracle_prefixes() {
        #[derive(Deserialize)]
        struct Schedules {
            cases: Vec<Schedule>,
        }
        let fixtures: Schedules =
            serde_json::from_str(include_str!("../../../tests/data/leaderlog/node-schedules.json")).unwrap();
        for case in fixtures.cases.into_iter().filter(|case| {
            case.slot_count == 3600
                && (case.f_numerator == "12345678901234567" || case.f_numerator == case.f_denominator)
        }) {
            let threshold = LeaderThreshold::new(
                case.pool_stake.parse().unwrap(),
                case.active_stake.parse().unwrap(),
                &ratio(case.f_numerator.parse().unwrap(), case.f_denominator.parse().unwrap()),
                if case.mode == "tpraos" { 64 } else { 32 },
            )
            .unwrap();
            let d = ratio(case.d_numerator.parse().unwrap(), case.d_denominator.parse().unwrap());
            let nonce = hex::decode(&case.nonce_hex).unwrap().try_into().unwrap();
            let key = hex::decode(&case.vrf_skey_hex).unwrap();
            for count in [0, 1, 1023, 1024, 1025, 2049, 3073] {
                let expected: Vec<_> = case
                    .assigned_slots
                    .iter()
                    .copied()
                    .take_while(|slot| *slot < case.first_slot + count)
                    .collect();
                assert_eq!(
                    calculate_schedule(case.first_slot, count, &case.mode, &d, &nonce, &key, &threshold).unwrap(),
                    expected,
                    "{} prefix {count}",
                    case.mode
                );
                if case.mode == "praos" {
                    assert_eq!(
                        calculate_schedule(case.first_slot, count, "cpraos", &d, &nonce, &key, &threshold).unwrap(),
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn inclusive_bounds_and_probability_decisions() {
        for width in [32, 64] {
            let full = LeaderThreshold::new(0, 1, &ratio(1, 1), width).unwrap();
            assert!(full.is_leader(&IBig::ZERO).unwrap());
            assert!(full.is_leader(&full.maximum).unwrap());
            assert_eq!(full.reciprocal_q(&full.maximum), full.maximum_fixed);
            assert_eq!(full.reciprocal_q(&(&full.maximum - IBig::ONE)), full.maximum_fixed);
            assert!(full.is_leader(&(-IBig::ONE)).is_err());
            assert!(full.is_leader(&(&full.maximum + IBig::ONE)).is_err());
            let zero = LeaderThreshold::new(0, 1, &ratio(1, 20), width).unwrap();
            for cert in [IBig::ZERO, IBig::ONE, &zero.maximum - IBig::ONE, zero.maximum.clone()] {
                assert!(!zero.is_leader(&cert).unwrap());
            }
        }
        for (pool, active, f, width) in [
            (0, 0, ratio(1, 2), 32),
            (2, 1, ratio(1, 2), 32),
            (1, 1, ratio(0, 1), 32),
            (1, 1, ratio(1, 2), 33),
        ] {
            assert!(LeaderThreshold::new(pool, active, &f, width).is_err());
        }
        let tiny = LeaderThreshold::new(1, 2, &ratio(1, 100000), 32).unwrap();
        let boundary =
            IBig::from_str("578961893594933494372516232843189694053309489864580603568403404332973678").unwrap();
        assert!(tiny.is_leader(&boundary).unwrap());
        assert!(!tiny.is_leader(&(boundary + IBig::ONE)).unwrap());
        let mut exhausted = LeaderThreshold::new(1, 1, &ratio(1, 2), 32).unwrap();
        exhausted.x = Some(FixedDecimal::from(1_000_000_u64));
        assert_eq!(
            exhausted
                .x
                .as_ref()
                .unwrap()
                .exp_cmp(1000, 3, &FixedDecimal::from(1_u64))
                .estimation,
            ExpOrdering::UNKNOWN
        );
        assert!(!exhausted.is_leader(&IBig::ZERO).unwrap());
    }

    #[test]
    fn schedule_validation_and_exact_overlay() {
        let full = LeaderThreshold::new(0, 1, &ratio(1, 1), 64).unwrap();
        let key = [0; 32];
        let nonce = [0; 32];
        let run = |first, count, mode, d: &ExactRatio, key: &[u8]| {
            calculate_schedule(first, count, mode, d, &nonce, key, &full)
        };
        assert!(run(0, 0, "tpraos", &ratio(1, 1), &[0; 31]).is_err());
        assert!(run(0, 1, "tpraos", &ratio(1, 1), &[0; 31]).is_err());
        assert!(run(0, 0, "bad", &ratio(0, 1), &key).is_err());
        assert!(run(0, 0, "praos", &ratio(0, 1), &key).is_err());
        assert!(run(u64::MAX, 2, "tpraos", &ratio(1, 1), &key).is_err());
        assert!(run(u64::MAX, 0, "tpraos", &ratio(0, 1), &key).unwrap().is_empty());
        assert_eq!(run(u64::MAX, 1, "tpraos", &ratio(0, 1), &key).unwrap(), vec![u64::MAX]);
        assert!(run(0, 10, "tpraos", &ratio(1, 1), &key).unwrap().is_empty());
        let slots = run(0, 2501, "tpraos", &ratio(1, 2500), &key).unwrap();
        assert!(!slots.contains(&0));
        assert!(slots.contains(&2499));
        assert!(!slots.contains(&2500));
        let praos = LeaderThreshold::new(1, 3, &ratio(1, 3), 32).unwrap();
        let expected = calculate_schedule(10_000_000, 60, "praos", &ratio(0, 1), &nonce, &key, &praos).unwrap();
        for mode in ["praos", "cpraos"] {
            for d in [ratio(1, 3), ratio(1, 1)] {
                assert_eq!(
                    calculate_schedule(10_000_000, 60, mode, &d, &nonce, &key, &praos).unwrap(),
                    expected
                );
            }
        }
    }

    // Drain stdout/stderr while the separate writer feeds stdin. Large value
    // batches otherwise deadlock when both pipe buffers fill.
    fn oracle(executable: &std::path::Path, args: &[String], input: String) -> String {
        let mut child = Command::new(executable)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start pinned oracle");
        let mut stdin = child.stdin.take().unwrap();
        let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
        let output = child.wait_with_output().expect("drain pinned oracle");
        writer
            .join()
            .expect("oracle writer panicked")
            .expect("write oracle input");
        assert!(
            output.status.success(),
            "oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.is_empty() || text.ends_with('\n'), "truncated oracle row");
        text
    }

    struct Random(u64);
    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn parameters(&mut self, width: usize) -> (u64, u64, ExactRatio, LeaderThreshold) {
            let active = self.next().max(1);
            let pool = self.next() % active;
            let denominator = self.next().max(1);
            let numerator = 1 + self.next() % denominator;
            let f = ratio(numerator, denominator);
            let threshold = LeaderThreshold::new(pool, active, &f, width).unwrap();
            (pool, active, f, threshold)
        }
    }

    fn value_query(width: usize, cert: &IBig, pool: u64, active: u64, f: &ExactRatio) -> String {
        format!("{width} {cert} {pool} {active} {} {}\n", f.numerator, f.denominator)
    }

    fn compare_batch(executable: &std::path::Path, input: String, rows: &[(LeaderThreshold, IBig)]) -> Vec<bool> {
        let text = oracle(executable, &["values".to_string()], input);
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), rows.len(), "oracle value row count");
        lines
            .iter()
            .zip(rows)
            .map(|(line, (threshold, cert))| {
                let fields: Vec<_> = line.split_whitespace().collect();
                assert_fields(threshold, cert, &fields);
                fields[0].parse().unwrap()
            })
            .collect()
    }

    #[test]
    #[ignore = "requires NODE_ORACLE pointing to the pinned real ledger oracle"]
    fn live_node_differential() {
        let executable = std::path::PathBuf::from(std::env::var_os("NODE_ORACLE").expect("NODE_ORACLE is required"));
        let mut random = Random(0x4e4f444550415249);
        let mut input = String::new();
        let mut rows = Vec::with_capacity(10_000);
        for index in 0..10_000 {
            let width = if index % 2 == 0 { 32 } else { 64 };
            let (pool, active, f, threshold) = random.parameters(width);
            let cert = match index % 5 {
                0 => IBig::ZERO,
                1 => IBig::ONE,
                2 => &threshold.maximum - IBig::ONE,
                3 => threshold.maximum.clone(),
                _ => {
                    let mut bytes = vec![0; width];
                    for chunk in bytes.as_chunks_mut::<8>().0 {
                        chunk.copy_from_slice(&random.next().to_be_bytes());
                    }
                    IBig::from(UBig::from_be_bytes(&bytes))
                }
            };
            input.push_str(&value_query(width, &cert, pool, active, &f));
            rows.push((threshold, cert));
        }
        compare_batch(&executable, input, &rows);

        // Batched binary search: oracle booleans drive every bound update,
        // while production is compared at every queried midpoint and neighbor.
        let parameters: Vec<_> = (0..32)
            .map(|index| {
                let width = if index % 2 == 0 { 32 } else { 64 };
                let (pool, active, f, threshold) = random.parameters(width);
                (width, pool, active, f, threshold)
            })
            .collect();
        let mut bounds: Vec<_> = parameters.iter().map(|p| (IBig::ZERO, p.4.maximum.clone())).collect();
        // Establish whether any natural is elected before searching.
        let mut input = String::new();
        let mut rows = Vec::new();
        for (width, pool, active, f, _) in &parameters {
            input.push_str(&value_query(*width, &IBig::ZERO, *pool, *active, f));
            rows.push((LeaderThreshold::new(*pool, *active, f, *width).unwrap(), IBig::ZERO));
        }
        let any = compare_batch(&executable, input, &rows);
        loop {
            let mut input = String::new();
            let mut rows = Vec::new();
            let mut indices = Vec::new();
            for (index, ((width, pool, active, f, _), (low, high))) in parameters.iter().zip(&bounds).enumerate() {
                if !any[index] || high - low <= IBig::ONE {
                    continue;
                }
                let middle = (low + high) / IBig::from(2);
                input.push_str(&value_query(*width, &middle, *pool, *active, f));
                rows.push((LeaderThreshold::new(*pool, *active, f, *width).unwrap(), middle));
                indices.push(index);
            }
            if rows.is_empty() {
                break;
            }
            for ((index, elected), (_, middle)) in indices
                .into_iter()
                .zip(compare_batch(&executable, input, &rows))
                .zip(rows)
            {
                if elected {
                    bounds[index].0 = middle;
                } else {
                    bounds[index].1 = middle;
                }
            }
        }
        let mut input = String::new();
        let mut rows = Vec::new();
        let mut expected = Vec::new();
        for (index, (width, pool, active, f, threshold)) in parameters.iter().enumerate() {
            let (low, high) = &bounds[index];
            let checks = if any[index] {
                vec![(low.clone(), true), (high.clone(), f.numerator == f.denominator)]
            } else {
                vec![(IBig::ZERO, false), (threshold.maximum.clone(), false)]
            };
            for (cert, elected) in checks {
                input.push_str(&value_query(*width, &cert, *pool, *active, f));
                rows.push((LeaderThreshold::new(*pool, *active, f, *width).unwrap(), cert));
                expected.push(elected);
            }
        }
        assert_eq!(compare_batch(&executable, input, &rows), expected);

        for index in 0..32 {
            let mode = if index % 2 == 0 { "tpraos" } else { "praos" };
            let width = if mode == "tpraos" { 64 } else { 32 };
            let (pool, active, f, threshold) = random.parameters(width);
            let denominator = random.next().max(1);
            let d = ratio(random.next() % denominator, denominator);
            let nonce = [0; 32];
            let args = vec![
                "schedule".to_string(),
                mode.to_string(),
                "10000000".to_string(),
                "60".to_string(),
                pool.to_string(),
                active.to_string(),
                f.numerator.to_string(),
                f.denominator.to_string(),
                d.numerator.to_string(),
                d.denominator.to_string(),
                hex::encode(nonce),
            ];
            let text = oracle(&executable, &args, String::new());
            let mut lines = text.lines();
            let key = hex::decode(lines.next().expect("oracle schedule key")).unwrap();
            assert_eq!(key.len(), 64);
            assert!(
                key[..32].iter().all(|byte| *byte == 0),
                "oracle public fixture signing seed"
            );
            let slots: Vec<u64> = lines.map(|line| line.parse().expect("oracle slot row")).collect();
            assert!(slots.windows(2).all(|pair| pair[0] < pair[1]));
            assert!(slots.iter().all(|slot| (10_000_000..10_000_060).contains(slot)));
            assert_eq!(
                calculate_schedule(10_000_000, 60, mode, &d, &nonce, &key, &threshold).unwrap(),
                slots
            );
            if mode == "praos" {
                assert_eq!(
                    calculate_schedule(10_000_000, 60, "cpraos", &d, &nonce, &key, &threshold).unwrap(),
                    slots
                );
            }
        }
    }
}
