use std::fs::File;
use std::io::{BufReader, Error};
use std::path::PathBuf;

use serde::Deserialize;
use serde::export::fmt::Display;
use serde_aux::prelude::deserialize_number_from_string;

use crate::nodeclient::LedgerSet;

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
    k: u64
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
    active_slots_coeff: f32,
    network_magic: u32,
    slot_length: u64,
    epoch_length: u64,
}

fn read_byron_genesis(byron_genesis: &PathBuf) -> Result<ByronGenesis, Error> {
    let buf = BufReader::new(File::open(byron_genesis)?);
    Ok(serde_json::from_reader(buf)?)
}

fn read_shelley_genesis(shelley_genesis: &PathBuf) -> Result<ShelleyGenesis, Error> {
    let buf = BufReader::new(File::open(shelley_genesis)?);
    Ok(serde_json::from_reader(buf)?)
}

pub(crate) fn calculate_leader_logs(db: &PathBuf, byron_genesis: &PathBuf, shelley_genesis: &PathBuf, ledger_state: &PathBuf, ledger_set: &LedgerSet, pool_id: &String, epoch: &u64) {
    match read_byron_genesis(byron_genesis) {
        Ok(byron) => {
            println!("{:?}", byron);
            match read_shelley_genesis(shelley_genesis) {
                Ok(shelley) => {
                    println!("{:?}", shelley);
                }
                Err(error) => { handle_error(error) }
            }
        }
        Err(error) => { handle_error(error) }
    }
}

fn handle_error<T: Display>(error_message: T) {
    println!("{{\n\
            \x20\"status\": \"error\",\n\
            \x20\"errorMessage\": \"{}\"\n\
            }}", error_message);
}