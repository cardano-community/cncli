use pallas_crypto::hash::Hash;
use thiserror::Error;

use crate::nodeclient::sync::BlockHeader;

pub(crate) mod redb;
pub(crate) mod sqlite;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Redb error: {0}")]
    Redb(#[from] redb::Error),

    #[error("Sqlite error: {0}")]
    Sqlite(#[from] sqlite::Error),

    #[error("rusqlite error: {0}")]
    Rusqlite(#[from] rusqlite::Error),

    #[error("Blockstore error: {0}")]
    Blockstore(String),
}

pub(crate) struct Block {
    pub(crate) block_number: u64,
    pub(crate) slot_number: u64,
    pub(crate) hash: String,
    pub(crate) prev_hash: String,
    pub(crate) pool_id: String,
    pub(crate) leader_vrf: String,
    pub(crate) orphaned: bool,
}

pub(crate) trait BlockStore {
    fn save_block(&mut self, pending_blocks: &mut Vec<BlockHeader>, shelley_genesis_hash: &str) -> Result<(), Error>;
    fn load_blocks(&mut self) -> Result<Vec<(u64, Vec<u8>)>, Error>;
    fn find_block_by_hash(&mut self, hash_start: &str) -> Result<Option<Block>, Error>;
    fn get_tip_slot_number(&mut self) -> Result<u64, Error>;
    fn get_eta_v_before_slot(&mut self, slot_number: u64) -> Result<Hash<32>, Error>;
    fn get_prev_hash_before_slot(&mut self, slot_number: u64) -> Result<Hash<32>, Error>;
    fn save_slots(&mut self, epoch: u64, pool_id: &str, slot_qty: u64, slots: &str, hash: &str) -> Result<(), Error>;

    /// Get the number of slots and the hash from the block store for the epoch and pool_id
    fn get_current_slots(&mut self, epoch: u64, pool_id: &str) -> Result<(u64, String), Error>;

    /// Get the previous slots list raw data String from the block store for the epoch and pool_id
    fn get_previous_slots(&mut self, epoch: u64, pool_id: &str) -> Result<Option<String>, Error>;
}
