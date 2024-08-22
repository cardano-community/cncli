use std::fs::File;
use std::io::BufReader;
use std::ops::Sub;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::nodeclient::blockstore;
use crate::nodeclient::blockstore::{Block, BlockStore, Error};
use crate::nodeclient::sync::BlockHeader;
use crate::APP_USER_AGENT;
use chrono::{SecondsFormat, Utc};
use pallas_crypto::hash::Hash;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{error, info};

pub(crate) fn get_pooltool_config(config: &Path) -> PooltoolConfig {
    let buf = BufReader::new(File::open(config).unwrap());
    serde_json::from_reader(buf).unwrap()
}

#[derive(Debug, Deserialize)]
pub(crate) struct PooltoolConfig {
    pub(crate) api_key: String,
    pub(crate) pools: Vec<Pool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Pool {
    pub(crate) name: String,
    pub(crate) pool_id: String,
    pub(crate) host: String,
    pub(crate) port: u16,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PooltoolStats0 {
    api_key: String,
    pool_id: String,
    data: PooltoolData0,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PooltoolData0 {
    node_id: String,
    version: String,
    at: String,
    block_no: u64,
    slot_no: u64,
    block_hash: String,
    parent_hash: String,
    leader_vrf: String,
    leader_vrf_proof: String,
    node_v_key: String,
    protocol_major_version: u64,
    protocol_minor_version: u64,
    platform: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PooltoolStats1 {
    api_key: String,
    pool_id: String,
    data: PooltoolData1,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PooltoolData1 {
    node_id: String,
    version: String,
    at: String,
    block_no: u64,
    slot_no: u64,
    block_hash: String,
    parent_hash: String,
    leader_vrf: String,
    block_vrf: String,
    block_vrf_proof: String,
    node_v_key: String,
    protocol_major_version: u64,
    protocol_minor_version: u64,
    platform: String,
}

pub struct PoolToolNotifier {
    pub pool_name: String,
    pub pool_id: String,
    pub api_key: String,
    pub cardano_node_path: PathBuf,
    pub last_node_version_time: Instant,
    pub node_version: String,
}

impl Default for PoolToolNotifier {
    fn default() -> Self {
        PoolToolNotifier {
            pool_name: String::new(),
            pool_id: String::new(),
            api_key: String::new(),
            cardano_node_path: PathBuf::new(),
            last_node_version_time: Instant::now().sub(Duration::from_secs(7200)), // 2 hours ago
            node_version: String::new(),
        }
    }
}

impl PoolToolNotifier {
    pub fn send_to_pooltool(&mut self, header: &BlockHeader) {
        if self.last_node_version_time.elapsed() > Duration::from_secs(3600) {
            // Our node version is outdated. Make a call to update it.
            match Command::new(&self.cardano_node_path)
                .arg("--version")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .output()
            {
                Ok(output) => {
                    let version_string = String::from_utf8_lossy(&output.stdout);
                    let cap = Regex::new("cardano-node (\\d+\\.\\d+\\.\\d+) .*\ngit rev ([a-f\\d]{5}).*")
                        .unwrap()
                        .captures(&version_string)
                        .unwrap();
                    self.node_version = format!(
                        "{}:{}",
                        cap.get(1).map_or("", |m| m.as_str()),
                        cap.get(2).map_or("", |m| m.as_str())
                    );
                    info!("Checking cardano-node version: {}", &self.node_version);
                    self.last_node_version_time = Instant::now();
                }
                Err(err) => {
                    panic!("Error getting cardano-node version: {}", err)
                }
            }
        }

        match reqwest::blocking::Client::builder().user_agent(APP_USER_AGENT).build() {
            Ok(client) => {
                let pooltool_result = if header.block_vrf_0.is_empty() {
                    client
                        .post("https://api.pooltool.io/v0/sendstats")
                        .body(
                            serde_json::ser::to_string(&PooltoolStats0 {
                                api_key: self.api_key.clone(),
                                pool_id: self.pool_id.clone(),
                                data: PooltoolData0 {
                                    node_id: "".to_string(),
                                    version: self.node_version.clone(),
                                    at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                                    block_no: header.block_number,
                                    slot_no: header.slot_number,
                                    block_hash: hex::encode(&header.hash),
                                    parent_hash: hex::encode(&header.prev_hash),
                                    leader_vrf: hex::encode(&header.leader_vrf_0),
                                    leader_vrf_proof: hex::encode(&header.leader_vrf_1),
                                    protocol_major_version: header.protocol_major_version,
                                    protocol_minor_version: header.protocol_minor_version,
                                    node_v_key: hex::encode(&header.node_vkey),
                                    platform: "cncli".to_string(),
                                },
                            })
                            .unwrap(),
                        )
                        .send()
                } else {
                    client
                        .post("https://api.pooltool.io/v1/sendstats")
                        .body(
                            serde_json::ser::to_string(&PooltoolStats1 {
                                api_key: self.api_key.clone(),
                                pool_id: self.pool_id.clone(),
                                data: PooltoolData1 {
                                    node_id: "".to_string(),
                                    version: self.node_version.clone(),
                                    at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                                    block_no: header.block_number,
                                    slot_no: header.slot_number,
                                    block_hash: hex::encode(&header.hash),
                                    parent_hash: hex::encode(&header.prev_hash),
                                    leader_vrf: hex::encode(&header.leader_vrf_0),
                                    block_vrf: hex::encode(&header.block_vrf_0),
                                    block_vrf_proof: hex::encode(&header.block_vrf_1),
                                    node_v_key: hex::encode(&header.node_vkey),
                                    protocol_major_version: header.protocol_major_version,
                                    protocol_minor_version: header.protocol_minor_version,
                                    platform: "cncli".to_string(),
                                },
                            })
                            .unwrap(),
                        )
                        .send()
                };

                match pooltool_result {
                    Ok(response) => match response.text() {
                        Ok(text) => {
                            info!(
                                "Pooltool ({}, {}): ({}, {}), json: {}",
                                &self.pool_name,
                                &self.pool_id[..8],
                                &header.block_number,
                                hex::encode(&header.hash[..8]),
                                text
                            );
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
}

impl BlockStore for PoolToolNotifier {
    fn save_block(
        &mut self,
        pending_blocks: &mut Vec<BlockHeader>,
        _shelley_genesis_hash: &str,
    ) -> Result<(), blockstore::Error> {
        self.send_to_pooltool(pending_blocks.last().unwrap());
        Ok(())
    }

    fn load_blocks(&mut self) -> Result<Vec<(u64, Vec<u8>)>, Error> {
        Err(Error::Blockstore("Not implemented".to_string()))
    }

    fn find_block_by_hash(&mut self, _hash_start: &str) -> Result<Option<Block>, Error> {
        Err(Error::Blockstore("Not implemented".to_string()))
    }

    fn get_tip_slot_number(&mut self) -> Result<u64, Error> {
        Err(Error::Blockstore("Not implemented".to_string()))
    }

    fn get_eta_v_before_slot(&mut self, _slot_number: u64) -> Result<Hash<32>, Error> {
        Err(Error::Blockstore("Not implemented".to_string()))
    }

    fn get_prev_hash_before_slot(&mut self, _slot_number: u64) -> Result<Hash<32>, Error> {
        Err(Error::Blockstore("Not implemented".to_string()))
    }

    fn save_slots(
        &mut self,
        _epoch: u64,
        _pool_id: &str,
        _slot_qty: u64,
        _slots: &str,
        _hash: &str,
    ) -> Result<(), Error> {
        Err(Error::Blockstore("Not implemented".to_string()))
    }

    fn get_current_slots(&mut self, _epoch: u64, _pool_id: &str) -> Result<(u64, String), Error> {
        Err(Error::Blockstore("Not implemented".to_string()))
    }

    fn get_previous_slots(&mut self, _epoch: u64, _pool_id: &str) -> Result<Option<String>, Error> {
        Err(Error::Blockstore("Not implemented".to_string()))
    }
}
