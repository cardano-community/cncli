use std::fmt::Display;
use std::fs::File;
use std::io::{stdout, BufReader};
use std::path::Path;
use std::str::FromStr;

use crate::nodeclient::blockstore;
use crate::nodeclient::blockstore::redb::{is_redb_database, RedbBlockStore};
use crate::nodeclient::blockstore::sqlite::SqLiteBlockStore;
use crate::nodeclient::blockstore::BlockStore;
use crate::nodeclient::leaderlog::deserialize::cbor_hex;
use crate::nodeclient::leaderlog::ledgerstate::calculate_ledger_state_sigma_d_and_extra_entropy;
use crate::{LedgerSet, PooltoolConfig};
use chrono::{DateTime, NaiveDateTime, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use itertools::sorted;
use pallas_crypto::hash::{Hash, Hasher};
use pallas_crypto::nonce::generate_epoch_nonce;
use pallas_crypto::vrf::{VrfSecretKey, VRF_SECRET_KEY_SIZE};
use pallas_math::math::{ExpOrdering, FixedDecimal, FixedPrecision, DEFAULT_PRECISION};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_aux::prelude::deserialize_number_from_string;
use thiserror::Error;
use tracing::{debug, error, info, span, trace, Level};

mod deserialize;
mod ledgerstate;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Rusqlite error: {0}")]
    Rusqlite(#[from] rusqlite::Error),

    #[error("FromHex error: {0}")]
    FromHex(#[from] hex::FromHexError),

    #[error("PallasMath error: {0}")]
    PallasMath(#[from] pallas_math::math::Error),

    #[error("Leaderlog error: {0}")]
    Leaderlog(String),

    #[error("Blockstore error: {0}")]
    Blockstore(#[from] blockstore::Error),

    #[error("Redb error: {0}")]
    Redb(#[from] blockstore::redb::Error),

    #[error("Sqlite error: {0}")]
    Sqlite(#[from] blockstore::sqlite::Error),

    #[error("ParseFloat error: {0}")]
    ParseFloat(#[from] std::num::ParseFloatError),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LeaderLogError {
    status: String,
    error_message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ByronGenesis {
    start_time: u64,
    protocol_consts: ProtocolConsts,
    block_version_data: BlockVersionData,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProtocolConsts {
    k: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlockVersionData {
    #[serde(deserialize_with = "deserialize_number_from_string")]
    slot_duration: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShelleyGenesis {
    active_slots_coeff: f64,
    network_magic: u32,
    slot_length: u64,
    epoch_length: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct VrfKey {
    #[serde(rename(deserialize = "type"))]
    pub(crate) key_type: String,
    #[serde(deserialize_with = "cbor_hex")]
    #[serde(rename(deserialize = "cborHex"))]
    pub(crate) key: Vec<u8>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LeaderLog {
    status: String,
    epoch: u64,
    epoch_nonce: String,
    consensus: String,
    epoch_slots: u64,
    epoch_slots_ideal: f64,
    max_performance: f64,
    pool_id: String,
    sigma: f64,
    active_stake: u64,
    total_active_stake: u64,
    d: f64,
    f: f64,
    assigned_slots: Vec<Slot>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Slot {
    no: u64,
    slot: u64,
    slot_in_epoch: u64,
    at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PooltoolSendSlots {
    api_key: String,
    pool_id: String,
    epoch: u64,
    slot_qty: u64,
    hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    override_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prev_slots: Option<String>,
}

fn read_byron_genesis(byron_genesis: &Path) -> Result<ByronGenesis, Error> {
    let buf = BufReader::new(File::open(byron_genesis)?);
    Ok(serde_json::from_reader(buf)?)
}

fn read_shelley_genesis(shelley_genesis: &Path) -> Result<ShelleyGenesis, Error> {
    let buf = BufReader::new(File::open(shelley_genesis)?);
    Ok(serde_json::from_reader(buf)?)
}

pub(crate) fn read_vrf_key(vrf_key_path: &Path) -> Result<VrfKey, Error> {
    let buf = BufReader::new(File::open(vrf_key_path)?);
    Ok(serde_json::from_reader(buf)?)
}

fn guess_shelley_transition_epoch(network_magic: u32) -> u64 {
    match network_magic {
        764824073 => {
            // mainnet
            208
        }
        1097911063 => {
            //testnet / ghostnet
            74
        }
        141 => {
            //guild
            2
        }
        1 => {
            //preprod
            4
        }
        2 => {
            //preview testnet
            0
        }
        4 => {
            //sancho
            0
        }
        _ => {
            // alonzo, fallback
            1
        }
    }
}

/// Calculate the first slot of the epoch and the epoch number for the given slot
fn get_first_slot_of_epoch(
    byron: &ByronGenesis,
    shelley: &ShelleyGenesis,
    current_slot: u64,
    shelley_transition_epoch: u64,
) -> (u64, u64) {
    let byron_epoch_length = 10 * byron.protocol_consts.k;
    let byron_slots = byron_epoch_length * shelley_transition_epoch;
    let shelley_slots = current_slot - byron_slots;
    let shelley_slot_in_epoch = shelley_slots % shelley.epoch_length;
    let first_slot_of_epoch = current_slot - shelley_slot_in_epoch;
    let epoch = (shelley_slots / shelley.epoch_length) + shelley_transition_epoch;

    (epoch, first_slot_of_epoch)
}

fn slot_to_naivedatetime(
    byron: &ByronGenesis,
    shelley: &ShelleyGenesis,
    slot: u64,
    shelley_transition_epoch: u64,
) -> NaiveDateTime {
    let network_start_time = DateTime::from_timestamp(byron.start_time as i64, 0)
        .unwrap()
        .naive_utc();
    let byron_epoch_length = 10 * byron.protocol_consts.k;
    let byron_slots = byron_epoch_length * shelley_transition_epoch;
    let shelley_slots = slot - byron_slots;

    let byron_secs = (byron.block_version_data.slot_duration * byron_slots) / 1000;
    let shelley_secs = shelley_slots * shelley.slot_length;

    network_start_time
        + TimeDelta::try_seconds(byron_secs as i64).unwrap()
        + TimeDelta::try_seconds(shelley_secs as i64).unwrap()
}

fn slot_to_timestamp(
    byron: &ByronGenesis,
    shelley: &ShelleyGenesis,
    slot: u64,
    tz: &Tz,
    shelley_transition_epoch: u64,
) -> String {
    let slot_time = slot_to_naivedatetime(byron, shelley, slot, shelley_transition_epoch);
    tz.from_utc_datetime(&slot_time).to_rfc3339()
}

pub fn is_overlay_slot(first_slot_of_epoch: &u64, current_slot: &u64, d: &f64) -> bool {
    let d = FixedDecimal::from((*d * 1000.0).round() as u64) / FixedDecimal::from(1000u64);
    trace!("d: {}", &d);
    let diff_slot: FixedDecimal = FixedDecimal::from(current_slot - first_slot_of_epoch);
    trace!("diff_slot: {}", &diff_slot);
    let diff_slot_inc: FixedDecimal = &diff_slot + &FixedDecimal::from(1u64);
    trace!("diff_slot_inc: {}", &diff_slot_inc);
    let left = (&d * &diff_slot).ceil();
    trace!("left: {}", &left);
    let right = (&d * &diff_slot_inc).ceil();
    trace!("right: {}", &right);
    trace!("is_overlay_slot: {} - {}", current_slot, left < right);
    left < right
}

//
// The universal constant nonce. The blake2b hash of the 8 byte long value of 1
// 12dd0a6a7d0e222a97926da03adb5a7768d31cc7c5c2bd6828e14a7d25fa3a60
// Sometimes called seedL in the haskell code
//
const UC_NONCE: [u8; 32] = [
    0x12, 0xdd, 0x0a, 0x6a, 0x7d, 0x0e, 0x22, 0x2a, 0x97, 0x92, 0x6d, 0xa0, 0x3a, 0xdb, 0x5a, 0x77, 0x68, 0xd3, 0x1c,
    0xc7, 0xc5, 0xc2, 0xbd, 0x68, 0x28, 0xe1, 0x4a, 0x7d, 0x25, 0xfa, 0x3a, 0x60,
];

fn mk_seed(slot: u64, eta0: &[u8]) -> Vec<u8> {
    trace!("mk_seed() start slot {}", slot);
    let mut hasher = Hasher::<256>::new();
    hasher.input(&slot.to_be_bytes());
    hasher.input(eta0);
    let slot_to_seed = hasher.finalize();

    UC_NONCE
        .iter()
        .enumerate()
        .map(|(i, byte)| byte ^ slot_to_seed[i])
        .collect()
}

fn mk_input_vrf(slot: u64, eta0: &[u8]) -> Vec<u8> {
    trace!("mk_seed() start slot {}", slot);
    let mut hasher = Hasher::<256>::new();
    hasher.input(&slot.to_be_bytes());
    hasher.input(eta0);
    hasher.finalize().to_vec()
}

fn vrf_eval_certified(seed: &[u8], pool_vrf_skey: &[u8]) -> Result<Hash<64>, Error> {
    let vrf_skey: [u8; VRF_SECRET_KEY_SIZE] = pool_vrf_skey[..VRF_SECRET_KEY_SIZE].try_into().expect("Infallible");
    let vrf_skey: VrfSecretKey = VrfSecretKey::from(&vrf_skey);
    let certified_proof = vrf_skey.prove(seed);
    let certified_proof_hash = certified_proof.to_hash();
    trace!("certified_proof_hash: {}", hex::encode(certified_proof_hash));
    Ok(certified_proof_hash)
}

fn vrf_leader_value(raw_vrf: &[u8]) -> Result<FixedDecimal, Error> {
    let mut hasher = Hasher::<256>::new();
    hasher.input(vec![0x4C_u8].as_slice()); // "L"
    hasher.input(raw_vrf);
    Ok(FixedDecimal::from(hasher.finalize().as_slice()))
}

// Determine if our pool is a slot leader for this given slot
// @param slot The slot to check
// @param sigma The controlled stake proportion for the pool
// @param eta0 The epoch nonce value
// @param pool_vrf_skey The vrf signing key for the pool
// @param cert_nat_max The value 2^256
// @param c ln(1-activeSlotsCoeff) - usually ln(1-0.05)
fn is_slot_leader_praos(
    slot: u64,
    sigma: &FixedDecimal,
    eta0: &[u8],
    pool_vrf_skey: &[u8],
    cert_nat_max: &FixedDecimal,
    c: &FixedDecimal,
) -> Result<bool, Error> {
    let seed: Vec<u8> = mk_input_vrf(slot, eta0);
    let cert_nat: Hash<64> = vrf_eval_certified(&seed, pool_vrf_skey)?;
    let cert_leader_vrf: FixedDecimal = vrf_leader_value(cert_nat.as_slice())?;
    let denominator = cert_nat_max - &cert_leader_vrf;
    let recip_q: FixedDecimal = cert_nat_max / &denominator;
    let x: FixedDecimal = -(sigma * c);
    let ordering = x.exp_cmp(1000, 3, &recip_q);

    let span = span!(Level::TRACE, "is_slot_leader_praos");
    let _enter = span.enter();
    trace!("is_slot_leader_praos: {}", slot);
    trace!("seed: {}", hex::encode(&seed));
    trace!("cert_nat: {}", &cert_nat);
    trace!("cert_leader_vrf: {}", &cert_leader_vrf);
    trace!("recip_q: {}", &recip_q);
    trace!("c: {}", c);
    trace!("x: {}", &x);

    Ok(ordering.estimation == ExpOrdering::LT)
}

// Determine if our pool is a slot leader for this given slot
// @param slot The slot to check
// @param sigma The controlled stake proportion for the pool
// @param eta0 The epoch nonce value
// @param pool_vrf_skey The vrf signing key for the pool
// @param cert_nat_max The value 2^512
// @param c 1-activeSlotsCoeff - usually 0.95
fn is_slot_leader_tpraos(
    slot: u64,
    sigma: &FixedDecimal,
    eta0: &[u8],
    pool_vrf_skey: &[u8],
    cert_nat_max: &FixedDecimal,
    c: &FixedDecimal,
) -> Result<bool, Error> {
    let seed: Vec<u8> = mk_seed(slot, eta0);
    let cert_nat: FixedDecimal = FixedDecimal::from(vrf_eval_certified(&seed, pool_vrf_skey)?.as_slice());
    let denominator = cert_nat_max - &cert_nat;
    let recip_q: FixedDecimal = cert_nat_max / &denominator;
    let x: FixedDecimal = -(sigma * c);
    let ordering = x.exp_cmp(1000, 3, &recip_q);

    let span = span!(Level::TRACE, "is_slot_leader_tpraos");
    let _enter = span.enter();
    trace!("is_slot_leader: {}", slot);
    trace!("seed: {}", hex::encode(&seed));
    trace!("cert_nat: {}", &cert_nat);
    trace!("recip_q: {}", &recip_q);
    trace!("c: {}", c);
    trace!("x: {}", &x);

    Ok(ordering.estimation == ExpOrdering::LT)
}

fn get_current_slot(
    byron: &ByronGenesis,
    shelley: &ShelleyGenesis,
    shelley_transition_epoch: u64,
) -> Result<u64, Error> {
    // read byron genesis values
    let byron_slot_length = byron.block_version_data.slot_duration;
    let byron_k = byron.protocol_consts.k;
    let byron_start_time_sec = byron.start_time;
    let byron_epoch_length = 10 * byron_k;
    let byron_end_time_sec =
        byron_start_time_sec + ((shelley_transition_epoch * byron_epoch_length * byron_slot_length) / 1000);

    // read shelley genesis values
    let slot_length = shelley.slot_length;

    let current_time_sec = Utc::now().timestamp() as u64;

    // Calculate current slot
    let byron_slots = shelley_transition_epoch * byron_epoch_length;
    let shelley_slots = (current_time_sec - byron_end_time_sec) / slot_length;
    Ok(byron_slots + shelley_slots)
}

fn get_current_epoch(byron: &ByronGenesis, shelley: &ShelleyGenesis, shelley_transition_epoch: u64) -> u64 {
    // read byron genesis values
    let byron_slot_length = byron.block_version_data.slot_duration;
    let byron_k = byron.protocol_consts.k;
    let byron_start_time_sec = byron.start_time;

    let byron_epoch_length_secs = 10 * byron_k;
    let byron_end_time_sec =
        byron_start_time_sec + ((shelley_transition_epoch * byron_epoch_length_secs * byron_slot_length) / 1000);

    // read shelley genesis values
    let slot_length = shelley.slot_length;
    let epoch_length = shelley.epoch_length;

    let current_time_sec = Utc::now().timestamp() as u64;

    shelley_transition_epoch + ((current_time_sec - byron_end_time_sec) / slot_length / epoch_length)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn calculate_leader_logs(
    db_path: &Path,
    byron_genesis: &Path,
    shelley_genesis: &Path,
    pool_stake: &u64,
    active_stake: &u64,
    d: &f64,
    extra_entropy: &Option<String>,
    ledger_set: &LedgerSet,
    pool_id: &str,
    pool_vrf_skey_path: &Path,
    timezone: &str,
    is_just_nonce: bool,
    consensus: &str,
    shelley_transition_epoch: &Option<u64>,
    nonce: &Option<String>,
    epoch: &Option<u64>,
) -> Result<(), Error> {
    debug!("calculate_leader_logs() start");
    let tz: Tz = timezone.parse::<Tz>().unwrap();

    if !db_path.exists() {
        return Err(Error::Leaderlog(format!(
            "Invalid Path: --db {}",
            db_path.to_string_lossy()
        )));
    }

    if !byron_genesis.exists() {
        return Err(Error::Leaderlog(format!(
            "Invalid Path: --byron-genesis {}",
            byron_genesis.to_string_lossy()
        )));
    }

    if !shelley_genesis.exists() {
        return Err(Error::Leaderlog(format!(
            "Invalid Path: --shelley-genesis {}",
            shelley_genesis.to_string_lossy()
        )));
    }

    if !is_just_nonce && !pool_vrf_skey_path.exists() {
        return Err(Error::Leaderlog(format!(
            "Invalid Path: --pool_vrf_skey {}",
            pool_vrf_skey_path.to_string_lossy()
        )));
    }

    if consensus != "praos" && consensus != "tpraos" && consensus != "cpraos" {
        return Err(Error::Leaderlog(format!("Invalid Consensus: --consensus {consensus}")));
    }

    // check if db_path is a redb database based on magic number
    let use_redb = is_redb_database(db_path)?;

    let mut block_store: Box<dyn BlockStore + Send> = if use_redb {
        Box::new(RedbBlockStore::new(db_path)?)
    } else {
        Box::new(SqLiteBlockStore::new(db_path)?)
    };

    let byron = read_byron_genesis(byron_genesis)?;
    debug!("{:?}", byron);

    let shelley = read_shelley_genesis(shelley_genesis)?;
    debug!("{:?}", shelley);

    let shelley_transition_epoch = match *shelley_transition_epoch {
        None => guess_shelley_transition_epoch(shelley.network_magic),
        Some(value) => value,
    };

    let ledger_info = calculate_ledger_state_sigma_d_and_extra_entropy(pool_stake, active_stake, d, extra_entropy)?;

    let tip_slot_number = match nonce {
        Some(_) => {
            // pretend we're on tip
            let now_slot_number = get_current_slot(&byron, &shelley, shelley_transition_epoch)?;
            debug!("now_slot_number: {}", now_slot_number);
            now_slot_number
        }
        None => {
            let tip_slot_number = block_store.get_tip_slot_number()?;
            debug!("tip_slot_number: {}", tip_slot_number);
            tip_slot_number
        }
    };

    let current_epoch = get_current_epoch(&byron, &shelley, shelley_transition_epoch);

    let epoch_offset = match epoch {
        Some(epoch) => {
            if *epoch > current_epoch || *epoch <= shelley_transition_epoch {
                return Err(Error::Leaderlog(format!("Invalid Epoch: --epoch {epoch}, current_epoch: {current_epoch}, shelley_transition_epoch: {shelley_transition_epoch}")));
            }
            current_epoch - *epoch
        }
        None => 0,
    };
    debug!("epoch_offset: {}", epoch_offset);

    // pretend we're on a different slot number if we want to calculate past or future epochs.
    let additional_slots: i64 = match epoch_offset {
        0 => match ledger_set {
            LedgerSet::Mark => shelley.epoch_length as i64,
            LedgerSet::Set => 0,
            LedgerSet::Go => -(shelley.epoch_length as i64),
        },
        _ => -((shelley.epoch_length * epoch_offset) as i64),
    };

    let (epoch, first_slot_of_epoch) = get_first_slot_of_epoch(
        &byron,
        &shelley,
        (tip_slot_number as i64 + additional_slots) as u64,
        shelley_transition_epoch,
    );
    debug!("epoch: {}", epoch);

    let epoch_nonce: Hash<32> = match nonce {
        Some(nonce) => Hash::<32>::from_str(nonce.as_str())?,
        None => {
            // Make sure we're fully sync'd
            let tip_time = slot_to_naivedatetime(&byron, &shelley, tip_slot_number, shelley_transition_epoch)
                .and_utc()
                .timestamp();
            let system_time = Utc::now().timestamp();
            if system_time - tip_time > 900 {
                return Err(Error::Leaderlog(format!(
                    "db not fully synced! system_time: {system_time}, tip_time: {tip_time}"
                )));
            }

            let first_slot_of_prev_epoch = first_slot_of_epoch - shelley.epoch_length;
            debug!("first_slot_of_epoch: {}", first_slot_of_epoch);
            debug!("first_slot_of_prev_epoch: {}", first_slot_of_prev_epoch);
            let stability_window_multiplier = match consensus {
                "cpraos" => 4u64,
                _ => 3u64,
            };
            let stability_window = ((stability_window_multiplier * byron.protocol_consts.k) as f64
                / shelley.active_slots_coeff)
                .ceil() as u64;
            let stability_window_start = first_slot_of_epoch - stability_window;
            debug!("stability_window: {}", stability_window);
            debug!("stability_window_start: {}", stability_window_start);
            let stability_window_start_plus_1_min = stability_window_start + 60;

            let tip_slot_number = block_store.get_tip_slot_number()?;
            if tip_slot_number < stability_window_start_plus_1_min {
                return Err(Error::Leaderlog(format!(
                    "Not enough blocks sync'd to calculate! Try again later after slot {stability_window_start_plus_1_min} is sync'd."
                )));
            }

            let nc: Hash<32> = block_store.get_eta_v_before_slot(stability_window_start)?;
            debug!("nc: {}", nc);

            let nh: Hash<32> = block_store.get_prev_hash_before_slot(first_slot_of_prev_epoch)?;
            debug!("nh: {}", nh);

            debug!("extra_entropy: {:?}", &ledger_info.extra_entropy);
            let extra_entropy_vec: Option<Vec<u8>> = ledger_info
                .extra_entropy
                .map(|entropy| hex::decode(entropy).expect("Invalid hex string"));
            generate_epoch_nonce(nc, nh, extra_entropy_vec.as_deref())
        }
    };

    if is_just_nonce {
        println!("{}", hex::encode(epoch_nonce));
        return Ok(());
    }

    debug!("epoch_nonce: {}", hex::encode(epoch_nonce));

    let pool_vrf_skey = read_vrf_key(pool_vrf_skey_path)?;
    if pool_vrf_skey.key_type != "VrfSigningKey_PraosVRF" {
        return Err(Error::Leaderlog(
            "Pool VRF Skey must be of type: VrfSigningKey_PraosVRF".to_string(),
        ));
    }

    let sigma = FixedDecimal::from(ledger_info.sigma.0) / FixedDecimal::from(ledger_info.sigma.1);
    debug!("sigma: {}", &sigma);
    debug!("decentralization_param: {:?}", &ledger_info.decentralization);

    let d: f64 = (ledger_info.decentralization * 1000.0).round() / 1000.0;
    debug!("d: {:?}", &d);

    let active_slots_coeff = (shelley.active_slots_coeff * 10000f64) as u64;
    let active_slots_coeff = format!("{}000000000000000000000000000000", active_slots_coeff);
    let active_slots_coeff = FixedDecimal::from_str(&active_slots_coeff.to_string(), DEFAULT_PRECISION)?;
    debug!("active_slots_coeff: {}", &active_slots_coeff);

    let d_multiplier = FixedDecimal::from(((1.0 - d) * 1000.0).round() as u64) / FixedDecimal::from(1000u64);
    let epoch_slots_ideal = f64::from_str(
        &(&sigma * &(&FixedDecimal::from(shelley.epoch_length) * &active_slots_coeff) * d_multiplier).to_string(),
    )?;
    let epoch_slots_ideal = (epoch_slots_ideal * 100.0).round() / 100.0;

    let mut leader_log = LeaderLog {
        status: "ok".to_string(),
        epoch,
        epoch_nonce: hex::encode(epoch_nonce),
        consensus: consensus.to_string(),
        epoch_slots: 0,
        epoch_slots_ideal,
        max_performance: 0.0,
        pool_id: pool_id.to_string(),
        sigma: f64::from_str(&sigma.to_string())?,
        active_stake: ledger_info.sigma.0,
        total_active_stake: ledger_info.sigma.1,
        d,
        f: shelley.active_slots_coeff,
        assigned_slots: vec![],
    };

    let cert_nat_max: FixedDecimal = match consensus {
        "tpraos" => FixedDecimal::from_str("134078079299425970995740249982058461274793658205923933777235614437217640300735469768018742981669034276900318581864860508537538828119465699464336490060840960000000000000000000000000000000000", DEFAULT_PRECISION)?, // 2^512
        "praos" | "cpraos" => FixedDecimal::from_str("1157920892373161954235709850086879078532699846656405640394575840079131296399360000000000000000000000000000000000", DEFAULT_PRECISION)?, // 2^256
        _ => return Err(Error::Leaderlog(format!(
            "Invalid Consensus: --consensus {consensus}"
        )))
    };
    let c: FixedDecimal = (FixedDecimal::from(1u64) - active_slots_coeff).ln();

    // Calculate all of our assigned slots in the epoch (in parallel)
    let assigned_slots = (0..shelley.epoch_length)
        .par_bridge() // <--- use rayon parallel bridge
        .map(|slot_in_epoch| first_slot_of_epoch + slot_in_epoch)
        .filter(|epoch_slot| !is_overlay_slot(&first_slot_of_epoch, epoch_slot, &ledger_info.decentralization))
        .filter_map(|leader_slot| match consensus {
            "tpraos" => {
                match is_slot_leader_tpraos(
                    leader_slot,
                    &sigma,
                    epoch_nonce.as_slice(),
                    &pool_vrf_skey.key,
                    &cert_nat_max,
                    &c,
                ) {
                    Ok(true) => Some(leader_slot),
                    Ok(false) => None,
                    Err(msg) => {
                        handle_error(msg);
                        None
                    }
                }
            }
            "praos" | "cpraos" => {
                match is_slot_leader_praos(
                    leader_slot,
                    &sigma,
                    epoch_nonce.as_slice(),
                    &pool_vrf_skey.key,
                    &cert_nat_max,
                    &c,
                ) {
                    Ok(true) => Some(leader_slot),
                    Ok(false) => None,
                    Err(msg) => {
                        handle_error(msg);
                        None
                    }
                }
            }
            _ => panic!(),
        })
        .collect::<Vec<_>>();

    // Update leader log with all assigned slots (sort first)
    for (i, slot) in sorted(assigned_slots.iter()).enumerate() {
        let no = (i + 1) as u64;
        let slot = Slot {
            no,
            slot: *slot,
            slot_in_epoch: slot - first_slot_of_epoch,
            at: slot_to_timestamp(&byron, &shelley, *slot, &tz, shelley_transition_epoch),
        };

        debug!("Found assigned slot: {:?}", &slot);
        leader_log.assigned_slots.push(slot);
        leader_log.epoch_slots = no;
    }

    // Calculate expected performance
    leader_log.max_performance = (leader_log.epoch_slots as f64 / epoch_slots_ideal * 10000.0).round() / 100.0;

    // Save slots to database so we can send to pooltool later
    let mut slots = String::new();
    slots.push('[');
    for (i, assigned_slot) in leader_log.assigned_slots.iter().enumerate() {
        if i > 0 {
            slots.push(',');
        }
        slots.push_str(&assigned_slot.slot.to_string())
    }
    slots.push(']');

    let hash = Hasher::<256>::hash(slots.as_bytes()).to_string();

    block_store.save_slots(epoch, pool_id, assigned_slots.len() as u64, slots.as_str(), &hash)?;

    println!("{}", serde_json::to_string_pretty(&leader_log)?);

    Ok(())
}

pub(crate) fn status(db_path: &Path, byron_genesis: &Path, shelley_genesis: &Path, shelley_trans_epoch: &Option<u64>) {
    if !db_path.exists() {
        handle_error("database not found!");
        return;
    }
    // check if db_path is a redb database based on magic number
    let use_redb = is_redb_database(db_path).expect("infallible");

    let mut block_store: Box<dyn BlockStore + Send> = if use_redb {
        Box::new(RedbBlockStore::new(db_path).expect("infallible"))
    } else {
        Box::new(SqLiteBlockStore::new(db_path).expect("infallible"))
    };

    match read_byron_genesis(byron_genesis) {
        Ok(byron) => {
            debug!("{:?}", byron);
            match read_shelley_genesis(shelley_genesis) {
                Ok(shelley) => {
                    debug!("{:?}", shelley);
                    match block_store.get_tip_slot_number() {
                        Ok(tip_slot_number) => {
                            debug!("tip_slot_number: {}", tip_slot_number);
                            let tip_time = slot_to_naivedatetime(
                                &byron,
                                &shelley,
                                tip_slot_number,
                                shelley_trans_epoch.expect("infallible"),
                            )
                            .and_utc()
                            .timestamp();
                            let system_time = Utc::now().timestamp();
                            if system_time - tip_time < 120 {
                                print_status_synced();
                            } else {
                                handle_error("db not fully synced!")
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

pub(crate) fn send_slots(
    db_path: &Path,
    byron_genesis: &Path,
    shelley_genesis: &Path,
    pooltool_config: PooltoolConfig,
    shelley_trans_epoch: &Option<u64>,
    override_time: &Option<String>,
) {
    if !db_path.exists() {
        handle_error("database not found!");
        return;
    }
    // check if db_path is a redb database based on magic number
    let use_redb = is_redb_database(db_path).expect("infallible");

    let mut block_store: Box<dyn BlockStore + Send> = if use_redb {
        Box::new(RedbBlockStore::new(db_path).expect("infallible"))
    } else {
        Box::new(SqLiteBlockStore::new(db_path).expect("infallible"))
    };

    match read_byron_genesis(byron_genesis) {
        Ok(byron) => {
            debug!("{:?}", byron);
            match read_shelley_genesis(shelley_genesis) {
                Ok(shelley) => {
                    debug!("{:?}", shelley);
                    match block_store.get_tip_slot_number() {
                        Ok(tip_slot_number) => {
                            debug!("tip_slot_number: {}", tip_slot_number);
                            let tip_time = slot_to_naivedatetime(
                                &byron,
                                &shelley,
                                tip_slot_number,
                                shelley_trans_epoch.expect("infallible"),
                            )
                            .and_utc()
                            .timestamp();
                            let system_time = Utc::now().timestamp();
                            if system_time - tip_time < 120 {
                                let (epoch, _) = get_first_slot_of_epoch(
                                    &byron,
                                    &shelley,
                                    tip_slot_number,
                                    shelley_trans_epoch.expect("infallible"),
                                );
                                debug!("epoch: {}", epoch);
                                for pool in pooltool_config.pools.iter() {
                                    match block_store.get_current_slots(epoch, &pool.pool_id) {
                                        Ok((slot_qty, hash)) => {
                                            debug!("slot_qty: {}", slot_qty);
                                            debug!("hash: {}", &hash);
                                            match block_store.get_previous_slots(epoch - 1, &pool.pool_id) {
                                                Ok(prev_slots) => {
                                                    let request = serde_json::ser::to_string(&PooltoolSendSlots {
                                                        api_key: pooltool_config.api_key.clone(),
                                                        pool_id: pool.pool_id.clone(),
                                                        epoch,
                                                        slot_qty,
                                                        hash,
                                                        override_time: override_time.clone(),
                                                        prev_slots,
                                                    })
                                                    .unwrap();
                                                    info!("Sending: {}", &request);
                                                    match reqwest::blocking::Client::builder().build() {
                                                        Ok(client) => {
                                                            let pooltool_result = client
                                                                .post("https://api.pooltool.io/v0/sendslots")
                                                                .body(request)
                                                                .send();

                                                            match pooltool_result {
                                                                Ok(response) => match response.text() {
                                                                    Ok(text) => {
                                                                        info!("Pooltool Response: {}", text);
                                                                    }
                                                                    Err(error) => {
                                                                        error!("PoolTool error: {}", error);
                                                                    }
                                                                },
                                                                Err(error) => {
                                                                    error!("PoolTool error: {}", error);
                                                                }
                                                            }
                                                        }
                                                        Err(err) => {
                                                            error!("Could not set up the reqwest client!: {}", err)
                                                        }
                                                    }
                                                }
                                                Err(error) => {
                                                    error!("Db Error: {}", error)
                                                }
                                            }
                                        }
                                        Err(error) => {
                                            error!("Cannot find db record for {},{}: {}", epoch, &pool.pool_id, error)
                                        }
                                    }
                                }
                            } else {
                                handle_error("db not fully synced!")
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

fn print_status_synced() {
    println!(
        "{{\n\
            \x20\"status\": \"ok\"\n\
            }}"
    );
}

pub fn handle_error<T: Display>(error_message: T) {
    serde_json::ser::to_writer_pretty(
        &mut stdout(),
        &LeaderLogError {
            status: "error".to_string(),
            error_message: format!("{error_message}"),
        },
    )
    .unwrap();
}

#[cfg(test)]
mod tests {
    use crate::nodeclient::leaderlog::is_overlay_slot;
    use chrono::{NaiveDateTime, Utc};

    #[test]
    fn test_is_overlay_slot() {
        let first_slot_of_epoch = 15724800_u64;
        let mut current_slot = 16128499_u64;
        let d: f64 = 32_f64 / 100_f64;

        assert!(!is_overlay_slot(&first_slot_of_epoch, &current_slot, &d));

        // AD test
        current_slot = 15920150_u64;
        assert!(is_overlay_slot(&first_slot_of_epoch, &current_slot, &d));
    }

    #[test]
    fn test_date_parsing() {
        let genesis_start_time_sec = NaiveDateTime::parse_from_str("2022-10-25T00:00:00Z", "%Y-%m-%dT%H:%M:%S%.fZ")
            .unwrap()
            .and_utc()
            .timestamp();

        assert_eq!(genesis_start_time_sec, 1666656000);
    }

    #[test]
    fn test_date_parsing2() {
        let genesis_start_time_sec =
            NaiveDateTime::parse_from_str("2024-05-16T17:18:10.000000000Z", "%Y-%m-%dT%H:%M:%S%.fZ")
                .unwrap()
                .and_utc()
                .timestamp();

        assert_eq!(genesis_start_time_sec, 1715879890);
    }

    #[test]
    fn test_date_parsing3() {
        let genesis_start_time_sec = NaiveDateTime::parse_from_str("2021-12-09T22:55:22Z", "%Y-%m-%dT%H:%M:%S%.fZ")
            .unwrap()
            .and_utc()
            .timestamp();
        assert_eq!(genesis_start_time_sec, 1639090522);
        let current_time_sec = Utc::now().timestamp();
        println!("current_time_sec: {}", current_time_sec);
        let current_epoch = (current_time_sec - genesis_start_time_sec) / 3600;
        println!("current_epoch: {}", current_epoch);
    }

    #[test]
    fn test_date_parsing_mainnet() {
        let genesis_start_time_sec = NaiveDateTime::parse_from_str("2017-09-23T21:44:51Z", "%Y-%m-%dT%H:%M:%S%.fZ")
            .unwrap()
            .and_utc()
            .timestamp();

        assert_eq!(genesis_start_time_sec, 1506203091);
        let current_time_sec = Utc::now().timestamp();
        println!("current_time_sec: {}", current_time_sec);
        let current_epoch = (current_time_sec - genesis_start_time_sec) / 432000;
        println!("current_epoch: {}", current_epoch);
    }
}
