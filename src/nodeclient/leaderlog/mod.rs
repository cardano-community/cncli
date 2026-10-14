use crate::nodeclient::blockstore;
use crate::nodeclient::blockstore::redb::{is_redb_database, RedbBlockStore};
use crate::nodeclient::blockstore::sqlite::SqLiteBlockStore;
use crate::nodeclient::blockstore::BlockStore;
use crate::nodeclient::leaderlog::deserialize::cbor_hex;
use crate::nodeclient::leaderlog::ledgerstate::calculate_ledger_state_sigma_d_and_extra_entropy;
use crate::nodeclient::sync::pooltool::PooltoolConfig;
use crate::LedgerSet;
use chrono::{DateTime, NaiveDateTime, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use dashu_int::IBig;
use pallas_crypto::hash::{Hash, Hasher};
use rational::ExactRatio;

use pallas_crypto::nonce::generate_epoch_nonce;
use pallas_math::math::FixedDecimal;
use serde::{Deserialize, Serialize};
use serde_aux::prelude::deserialize_number_from_string;
use std::fs::File;
use std::io::{stdout, BufReader};
use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{debug, error, info};

mod deserialize;
mod election;
mod ledgerstate;
mod rational;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Output error: {0}")]
    Output(#[from] crate::OutputError),

    #[error("Rusqlite error: {0}")]
    Rusqlite(#[from] rusqlite::Error),

    #[error("FromHex error: {0}")]
    FromHex(#[from] hex::FromHexError),

    #[error("PallasMath error: {0}")]
    PallasMath(#[from] pallas_math::math::Error),

    #[error("Leaderlog error: {0}")]
    Leaderlog(String),

    #[error("Blockstore error: {0}")]
    Blockstore(#[source] Box<blockstore::Error>),

    #[error("Redb error: {0}")]
    Redb(#[source] Box<blockstore::redb::Error>),

    #[error("Sqlite error: {0}")]
    Sqlite(#[from] blockstore::sqlite::Error),

    #[error("ParseFloat error: {0}")]
    ParseFloat(#[from] std::num::ParseFloatError),
}

impl From<blockstore::Error> for Error {
    fn from(err: blockstore::Error) -> Self {
        Error::Blockstore(Box::new(err))
    }
}

impl From<blockstore::redb::Error> for Error {
    fn from(err: blockstore::redb::Error) -> Self {
        Error::Redb(Box::new(err))
    }
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
    #[serde(deserialize_with = "deserialize_active_slots_coeff")]
    active_slots_coeff: ExactRatio,
    network_magic: u32,
    slot_length: u64,
    epoch_length: u64,
}

fn deserialize_active_slots_coeff<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<ExactRatio, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    let ratio = match value {
        serde_json::Value::Number(number) => ExactRatio::from_text(&number.to_string()),
        serde_json::Value::Object(object) => {
            let parse_integer = |name| match object.get(name) {
                Some(serde_json::Value::Number(number)) => {
                    let text = number.to_string();
                    let digits = text.strip_prefix('-').unwrap_or(&text);
                    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
                        return Err("Rational JSON parts must be integers".to_string());
                    }
                    IBig::from_str(&text).map_err(|_| "Invalid rational JSON integer".to_string())
                }
                _ => Err("Rational JSON parts must be integers".to_string()),
            };
            parse_integer("numerator").and_then(|numerator| {
                parse_integer("denominator").and_then(|denominator| ExactRatio::from_parts(numerator, denominator))
            })
        }
        _ => Err("Active slots coefficient must be a number or rational object".to_string()),
    }
    .map_err(serde::de::Error::custom)?;
    if ratio.numerator == 0 {
        return Err(serde::de::Error::custom("Active slots coefficient must be positive"));
    }
    Ok(ratio)
}

fn slot_range_error() -> Error {
    Error::Leaderlog("Leader schedule arithmetic exceeds supported slot range".to_string())
}

fn stability_window(k: u64, multiplier: u64, f: &ExactRatio) -> Result<u64, Error> {
    if f.numerator == 0 {
        return Err(Error::Leaderlog(
            "Invalid leader schedule stake or probability parameters".to_string(),
        ));
    }
    let numerator = IBig::from(k) * IBig::from(multiplier) * IBig::from(f.denominator);
    let denominator = IBig::from(f.numerator);
    let quotient = &numerator / &denominator;
    let ceiling = quotient + IBig::from(u8::from(&numerator % &denominator != IBig::ZERO));
    u64::try_from(&ceiling).map_err(|_| slot_range_error())
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

fn is_overlay_slot(offset: u64, d: &ExactRatio) -> bool {
    let denominator = u128::from(d.denominator);
    let numerator = u128::from(d.numerator);
    let ceiling = |offset: u128| {
        let product = offset * numerator;
        product / denominator + u128::from(!product.is_multiple_of(denominator))
    };
    ceiling(u128::from(offset)) < ceiling(u128::from(offset) + 1)
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
    d: &str,
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
    let tz: Tz = timezone
        .parse::<Tz>()
        .map_err(|_| Error::Leaderlog("Invalid timezone".to_string()))?;

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
    if shelley.slot_length == 0 || shelley.epoch_length == 0 || byron.protocol_consts.k == 0 {
        return Err(Error::Leaderlog(
            "Genesis slot and epoch lengths must be nonzero".to_string(),
        ));
    }
    let d = ExactRatio::from_text(d)
        .map_err(|_| Error::Leaderlog("Invalid leader schedule stake or probability parameters".to_string()))?;
    if !is_just_nonce && (*active_stake == 0 || pool_stake > active_stake) {
        return Err(Error::Leaderlog(
            "Invalid leader schedule stake or probability parameters".to_string(),
        ));
    }

    let shelley_transition_epoch = match *shelley_transition_epoch {
        None => guess_shelley_transition_epoch(shelley.network_magic),
        Some(value) => value,
    };

    let ledger_info = calculate_ledger_state_sigma_d_and_extra_entropy(pool_stake, active_stake, &d, extra_entropy)?;
    let extra_entropy_vec = ledger_info.extra_entropy.as_ref().map(hex::decode).transpose()?;

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
    if tip_slot_number == 0 {
        return Err(Error::Leaderlog("db not fully synced!".to_string()));
    }

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

            let first_slot_of_prev_epoch = first_slot_of_epoch
                .checked_sub(shelley.epoch_length)
                .ok_or_else(slot_range_error)?;
            debug!("first_slot_of_epoch: {}", first_slot_of_epoch);
            debug!("first_slot_of_prev_epoch: {}", first_slot_of_prev_epoch);
            let stability_window_multiplier = match consensus {
                "cpraos" => 4u64,
                _ => 3u64,
            };
            let stability_window = stability_window(
                byron.protocol_consts.k,
                stability_window_multiplier,
                &shelley.active_slots_coeff,
            )?;
            let stability_window_start = first_slot_of_epoch
                .checked_sub(stability_window)
                .ok_or_else(slot_range_error)?;
            debug!("stability_window: {}", stability_window);
            debug!("stability_window_start: {}", stability_window_start);
            let stability_window_start_plus_1_min =
                stability_window_start.checked_add(60).ok_or_else(slot_range_error)?;

            let tip_slot_number = block_store.get_tip_slot_number()?;
            if tip_slot_number < stability_window_start_plus_1_min {
                return Err(Error::Leaderlog(format!(
                    "Not enough blocks sync'd to calculate! Try again later after slot {stability_window_start_plus_1_min} is sync'd."
                )));
            }

            let nc = block_store.get_eta_v_before_slot(stability_window_start)?;
            debug!("nc: {}", nc);

            let nh = block_store.get_prev_hash_before_slot(first_slot_of_prev_epoch)?;
            debug!("nh: {}", nh);

            debug!("extra_entropy: {:?}", &ledger_info.extra_entropy);
            generate_epoch_nonce(nc, nh, extra_entropy_vec.as_deref())
        }
    };

    if is_just_nonce {
        crate::write_line(&mut stdout().lock(), &hex::encode(epoch_nonce))?;
        return Ok(());
    }

    debug!("epoch_nonce: {}", hex::encode(epoch_nonce));

    let pool_vrf_skey = read_vrf_key(pool_vrf_skey_path)?;
    if pool_vrf_skey.key_type != "VrfSigningKey_PraosVRF" {
        return Err(Error::Leaderlog(
            "Pool VRF Skey must be of type: VrfSigningKey_PraosVRF".to_string(),
        ));
    }
    if pool_vrf_skey.key.len() < 32 {
        return Err(Error::Leaderlog(
            "VRF signing key must contain at least 32 bytes".to_string(),
        ));
    }

    let sigma = FixedDecimal::from(ledger_info.sigma.0) / FixedDecimal::from(ledger_info.sigma.1);
    debug!("sigma: {}", &sigma);
    debug!("decentralization_param: {:?}", &ledger_info.decentralization);

    let d = ledger_info.decentralization;
    let active_slots_coeff = shelley.active_slots_coeff.to_fixed();
    let d_multiplier = if consensus == "tpraos" {
        ExactRatio {
            numerator: d.denominator - d.numerator,
            denominator: d.denominator,
        }
        .to_fixed()
    } else {
        FixedDecimal::from(1u64)
    };
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
        d: f64::from_str(&d.to_fixed().to_string())?,
        f: f64::from_str(&shelley.active_slots_coeff.to_fixed().to_string())?,
        assigned_slots: vec![],
    };

    let threshold = election::LeaderThreshold::new(
        ledger_info.sigma.0,
        ledger_info.sigma.1,
        &shelley.active_slots_coeff,
        if consensus == "tpraos" { 64 } else { 32 },
    )?;
    let assigned_slots = election::calculate_schedule(
        first_slot_of_epoch,
        shelley.epoch_length,
        consensus,
        &d,
        &epoch_nonce,
        &pool_vrf_skey.key,
        &threshold,
    )?;

    // Schedule order is the persisted and displayed order.
    for (i, slot) in assigned_slots.iter().enumerate() {
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

    crate::write_json(&mut stdout().lock(), &leader_log)?;

    Ok(())
}

pub(crate) fn status(
    db_path: &Path,
    byron_genesis: &Path,
    shelley_genesis: &Path,
    shelley_trans_epoch: &Option<u64>,
) -> Result<(), Error> {
    if !db_path.exists() {
        return Err(Error::Leaderlog("database not found!".to_string()));
    }
    let mut block_store: Box<dyn BlockStore + Send> = if is_redb_database(db_path)? {
        Box::new(RedbBlockStore::new(db_path)?)
    } else {
        Box::new(SqLiteBlockStore::new(db_path)?)
    };
    let byron = read_byron_genesis(byron_genesis)?;
    let shelley = read_shelley_genesis(shelley_genesis)?;
    let shelley_trans_epoch =
        shelley_trans_epoch.unwrap_or_else(|| guess_shelley_transition_epoch(shelley.network_magic));
    let tip_slot_number = block_store.get_tip_slot_number()?;
    if tip_slot_number == 0 {
        return Err(Error::Leaderlog("db not fully synced!".to_string()));
    }
    let tip_time = slot_to_naivedatetime(&byron, &shelley, tip_slot_number, shelley_trans_epoch)
        .and_utc()
        .timestamp();
    if Utc::now().timestamp() - tip_time >= 120 {
        return Err(Error::Leaderlog("db not fully synced!".to_string()));
    }
    crate::write_json(&mut stdout().lock(), &serde_json::json!({"status": "ok"}))?;
    Ok(())
}

pub(crate) fn send_slots(
    db_path: &Path,
    byron_genesis: &Path,
    shelley_genesis: &Path,
    pooltool_config: PooltoolConfig,
    shelley_trans_epoch: &Option<u64>,
    override_time: &Option<String>,
) -> Result<(), Error> {
    if pooltool_config.api_key.trim().is_empty()
        || pooltool_config.pools.is_empty()
        || pooltool_config.pools.iter().any(|pool| pool.pool_id.trim().is_empty())
    {
        return Err(Error::Leaderlog("Invalid PoolTool configuration".to_string()));
    }
    if !db_path.exists() {
        return Err(Error::Leaderlog("database not found!".to_string()));
    }
    let mut block_store: Box<dyn BlockStore + Send> = if is_redb_database(db_path)? {
        Box::new(RedbBlockStore::new(db_path)?)
    } else {
        Box::new(SqLiteBlockStore::new(db_path)?)
    };
    let byron = read_byron_genesis(byron_genesis)?;
    let shelley = read_shelley_genesis(shelley_genesis)?;
    let tip_slot_number = block_store.get_tip_slot_number()?;
    if tip_slot_number == 0 {
        return Err(Error::Leaderlog("db not fully synced!".to_string()));
    }
    let shelley_trans_epoch =
        shelley_trans_epoch.unwrap_or_else(|| guess_shelley_transition_epoch(shelley.network_magic));
    let tip_time = slot_to_naivedatetime(&byron, &shelley, tip_slot_number, shelley_trans_epoch)
        .and_utc()
        .timestamp();
    if Utc::now().timestamp() - tip_time >= 120 {
        return Err(Error::Leaderlog("db not fully synced!".to_string()));
    }
    let (epoch, _) = get_first_slot_of_epoch(&byron, &shelley, tip_slot_number, shelley_trans_epoch);
    let previous_epoch = epoch
        .checked_sub(1)
        .ok_or_else(|| Error::Leaderlog("No previous epoch available".to_string()))?;
    let client = reqwest::blocking::Client::builder()
        .user_agent(crate::APP_USER_AGENT)
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()?;
    for pool in &pooltool_config.pools {
        let (slot_qty, hash) = block_store.get_current_slots(epoch, &pool.pool_id)?;
        let prev_slots = block_store.get_previous_slots(previous_epoch, &pool.pool_id)?;
        let request = PooltoolSendSlots {
            api_key: pooltool_config.api_key.clone(),
            pool_id: pool.pool_id.clone(),
            epoch,
            slot_qty,
            hash,
            override_time: override_time.clone(),
            prev_slots,
        };
        post_slots(&client, crate::nodeclient::sync::pooltool::POOLTOOL_BASE_URL, &request)?;
    }
    Ok(())
}

fn post_slots(client: &reqwest::blocking::Client, base_url: &str, request: &PooltoolSendSlots) -> Result<(), Error> {
    let started = Instant::now();
    let result = (|| {
        let body = serde_json::to_string(request)?;
        let response = client.post(format!("{base_url}/v0/sendslots")).body(body).send()?;
        response.error_for_status().map_err(Error::from)
    })();
    let http_status = match &result {
        Ok(response) => Some(response.status().as_u16()),
        Err(Error::Http(error)) => error.status().map(|status| status.as_u16()),
        Err(_) => None,
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(_) => {
            info!(
                operation = "pooltool.sendslots",
                pool_id = %request.pool_id,
                epoch = request.epoch,
                http_status,
                duration_ms,
                outcome = "ok"
            );
            Ok(())
        }
        Err(error) => {
            error!(
                operation = "pooltool.sendslots",
                pool_id = %request.pool_id,
                epoch = request.epoch,
                http_status,
                duration_ms,
                outcome = "error"
            );
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::nodeclient::leaderlog::is_overlay_slot;

    #[test]
    fn exact_stability_window_and_overflow() {
        use super::{rational::ExactRatio, stability_window};
        let f = ExactRatio::from_text("1/3").unwrap();
        assert_eq!(stability_window(2, 3, &f).unwrap(), 18);
        assert_eq!(
            stability_window(1, 3, &ExactRatio::from_text("0.4").unwrap()).unwrap(),
            8
        );
        assert!(stability_window(u64::MAX, 4, &f).is_err());
        assert!(stability_window(1, 3, &ExactRatio::from_text("0").unwrap()).is_err());
    }

    #[test]
    fn genesis_coefficients_keep_lexical_precision() {
        for (value, expected) in [
            ("0.12345678901234567", "12345678901234567/100000000000000000"),
            ("1e-19", "1/10000000000000000000"),
            (r#"{"numerator":-2,"denominator":-6}"#, "1/3"),
            ("1", "1"),
        ] {
            let genesis: super::ShelleyGenesis = serde_json::from_str(&format!(
                r#"{{"activeSlotsCoeff":{value},"networkMagic":42,"slotLength":1,"epochLength":60}}"#
            ))
            .unwrap();
            assert_eq!(
                genesis.active_slots_coeff,
                super::rational::ExactRatio::from_text(expected).unwrap()
            );
        }
        for value in [
            "0",
            "-0.1",
            "1.1",
            "1e-20",
            r#"{"numerator":1,"denominator":0}"#,
            r#""0.05""#,
            "null",
        ] {
            assert!(
                serde_json::from_str::<super::ShelleyGenesis>(&format!(
                    r#"{{"activeSlotsCoeff":{value},"networkMagic":42,"slotLength":1,"epochLength":60}}"#
                ))
                .is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn short_key_cannot_save_even_an_overlay_only_schedule() {
        use super::{calculate_leader_logs, BlockStore, SqLiteBlockStore};
        use std::fs;
        let directory = std::env::temp_dir().join(format!("cncli-leaderlog-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&directory).unwrap();
        let db = directory.join("chain.db");
        let byron = directory.join("byron.json");
        let shelley = directory.join("shelley.json");
        let key = directory.join("vrf.skey");
        drop(SqLiteBlockStore::new(&db).unwrap());
        fs::write(
            &byron,
            serde_json::to_vec(&serde_json::json!({
                "startTime": Utc::now().timestamp() - 1000,
                "protocolConsts": {"k": 1},
                "blockVersionData": {"slotDuration": "1000"}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            &shelley,
            r#"{"activeSlotsCoeff":0.05,"networkMagic":42,"slotLength":1,"epochLength":100}"#,
        )
        .unwrap();
        fs::write(
            &key,
            serde_json::to_vec(&serde_json::json!({
                "type": "VrfSigningKey_PraosVRF",
                "cborHex": format!("581f{}", "00".repeat(31))
            }))
            .unwrap(),
        )
        .unwrap();
        for consensus in ["praos", "tpraos", "cpraos"] {
            let result = calculate_leader_logs(
                &db,
                &byron,
                &shelley,
                &1,
                &100,
                "1",
                &None,
                &crate::LedgerSet::Set,
                "test-pool",
                &key,
                "UTC",
                false,
                consensus,
                &Some(0),
                &Some("00".repeat(32)),
                &Some(2),
            );
            assert!(matches!(result, Err(super::Error::Leaderlog(_))));
            let mut store = SqLiteBlockStore::new(&db).unwrap();
            assert!(store.get_current_slots(2, "test-pool").is_err());
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn sendslots_http_logging_boundary() {
        use super::{post_slots, PooltoolSendSlots};
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::sync::Mutex;

        #[derive(Clone)]
        struct LogWriter(Arc<Mutex<Vec<u8>>>);

        impl Write for LogWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.blocking_lock().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        const SECRET: &str = "CNCLI_TEST_SECRET_DO_NOT_LOG";
        for (status, outcome) in [(200, "ok"), (429, "error"), (500, "error")] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                socket.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut received = Vec::new();
                let (header_end, content_length) = loop {
                    let mut chunk = [0; 1024];
                    let count = socket.read(&mut chunk).unwrap();
                    assert_ne!(count, 0, "request ended before its HTTP headers");
                    received.extend_from_slice(&chunk[..count]);
                    if let Some(end) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let headers = std::str::from_utf8(&received[..end]).unwrap();
                        assert_eq!(headers.lines().next().unwrap(), "POST /v0/sendslots HTTP/1.1");
                        let length = headers
                            .lines()
                            .filter_map(|line| line.split_once(':'))
                            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .unwrap()
                            .1
                            .trim()
                            .parse::<usize>()
                            .unwrap();
                        break (end + 4, length);
                    }
                };
                while received.len() < header_end + content_length {
                    let mut chunk = [0; 1024];
                    let count = socket.read(&mut chunk).unwrap();
                    assert_ne!(count, 0, "request ended before its HTTP body");
                    received.extend_from_slice(&chunk[..count]);
                }
                let payload: serde_json::Value =
                    serde_json::from_slice(&received[header_end..header_end + content_length]).unwrap();
                let echo = serde_json::to_string(&payload).unwrap();
                write!(
                    socket,
                    "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{echo}",
                    echo.len()
                )
                .unwrap();
                payload
            });
            let logs = Arc::new(Mutex::new(Vec::new()));
            let writer = LogWriter(Arc::clone(&logs));
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_ansi(false)
                .without_time()
                .with_writer(move || writer.clone())
                .finish();
            let request = PooltoolSendSlots {
                api_key: SECRET.to_string(),
                // A short, multibyte ID also proves logging does not slice at byte eight.
                pool_id: "池".to_string(),
                epoch: 42,
                slot_qty: 2,
                hash: "CNCLI_TEST_HASH_DO_NOT_LOG".to_string(),
                override_time: (status == 200).then(|| "2026-10-02T00:00:00Z".to_string()),
                prev_slots: (status == 200).then(|| "CNCLI_TEST_SLOTS_DO_NOT_LOG".to_string()),
            };
            let result = tracing::subscriber::with_default(subscriber, || {
                let client = reqwest::blocking::Client::builder()
                    .no_proxy()
                    .timeout(Duration::from_secs(3))
                    .build()
                    .unwrap();
                post_slots(&client, &base_url, &request)
            });
            let payload = server.join().unwrap();
            assert_eq!(payload, serde_json::to_value(&request).unwrap());
            assert_eq!(payload["apiKey"], SECRET);
            assert_eq!(payload["poolId"], "池");
            assert_eq!(payload["epoch"], 42);
            assert_eq!(payload["slotQty"], 2);
            assert_eq!(payload["hash"], "CNCLI_TEST_HASH_DO_NOT_LOG");
            assert_eq!(payload.as_object().unwrap().len(), if status == 200 { 7 } else { 5 });
            if status == 200 {
                assert_eq!(payload["overrideTime"], "2026-10-02T00:00:00Z");
                assert_eq!(payload["prevSlots"], "CNCLI_TEST_SLOTS_DO_NOT_LOG");
            } else {
                assert!(payload.get("overrideTime").is_none());
                assert!(payload.get("prevSlots").is_none());
            }
            assert_eq!(result.is_ok(), status == 200);
            if status != 200 {
                assert!(matches!(result, Err(super::Error::Http(error)) if error.status().unwrap().as_u16() == status));
            }
            let logs = String::from_utf8(logs.blocking_lock().clone()).unwrap();
            assert!(!logs.contains(SECRET), "{logs}");
            assert!(!logs.contains(&request.hash), "{logs}");
            assert!(!logs.contains("CNCLI_TEST_SLOTS_DO_NOT_LOG"), "{logs}");
            assert!(!logs.contains("apiKey"), "{logs}");
            assert!(!logs.contains("slotQty"), "{logs}");
            assert!(!logs.contains("overrideTime"), "{logs}");
            assert!(logs.contains("operation=\"pooltool.sendslots\""), "{logs}");
            assert!(logs.contains("pool_id=池"), "{logs}");
            assert!(logs.contains("epoch=42"), "{logs}");
            assert!(logs.contains(&format!("http_status={status}")), "{logs}");
            assert!(logs.contains("duration_ms="), "{logs}");
            assert!(logs.contains(&format!("outcome=\"{outcome}\"")), "{logs}");
        }
    }

    #[test]
    fn test_is_overlay_slot() {
        let first_slot_of_epoch = 15724800_u64;
        let d = super::rational::ExactRatio::from_text("0.32").unwrap();
        assert!(!is_overlay_slot(16128499 - first_slot_of_epoch, &d));
        assert!(is_overlay_slot(15920150 - first_slot_of_epoch, &d));
        let tiny = super::rational::ExactRatio::from_text("1/2500").unwrap();
        assert!(is_overlay_slot(0, &tiny));
        assert!(!is_overlay_slot(2499, &tiny));
        assert!(is_overlay_slot(2500, &tiny));
        assert!(is_overlay_slot(
            u64::MAX,
            &super::rational::ExactRatio::from_text("1").unwrap()
        ));
    }
}
