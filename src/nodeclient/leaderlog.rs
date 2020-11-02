use std::fs::File;
use std::io::{BufReader, Error};
use std::path::PathBuf;

use rusqlite::{Connection, NO_PARAMS};
use serde::Deserialize;
use serde::export::fmt::Display;
use serde_aux::prelude::deserialize_number_from_string;

use crate::nodeclient::leaderlog::ledgerstate::calculate_ledger_state_sigma_and_d;
use crate::nodeclient::LedgerSet;

mod ledgerstate;

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
    k: i64
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlockVersionData {
    #[serde(deserialize_with = "deserialize_number_from_string")]
    slot_duration: u64
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShelleyGenesis {
    active_slots_coeff: f64,
    network_magic: u32,
    slot_length: i64,
    epoch_length: i64,
}

fn read_byron_genesis(byron_genesis: &PathBuf) -> Result<ByronGenesis, Error> {
    let buf = BufReader::new(File::open(byron_genesis)?);
    Ok(serde_json::from_reader(buf)?)
}

fn read_shelley_genesis(shelley_genesis: &PathBuf) -> Result<ShelleyGenesis, Error> {
    let buf = BufReader::new(File::open(shelley_genesis)?);
    Ok(serde_json::from_reader(buf)?)
}

fn get_tip_slot_number(db: &Connection) -> Result<i64, rusqlite::Error> {
    Ok(
        db.query_row("SELECT MAX(slot_number) FROM chain", NO_PARAMS, |row| Ok(row.get(0)?))?
    )
}

fn get_eta_v_before_slot(db: &Connection, slot_number: i64) -> Result<String, rusqlite::Error> {
    Ok(
        db.query_row("SELECT eta_v FROM chain WHERE slot_number < ?1 AND ?1 - slot_number < 120 ORDER BY slot_number DESC LIMIT 1", &[&slot_number], |row| {
            Ok(row.get(0)?)
        })?
    )
}

fn get_first_slot_of_epoch(byron: &ByronGenesis, shelley: &ShelleyGenesis, current_slot: i64) -> i64 {
    let shelley_transition_epoch: i64 = if shelley.network_magic == 764824073 {
        // mainnet
        208
    } else {
        // testnet
        74
    };
    let byron_epoch_length = 10 * byron.protocol_consts.k;
    let byron_slots = byron_epoch_length * shelley_transition_epoch;
    let shelley_slots = current_slot - byron_slots;
    let shelley_slot_in_epoch = shelley_slots % shelley.epoch_length;
    current_slot - shelley_slot_in_epoch
}

pub(crate) fn calculate_leader_logs(db_path: &PathBuf, byron_genesis: &PathBuf, shelley_genesis: &PathBuf, ledger_state: &PathBuf, ledger_set: &LedgerSet, pool_id: &String) {
    let db = Connection::open(db_path).unwrap();

    match read_byron_genesis(byron_genesis) {
        Ok(byron) => {
            println!("{:?}", byron);
            match read_shelley_genesis(shelley_genesis) {
                Ok(shelley) => {
                    println!("{:?}", shelley);
                    match calculate_ledger_state_sigma_and_d(ledger_state, ledger_set, pool_id) {
                        Ok((sigma, decentralization_param)) => {
                            println!("sigma: {:?}", sigma);
                            println!("decentralization_param: {:?}", decentralization_param);
                            let tip_slot_number = get_tip_slot_number(&db).unwrap();
                            println!("tip_slot_number: {}", tip_slot_number);
                            // pretend we're on a different slot number if we want to calculate past or future epochs.
                            let additional_slots: i64 = match ledger_set {
                                LedgerSet::Mark => { shelley.epoch_length }
                                LedgerSet::Set => { 0 }
                                LedgerSet::Go => { -shelley.epoch_length }
                            };
                            let first_slot_of_epoch = get_first_slot_of_epoch(&byron, &shelley, tip_slot_number + additional_slots);
                            let first_slot_of_prev_epoch = first_slot_of_epoch - shelley.epoch_length;
                            println!("first_slot_of_epoch: {}", first_slot_of_epoch);
                            println!("first_slot_of_prev_epoch: {}", first_slot_of_prev_epoch);
                            let stability_window = ((3 * byron.protocol_consts.k) as f64 / shelley.active_slots_coeff).ceil() as i64;
                            let stability_window_start = first_slot_of_epoch - stability_window;
                            println!("stability_window: {}", stability_window);
                            println!("stability_window_start: {}", stability_window_start);
                            let eta_v = get_eta_v_before_slot(&db, stability_window_start).unwrap();
                            println!("eta_v: {}", eta_v);
                        }
                        Err(error) => { handle_error(error) }
                    }
                }
                Err(error) => { handle_error(error) }
            }
        }
        Err(error) => { handle_error(error) }
    }

    match db.close() {
        Err(error) => {
            println!("db close error: {}", error.1);
        }
        _ => {}
    }
}

fn handle_error<T: Display>(error_message: T) {
    println!("{{\n\
            \x20\"status\": \"error\",\n\
            \x20\"errorMessage\": \"{}\"\n\
            }}", error_message);
}