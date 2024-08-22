use crate::nodeclient::blockstore;
use crate::nodeclient::blockstore::{Block, BlockStore};
use crate::nodeclient::sync::BlockHeader;
use pallas_crypto::hash::{Hash, Hasher};
use pallas_crypto::nonce::generate_rolling_nonce;
use redb::{
    Builder, Database, MultimapTableDefinition, MultimapValue, ReadableMultimapTable, ReadableTable, RepairSession,
    TableDefinition, TypeName, Value,
};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use thiserror::Error;
use tracing::info;
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Redb error: {0}")]
    Redb(#[from] redb::Error),

    #[error("Redb db error: {0}")]
    RedbDb(#[from] redb::DatabaseError),

    #[error("Redb commit error: {0}")]
    RedbCommit(#[from] redb::CommitError),

    #[error("Redb transaction error: {0}")]
    RedbTransaction(#[from] redb::TransactionError),

    #[error("Redb table error: {0}")]
    RedbTable(#[from] redb::TableError),

    #[error("Redb storage error: {0}")]
    RedbStorage(#[from] redb::StorageError),

    #[error("FromHex error: {0}")]
    FromHex(#[from] hex::FromHexError),

    #[error("Data not found")]
    DataNotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChainRecord {
    block_number: u64,
    slot_number: u64,
    hash: Vec<u8>,
    prev_hash: Vec<u8>,
    pool_id: Vec<u8>,
    eta_v: Vec<u8>,
    node_vkey: Vec<u8>,
    node_vrf_vkey: Vec<u8>,
    block_vrf_0: Vec<u8>,
    block_vrf_1: Vec<u8>,
    eta_vrf_0: Vec<u8>,
    eta_vrf_1: Vec<u8>,
    leader_vrf_0: Vec<u8>,
    leader_vrf_1: Vec<u8>,
    block_size: u64,
    block_body_hash: Vec<u8>,
    pool_opcert: Vec<u8>,
    unknown_0: u64,
    unknown_1: u64,
    unknown_2: Vec<u8>,
    protocol_major_version: u64,
    protocol_minor_version: u64,
    orphaned: bool,
}

impl Value for ChainRecord {
    type SelfType<'a> = Self;
    type AsBytes<'a> = Vec<u8>
    where
        Self: 'a;

    fn fixed_width() -> Option<usize> {
        // dynamic sized object. not fixed width
        None
    }

    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a,
    {
        bincode::deserialize(data).unwrap()
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a>
    where
        Self: 'a,
        Self: 'b,
    {
        bincode::serialize(value).unwrap()
    }

    fn type_name() -> TypeName {
        TypeName::new(stringify!(ChainRecord))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlotsRecord {
    epoch: u64,
    pool_id: Vec<u8>,
    slot_qty: u64,
    slots: String,
    hash: Vec<u8>,
}

impl Value for SlotsRecord {
    type SelfType<'a> = Self;
    type AsBytes<'a> = Vec<u8>
    where
        Self: 'a;

    fn fixed_width() -> Option<usize> {
        // dynamic sized object. not fixed width
        None
    }

    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a,
    {
        bincode::deserialize(data).unwrap()
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a>
    where
        Self: 'a,
        Self: 'b,
    {
        bincode::serialize(value).unwrap()
    }

    fn type_name() -> TypeName {
        TypeName::new(stringify!(SlotsRecord))
    }
}

// magic number must be set to the ASCII letters 'redb' followed by 0x1A, 0x0A, 0xA9, 0x0D, 0x0A.
// This sequence is inspired by the PNG magic number.
const MAGIC_NUMBER: &[u8; 9] = b"redb\x1A\x0A\xA9\x0D\x0A";

const VERSION_TABLE: TableDefinition<&str, u16> = TableDefinition::new("version");
const CHAIN_TABLE: TableDefinition<u128, ChainRecord> = TableDefinition::new("chain");
const CHAIN_TABLE_SLOT_INDEX: MultimapTableDefinition<u64, u128> = MultimapTableDefinition::new("chain_slot_index");
const CHAIN_TABLE_HASH_INDEX: MultimapTableDefinition<&[u8], u128> = MultimapTableDefinition::new("chain_hash_index");
const SLOTS_TABLE: TableDefinition<u128, SlotsRecord> = TableDefinition::new("slots");
const SLOTS_TABLE_POOL_ID_EPOCH_INDEX: TableDefinition<&[u8], u128> = TableDefinition::new("slots_pool_id_epoch_index");

pub(crate) fn is_redb_database(db_path: &Path) -> Result<bool, Error> {
    let mut file = std::fs::File::open(db_path)?;
    let mut magic_number = [0u8; 9];
    file.read_exact(&mut magic_number)?;
    Ok(&magic_number == MAGIC_NUMBER)
}

pub struct RedbBlockStore {
    db: Database,
}

impl RedbBlockStore {
    const DB_VERSION: u16 = 1;

    pub fn new(db_path: &Path) -> Result<Self, Error> {
        let db = Builder::new()
            .set_repair_callback(Self::repair_callback)
            .create(db_path)?;
        Self::migrate(&db)?;
        Ok(Self { db })
    }

    pub fn repair_callback(session: &mut RepairSession) {
        let progress = session.progress();
        info!("Redb Repair progress: {:?}", progress);
    }

    fn migrate(db: &Database) -> Result<(), Error> {
        let read_tx = db.begin_read()?;
        let current_version = match read_tx.open_table(VERSION_TABLE) {
            Ok(version_table) => match version_table.get("version")? {
                Some(version) => version.value(),
                None => 0,
            },
            Err(_) => 0,
        };

        if current_version < Self::DB_VERSION {
            // Do migration
            let write_tx = db.begin_write()?;
            {
                let mut version_table = write_tx.open_table(VERSION_TABLE)?;
                info!("Migrating database from version 0 to 1");
                version_table.insert("version", Self::DB_VERSION)?;
                // create the chain table if it doesn't exist
                write_tx.open_table(CHAIN_TABLE)?;
                write_tx.open_multimap_table(CHAIN_TABLE_SLOT_INDEX)?;
                write_tx.open_multimap_table(CHAIN_TABLE_HASH_INDEX)?;
                // create the slots table if it doesn't exist
                write_tx.open_table(SLOTS_TABLE)?;
                write_tx.open_table(SLOTS_TABLE_POOL_ID_EPOCH_INDEX)?;
            }
            write_tx.commit()?;
        }

        Ok(())
    }

    fn redb_save_block(
        &mut self,
        pending_blocks: &mut Vec<BlockHeader>,
        shelley_genesis_hash: &str,
    ) -> Result<(), Error> {
        let first_pending_block_number = pending_blocks.first().unwrap().block_number;

        let write_tx = self.db.begin_write()?;
        {
            // get the last block eta_v (nonce) in the db
            let mut chain_table = write_tx.open_table(CHAIN_TABLE)?;
            let mut chain_table_slot_index = write_tx.open_multimap_table(CHAIN_TABLE_SLOT_INDEX)?;
            let mut chain_table_hash_index = write_tx.open_multimap_table(CHAIN_TABLE_HASH_INDEX)?;
            let mut chain_iter = chain_table.iter()?;
            let mut prev_eta_v: Hash<32> = shelley_genesis_hash.parse()?;
            let mut to_update: Vec<(u128, ChainRecord)> = Vec::new();

            while let Some(chain_record) = chain_iter.next_back() {
                let (key, chain_record) = chain_record?;
                let chain_record: ChainRecord = chain_record.value();
                if chain_record.orphaned {
                    continue;
                }
                if chain_record.block_number >= first_pending_block_number && !chain_record.orphaned {
                    // set it to orphaned
                    to_update.push((
                        key.value(),
                        ChainRecord {
                            orphaned: true,
                            ..chain_record.clone()
                        },
                    ));
                    continue;
                }
                prev_eta_v = Hash::from(chain_record.eta_v.as_slice());
                // sanity check
                assert_eq!(
                    chain_record.block_number,
                    first_pending_block_number - 1,
                    "block_number: {}, first_pending_block_number: {}",
                    chain_record.block_number,
                    first_pending_block_number
                );
                break;
            }
            for (key, chain_record) in to_update {
                chain_table.insert(key, chain_record)?;
            }

            // save the pending blocks
            for block in pending_blocks.drain(..) {
                let key = Uuid::now_v7().as_u128();

                // blake2b 224 of node_vkey is the pool_id
                let pool_id = Hasher::<224>::hash(block.node_vkey.as_slice());

                // calculate rolling nonce (eta_v)
                let eta_v = generate_rolling_nonce(prev_eta_v, &block.eta_vrf_0);

                let chain_record = ChainRecord {
                    block_number: block.block_number,
                    slot_number: block.slot_number,
                    hash: block.hash.clone(),
                    prev_hash: block.prev_hash.clone(),
                    pool_id: pool_id.to_vec(),
                    eta_v: eta_v.to_vec(),
                    node_vkey: block.node_vkey.clone(),
                    node_vrf_vkey: block.node_vrf_vkey.clone(),
                    block_vrf_0: block.block_vrf_0.clone(),
                    block_vrf_1: block.block_vrf_1.clone(),
                    eta_vrf_0: block.eta_vrf_0.clone(),
                    eta_vrf_1: block.eta_vrf_1.clone(),
                    leader_vrf_0: block.leader_vrf_0.clone(),
                    leader_vrf_1: block.leader_vrf_1.clone(),
                    block_size: block.block_size,
                    block_body_hash: block.block_body_hash.clone(),
                    pool_opcert: block.pool_opcert.clone(),
                    unknown_0: block.unknown_0,
                    unknown_1: block.unknown_1,
                    unknown_2: block.unknown_2.clone(),
                    protocol_major_version: block.protocol_major_version,
                    protocol_minor_version: block.protocol_minor_version,
                    orphaned: false,
                };
                chain_table.insert(key, chain_record)?;
                chain_table_slot_index.insert(block.slot_number, key)?;
                chain_table_hash_index.insert(block.hash.as_slice(), key)?;

                prev_eta_v = eta_v;
            }
        }
        write_tx.commit()?;

        Ok(())
    }

    fn redb_load_blocks(&mut self) -> Result<Vec<(u64, Vec<u8>)>, Error> {
        let read_tx = self.db.begin_read()?;
        // get slot_number and hash from chain table ordering by slot_number descending where orphaned is false
        // limit the result to 33 records
        let chain_table = read_tx.open_table(CHAIN_TABLE)?;
        let mut chain_iter = chain_table.iter()?;
        let mut blocks: Vec<(u64, Vec<u8>)> = Vec::new();
        while let Some(record) = chain_iter.next_back() {
            let (_, chain_record) = record?;
            let chain_record: ChainRecord = chain_record.value();
            if chain_record.orphaned {
                continue;
            }
            let slot_number = chain_record.slot_number;
            let hash = chain_record.hash.clone();
            blocks.push((slot_number, hash));
            if blocks.len() >= 33 {
                break;
            }
        }

        Ok(blocks)
    }

    fn redb_find_block_by_hash(&mut self, hash_start: &str) -> Result<Option<Block>, Error> {
        let read_tx = self.db.begin_read()?;
        let chain_table = read_tx.open_table(CHAIN_TABLE)?;
        let mut chain_iter = chain_table.iter()?;
        while let Some(record) = chain_iter.next_back() {
            let (_, chain_record) = record?;
            let chain_record: ChainRecord = chain_record.value();
            if hex::encode(&chain_record.hash).starts_with(hash_start) {
                let block = Block {
                    block_number: chain_record.block_number,
                    slot_number: chain_record.slot_number,
                    hash: hex::encode(&chain_record.hash),
                    prev_hash: hex::encode(&chain_record.prev_hash),
                    pool_id: hex::encode(&chain_record.pool_id),
                    leader_vrf: hex::encode(&chain_record.leader_vrf_0),
                    orphaned: chain_record.orphaned,
                };
                return Ok(Some(block));
            }
        }

        Ok(None)
    }

    fn redb_get_tip_slot_number(&mut self) -> Result<u64, Error> {
        let read_tx = self.db.begin_read()?;
        let chain_table_slot_index = read_tx.open_multimap_table(CHAIN_TABLE_SLOT_INDEX)?;
        let mut iter = chain_table_slot_index.iter()?;
        if let Some(result) = iter.next_back() {
            let (slot_number, _) = result?;
            return Ok(slot_number.value());
        }
        Ok(0)
    }

    fn redb_get_eta_v_before_slot(&mut self, slot_number: u64) -> Result<Hash<32>, Error> {
        let read_tx = self.db.begin_read()?;
        let chain_table_slot_index = read_tx.open_multimap_table(CHAIN_TABLE_SLOT_INDEX)?;
        let chain_table = read_tx.open_table(CHAIN_TABLE)?;
        for slot_number in (0..slot_number).rev() {
            let mut chain_keys: MultimapValue<u128> = match chain_table_slot_index.get(slot_number) {
                Ok(keys) => {
                    if !keys.is_empty() {
                        keys
                    } else {
                        continue;
                    }
                }
                Err(_) => continue,
            };
            let eta_v: Option<Hash<32>> = chain_keys.find_map(|key| {
                let key = key.ok()?;
                let chain_record: ChainRecord = chain_table.get(key.value()).ok()??.value();
                if !chain_record.orphaned {
                    Some(Hash::<32>::from(chain_record.eta_v.as_slice()))
                } else {
                    None
                }
            });
            if let Some(eta_v) = eta_v {
                return Ok(eta_v);
            }
        }

        Err(Error::DataNotFound)
    }

    fn redb_get_prev_hash_before_slot(&mut self, slot_number: u64) -> Result<Hash<32>, Error> {
        let read_tx = self.db.begin_read()?;
        let chain_table_slot_index = read_tx.open_multimap_table(CHAIN_TABLE_SLOT_INDEX)?;
        let chain_table = read_tx.open_table(CHAIN_TABLE)?;
        for slot_number in (0..slot_number).rev() {
            let mut chain_keys: MultimapValue<u128> = match chain_table_slot_index.get(slot_number) {
                Ok(keys) => {
                    if !keys.is_empty() {
                        keys
                    } else {
                        continue;
                    }
                }
                Err(_) => continue,
            };
            let prev_hash: Option<Hash<32>> = chain_keys.find_map(|key| {
                let key = key.ok()?.value();
                let chain_record: ChainRecord = chain_table.get(key).ok()??.value();
                if !chain_record.orphaned {
                    Some(Hash::<32>::from(chain_record.prev_hash.as_slice()))
                } else {
                    None
                }
            });
            if let Some(prev_hash) = prev_hash {
                return Ok(prev_hash);
            }
        }

        Err(Error::DataNotFound)
    }

    fn redb_save_slots(
        &mut self,
        epoch: u64,
        pool_id: &str,
        slot_qty: u64,
        slots: &str,
        hash: &str,
    ) -> Result<(), Error> {
        // See if record exists already
        let mut hasher = Hasher::<224>::new();
        hasher.input(&epoch.to_be_bytes());
        hasher.input(hex::decode(pool_id)?.as_slice());
        let index_key = hasher.finalize();

        let read_tx = self.db.begin_read()?;
        let slots_key = {
            let slots_table_pool_id_epoch_index = read_tx.open_table(SLOTS_TABLE_POOL_ID_EPOCH_INDEX)?;
            slots_table_pool_id_epoch_index
                .get(index_key.as_slice())?
                .map(|key| key.value())
        };

        let write_tx = self.db.begin_write()?;
        {
            let mut slots_table = write_tx.open_table(SLOTS_TABLE)?;
            match slots_key {
                Some(key) => {
                    // Update existing record
                    let mut slots_record: SlotsRecord = slots_table
                        .get(key)?
                        .map(|record| record.value())
                        .ok_or(Error::DataNotFound)?;
                    slots_record.slot_qty = slot_qty;
                    slots_record.slots = slots.to_string();
                    slots_record.hash = hex::decode(hash)?;
                    slots_table.insert(key, slots_record)?;
                }
                None => {
                    // Add new record and index
                    let mut slots_table_pool_id_epoch_index = write_tx.open_table(SLOTS_TABLE_POOL_ID_EPOCH_INDEX)?;
                    let key = Uuid::now_v7().as_u128();
                    let slots_record = SlotsRecord {
                        epoch,
                        pool_id: hex::decode(pool_id)?,
                        slot_qty,
                        slots: slots.to_string(),
                        hash: hex::decode(hash)?,
                    };
                    slots_table.insert(key, slots_record)?;
                    slots_table_pool_id_epoch_index.insert(index_key.as_slice(), key)?;
                }
            }
        }
        write_tx.commit()?;

        Ok(())
    }

    fn redb_get_current_slots(&mut self, epoch: u64, pool_id: &str) -> Result<(u64, String), Error> {
        let mut hasher = Hasher::<224>::new();
        hasher.input(&epoch.to_be_bytes());
        hasher.input(hex::decode(pool_id)?.as_slice());
        let index_key = hasher.finalize();

        let read_tx = self.db.begin_read()?;
        let slots_table_pool_id_epoch_index = read_tx.open_table(SLOTS_TABLE_POOL_ID_EPOCH_INDEX)?;
        let slots_key = slots_table_pool_id_epoch_index
            .get(index_key.as_slice())?
            .map(|key| key.value())
            .ok_or(Error::DataNotFound)?;

        let slots_table = read_tx.open_table(SLOTS_TABLE)?;
        let slots_record = slots_table
            .get(slots_key)?
            .map(|record| record.value())
            .ok_or(Error::DataNotFound)?;

        Ok((slots_record.slot_qty, hex::encode(slots_record.hash)))
    }

    fn redb_get_previous_slots(&mut self, epoch: u64, pool_id: &str) -> Result<Option<String>, Error> {
        let mut hasher = Hasher::<224>::new();
        hasher.input(&epoch.to_be_bytes());
        hasher.input(hex::decode(pool_id)?.as_slice());
        let index_key = hasher.finalize();

        let read_tx = self.db.begin_read()?;
        let slots_table_pool_id_epoch_index = read_tx.open_table(SLOTS_TABLE_POOL_ID_EPOCH_INDEX)?;
        if let Some(slots_key) = slots_table_pool_id_epoch_index
            .get(index_key.as_slice())?
            .map(|key| key.value())
        {
            let slots_table = read_tx.open_table(SLOTS_TABLE)?;
            let slots_record = slots_table
                .get(slots_key)?
                .map(|record| record.value())
                .ok_or(Error::DataNotFound)?;
            Ok(Some(slots_record.slots))
        } else {
            Ok(None)
        }
    }
}

impl BlockStore for RedbBlockStore {
    fn save_block(
        &mut self,
        pending_blocks: &mut Vec<BlockHeader>,
        shelley_genesis_hash: &str,
    ) -> Result<(), blockstore::Error> {
        Ok(self.redb_save_block(pending_blocks, shelley_genesis_hash)?)
    }

    fn load_blocks(&mut self) -> Result<Vec<(u64, Vec<u8>)>, blockstore::Error> {
        Ok(self.redb_load_blocks()?)
    }

    fn find_block_by_hash(&mut self, hash_start: &str) -> Result<Option<Block>, blockstore::Error> {
        Ok(self.redb_find_block_by_hash(hash_start)?)
    }

    fn get_tip_slot_number(&mut self) -> Result<u64, blockstore::Error> {
        Ok(self.redb_get_tip_slot_number()?)
    }

    fn get_eta_v_before_slot(&mut self, slot_number: u64) -> Result<Hash<32>, blockstore::Error> {
        Ok(self.redb_get_eta_v_before_slot(slot_number)?)
    }

    fn get_prev_hash_before_slot(&mut self, slot_number: u64) -> Result<Hash<32>, blockstore::Error> {
        Ok(self.redb_get_prev_hash_before_slot(slot_number)?)
    }

    fn save_slots(
        &mut self,
        epoch: u64,
        pool_id: &str,
        slot_qty: u64,
        slots: &str,
        hash: &str,
    ) -> Result<(), blockstore::Error> {
        Ok(self.redb_save_slots(epoch, pool_id, slot_qty, slots, hash)?)
    }

    fn get_current_slots(&mut self, epoch: u64, pool_id: &str) -> Result<(u64, String), blockstore::Error> {
        Ok(self.redb_get_current_slots(epoch, pool_id)?)
    }

    fn get_previous_slots(&mut self, epoch: u64, pool_id: &str) -> Result<Option<String>, blockstore::Error> {
        Ok(self.redb_get_previous_slots(epoch, pool_id)?)
    }
}
