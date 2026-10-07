use crate::nodeclient::blockstore;
use crate::nodeclient::blockstore::{Block, BlockStore};
use crate::nodeclient::sync::BlockHeader;
use pallas_crypto::hash::{Hash, Hasher};
use pallas_crypto::nonce::generate_rolling_nonce;
use redb::{
    Builder, Database, MultimapTableDefinition, MultimapValue, ReadableMultimapTable, ReadableTable, RepairSession,
    TableDefinition, TableHandle, TypeName, Value,
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
    Redb(Box<redb::Error>),

    #[error("Redb db error: {0}")]
    RedbDb(Box<redb::DatabaseError>),

    #[error("Redb commit error: {0}")]
    RedbCommit(Box<redb::CommitError>),

    #[error("Redb transaction error: {0}")]
    RedbTransaction(Box<redb::TransactionError>),

    #[error("Redb table error: {0}")]
    RedbTable(Box<redb::TableError>),

    #[error("Redb storage error: {0}")]
    RedbStorage(Box<redb::StorageError>),

    #[error("FromHex error: {0}")]
    FromHex(#[from] hex::FromHexError),

    #[error("Data not found")]
    DataNotFound,

    #[error("Data integrity error: {0}")]
    DataIntegrity(String),

    #[error("Unsupported schema version {found}; supported version is {supported}")]
    UnsupportedSchemaVersion { found: u16, supported: u16 },
}

impl From<redb::Error> for Error {
    fn from(err: redb::Error) -> Self {
        Error::Redb(Box::new(err))
    }
}

impl From<redb::DatabaseError> for Error {
    fn from(err: redb::DatabaseError) -> Self {
        Error::RedbDb(Box::new(err))
    }
}

impl From<redb::CommitError> for Error {
    fn from(err: redb::CommitError) -> Self {
        Error::RedbCommit(Box::new(err))
    }
}

impl From<redb::TransactionError> for Error {
    fn from(err: redb::TransactionError) -> Self {
        Error::RedbTransaction(Box::new(err))
    }
}

impl From<redb::TableError> for Error {
    fn from(err: redb::TableError) -> Self {
        Error::RedbTable(Box::new(err))
    }
}

impl From<redb::StorageError> for Error {
    fn from(err: redb::StorageError) -> Self {
        Error::RedbStorage(Box::new(err))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, bincode::Encode, bincode::Decode)]
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
    type AsBytes<'a>
        = Vec<u8>
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
        bincode::decode_from_slice(data, bincode::config::legacy()).unwrap().0
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a>
    where
        Self: 'a,
        Self: 'b,
    {
        bincode::encode_to_vec(value, bincode::config::legacy()).unwrap()
    }

    fn type_name() -> TypeName {
        TypeName::new(stringify!(ChainRecord))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, bincode::Encode, bincode::Decode)]
struct SlotsRecord {
    epoch: u64,
    pool_id: Vec<u8>,
    slot_qty: u64,
    slots: String,
    hash: Vec<u8>,
}

impl Value for SlotsRecord {
    type SelfType<'a> = Self;
    type AsBytes<'a>
        = Vec<u8>
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
        bincode::decode_from_slice(data, bincode::config::legacy()).unwrap().0
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a>
    where
        Self: 'a,
        Self: 'b,
    {
        bincode::encode_to_vec(value, bincode::config::legacy()).unwrap()
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
            Ok(version_table) => version_table
                .get("version")?
                .map(|version| version.value())
                .ok_or_else(|| Error::DataIntegrity("Schema version key is missing".into()))?,
            Err(redb::TableError::TableDoesNotExist(_)) => {
                let has_data_tables = read_tx.list_tables()?.any(|table| table.name() != "version")
                    || read_tx.list_multimap_tables()?.next().is_some();
                if has_data_tables {
                    return Err(Error::DataIntegrity(
                        "Existing data has no schema version metadata".into(),
                    ));
                }
                0
            }
            Err(error) => return Err(error.into()),
        };
        drop(read_tx);
        if current_version > Self::DB_VERSION {
            return Err(Error::UnsupportedSchemaVersion {
                found: current_version,
                supported: Self::DB_VERSION,
            });
        }

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

    fn redb_save_block(&mut self, pending_blocks: &[BlockHeader], shelley_genesis_hash: &str) -> Result<(), Error> {
        let Some(first) = pending_blocks.first() else {
            return Ok(());
        };
        let predecessor_number = first
            .block_number
            .checked_sub(1)
            .ok_or_else(|| Error::DataIntegrity("First block number must be nonzero".into()))?;
        let mut genesis = [0u8; 32];
        hex::decode_to_slice(shelley_genesis_hash, &mut genesis)
            .map_err(|_| Error::DataIntegrity("Shelley genesis hash must be 32 bytes of hex".into()))?;
        for pair in pending_blocks.windows(2) {
            let [previous, next] = pair else { unreachable!() };
            if previous.block_number.checked_add(1) != Some(next.block_number)
                || next.slot_number <= previous.slot_number
                || next.prev_hash != previous.hash
            {
                return Err(Error::DataIntegrity("Pending blocks are not a continuous chain".into()));
            }
        }
        let write_tx = self.db.begin_write()?;
        {
            let mut chain_table = write_tx.open_table(CHAIN_TABLE)?;
            let mut chain_table_slot_index = write_tx.open_multimap_table(CHAIN_TABLE_SLOT_INDEX)?;
            let mut chain_table_hash_index = write_tx.open_multimap_table(CHAIN_TABLE_HASH_INDEX)?;
            let mut predecessor = None;
            let mut has_active_chain = false;
            let mut to_update: Vec<(u128, ChainRecord)> = Vec::new();
            // Walk only the active suffix and predecessor, in chain (slot) order.
            let mut slots = chain_table_slot_index.range::<u64>(..)?;
            while let Some(entry) = slots.next_back() {
                let (slot, keys) = entry?;
                let mut active = None;
                for key in keys {
                    let key = key?.value();
                    let record = chain_table
                        .get(key)?
                        .ok_or_else(|| Error::DataIntegrity("Slot index references a missing block".into()))?
                        .value();
                    if record.slot_number != slot.value() {
                        return Err(Error::DataIntegrity("Slot index does not match block slot".into()));
                    }
                    if !record.orphaned && active.replace((key, record)).is_some() {
                        return Err(Error::DataIntegrity("Multiple active blocks at one slot".into()));
                    }
                }
                let Some((key, mut record)) = active else { continue };
                has_active_chain = true;
                if record.block_number < first.block_number {
                    if record.block_number == predecessor_number {
                        predecessor = Some(record);
                    }
                    break;
                }
                record.orphaned = true;
                to_update.push((key, record));
            }
            drop(slots);
            let mut prev_eta_v = match predecessor {
                Some(record) => {
                    if record.hash != first.prev_hash || record.slot_number >= first.slot_number {
                        return Err(Error::DataIntegrity(
                            "Pending batch does not match its active predecessor".into(),
                        ));
                    }
                    let nonce: [u8; 32] = record
                        .eta_v
                        .as_slice()
                        .try_into()
                        .map_err(|_| Error::DataIntegrity("Predecessor nonce must be 32 bytes".into()))?;
                    Hash::from(nonce)
                }
                None if !has_active_chain => Hash::from(genesis),
                None => return Err(Error::DataIntegrity("Active predecessor block is missing".into())),
            };
            for (key, record) in to_update {
                chain_table.insert(key, record)?;
            }

            for block in pending_blocks {
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
    fn active_at_slot(
        chain_table: &impl ReadableTable<u128, ChainRecord>,
        keys: MultimapValue<u128>,
        slot: u64,
    ) -> Result<Option<ChainRecord>, Error> {
        let mut active = None;
        for key in keys {
            let record = chain_table
                .get(key?.value())?
                .ok_or_else(|| Error::DataIntegrity("Slot index references a missing block".into()))?
                .value();
            if record.slot_number != slot {
                return Err(Error::DataIntegrity("Slot index does not match block slot".into()));
            }
            if !record.orphaned && active.replace(record).is_some() {
                return Err(Error::DataIntegrity("Multiple active blocks at one slot".into()));
            }
        }
        Ok(active)
    }

    fn redb_rollback_to(&mut self, point: &pallas_network::miniprotocols::Point) -> Result<(), Error> {
        use pallas_network::miniprotocols::Point;
        let write_tx = self.db.begin_write()?;
        {
            let mut chain_table = write_tx.open_table(CHAIN_TABLE)?;
            let target_number = match point {
                Point::Origin => None,
                Point::Specific(slot, hash) => {
                    let index = write_tx.open_multimap_table(CHAIN_TABLE_SLOT_INDEX)?;
                    let record = Self::active_at_slot(&chain_table, index.get(*slot)?, *slot)?
                        .filter(|record| record.hash.as_slice() == hash.as_slice())
                        .ok_or_else(|| Error::DataIntegrity("Rollback point not found in active chain".into()))?;
                    Some(record.block_number)
                }
            };
            let mut updates = Vec::new();
            for entry in chain_table.iter()? {
                let (key, record) = entry?;
                let mut record = record.value();
                if !record.orphaned && target_number.is_none_or(|number| record.block_number > number) {
                    record.orphaned = true;
                    updates.push((key.value(), record));
                }
            }
            for (key, record) in updates {
                chain_table.insert(key, record)?;
            }
        }
        write_tx.commit()?;
        Ok(())
    }

    fn redb_load_blocks(&mut self) -> Result<Vec<(u64, Vec<u8>)>, Error> {
        let read_tx = self.db.begin_read()?;
        let chain_table = read_tx.open_table(CHAIN_TABLE)?;
        let index = read_tx.open_multimap_table(CHAIN_TABLE_SLOT_INDEX)?;
        let mut chain_iter = index.range::<u64>(..)?;
        let mut blocks: Vec<(u64, Vec<u8>)> = Vec::new();
        while let Some(record) = chain_iter.next_back() {
            let (slot, keys) = record?;
            if let Some(record) = Self::active_at_slot(&chain_table, keys, slot.value())? {
                blocks.push((slot.value(), record.hash));
            }
            if blocks.len() >= 262145 {
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
        let chain_table = read_tx.open_table(CHAIN_TABLE)?;
        let index = read_tx.open_multimap_table(CHAIN_TABLE_SLOT_INDEX)?;
        let mut iter = index.range::<u64>(..)?;
        while let Some(entry) = iter.next_back() {
            let (slot, keys) = entry?;
            if Self::active_at_slot(&chain_table, keys, slot.value())?.is_some() {
                return Ok(slot.value());
            }
        }
        Ok(0)
    }

    fn redb_get_eta_v_before_slot(&mut self, slot_number: u64) -> Result<Hash<32>, Error> {
        let read_tx = self.db.begin_read()?;
        let chain_table_slot_index = read_tx.open_multimap_table(CHAIN_TABLE_SLOT_INDEX)?;
        let chain_table = read_tx.open_table(CHAIN_TABLE)?;
        let mut iter = chain_table_slot_index.range(..slot_number)?;
        while let Some(entry) = iter.next_back() {
            let (slot, keys) = entry?;
            if let Some(record) = Self::active_at_slot(&chain_table, keys, slot.value())? {
                let nonce: [u8; 32] = record
                    .eta_v
                    .as_slice()
                    .try_into()
                    .map_err(|_| Error::DataIntegrity("Rolling nonce must be 32 bytes".into()))?;
                return Ok(Hash::from(nonce));
            }
        }

        Err(Error::DataNotFound)
    }

    fn redb_get_prev_hash_before_slot(&mut self, slot_number: u64) -> Result<Hash<32>, Error> {
        let read_tx = self.db.begin_read()?;
        let chain_table_slot_index = read_tx.open_multimap_table(CHAIN_TABLE_SLOT_INDEX)?;
        let chain_table = read_tx.open_table(CHAIN_TABLE)?;
        let mut iter = chain_table_slot_index.range(..slot_number)?;
        while let Some(entry) = iter.next_back() {
            let (slot, keys) = entry?;
            if let Some(record) = Self::active_at_slot(&chain_table, keys, slot.value())? {
                let hash: [u8; 32] = record
                    .prev_hash
                    .as_slice()
                    .try_into()
                    .map_err(|_| Error::DataIntegrity("Previous hash must be 32 bytes".into()))?;
                return Ok(Hash::from(hash));
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
        pending_blocks: &[BlockHeader],
        shelley_genesis_hash: &str,
    ) -> Result<(), blockstore::Error> {
        Ok(self.redb_save_block(pending_blocks, shelley_genesis_hash)?)
    }

    fn rollback_to(&mut self, point: &pallas_network::miniprotocols::Point) -> Result<(), blockstore::Error> {
        Ok(self.redb_rollback_to(point)?)
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

#[cfg(test)]
mod tests {
    use super::*;
    use redb::{backends::FileBackend, StorageBackend};
    use std::io;
    use std::path::PathBuf;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("cncli-redb-{}", Uuid::now_v7()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> PathBuf {
            self.0.join("chain.redb")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn header(number: u64, slot: u64, hash: u8, parent: u8) -> BlockHeader {
        BlockHeader {
            block_number: number,
            slot_number: slot,
            hash: vec![hash; 32],
            prev_hash: vec![parent; 32],
            node_vkey: vec![0; 32],
            node_vrf_vkey: vec![0; 32],
            block_vrf_0: vec![],
            block_vrf_1: vec![],
            eta_vrf_0: vec![hash; 32],
            eta_vrf_1: vec![],
            leader_vrf_0: vec![],
            leader_vrf_1: vec![],
            block_size: 0,
            block_body_hash: vec![0; 32],
            pool_opcert: vec![],
            unknown_0: 0,
            unknown_1: 0,
            unknown_2: vec![],
            protocol_major_version: 0,
            protocol_minor_version: 0,
        }
    }

    fn active(store: &RedbBlockStore) -> Vec<(u64, Vec<u8>, Vec<u8>)> {
        let tx = store.db.begin_read().unwrap();
        let table = tx.open_table(CHAIN_TABLE).unwrap();
        let mut result = table
            .iter()
            .unwrap()
            .map(|row| {
                let (_, value) = row.unwrap();
                value.value()
            })
            .filter(|row| !row.orphaned)
            .map(|row| (row.block_number, row.hash, row.eta_v))
            .collect::<Vec<_>>();
        result.sort_by_key(|row| row.0);
        result
    }

    #[derive(Debug)]
    struct FailingBackend {
        inner: FileBackend,
        armed: Arc<AtomicBool>,
    }

    impl StorageBackend for FailingBackend {
        fn len(&self) -> io::Result<u64> {
            self.inner.len()
        }
        fn read(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
            self.inner.read(offset, len)
        }
        fn set_len(&self, len: u64) -> io::Result<()> {
            self.inner.set_len(len)
        }
        fn sync_data(&self, eventual: bool) -> io::Result<()> {
            self.inner.sync_data(eventual)
        }
        fn write(&self, offset: u64, data: &[u8]) -> io::Result<()> {
            if self.armed.load(Ordering::SeqCst) {
                return Err(io::Error::other("injected write failure"));
            }
            self.inner.write(offset, data)
        }
    }

    #[test]
    fn failed_write_retains_batch_and_committed_prefix() {
        let fixture = Fixture::new();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(fixture.path())
            .unwrap();
        let armed = Arc::new(AtomicBool::new(false));
        let db = Builder::new()
            .create_with_backend(FailingBackend {
                inner: FileBackend::new(file).unwrap(),
                armed: armed.clone(),
            })
            .unwrap();
        RedbBlockStore::migrate(&db).unwrap();
        let mut store = RedbBlockStore { db };
        let genesis = "00".repeat(32);
        let prefix = [header(1, 10, 1, 0), header(2, 20, 2, 1)];
        store.redb_save_block(&prefix, &genesis).unwrap();
        let before = active(&store);
        // Replacing the old suffix also proves orphaning rolls back on failure.
        let pending = vec![header(2, 30, 4, 1), header(3, 40, 5, 4)];
        let encoded = format!("{pending:?}");
        armed.store(true, Ordering::SeqCst);
        assert!(store.redb_save_block(&pending, &genesis).is_err());
        assert_eq!(format!("{pending:?}"), encoded);
        drop(store);
        armed.store(false, Ordering::SeqCst);
        let mut store = RedbBlockStore::new(&fixture.path()).unwrap();
        assert_eq!(active(&store), before);
        store.redb_save_block(&pending, &genesis).unwrap();
        let expected_nonce = generate_rolling_nonce(
            generate_rolling_nonce(Hash::from(before[0].2.as_slice()), &pending[0].eta_vrf_0),
            &pending[1].eta_vrf_0,
        );
        assert_eq!(
            active(&store),
            vec![
                before[0].clone(),
                (
                    2,
                    vec![4; 32],
                    generate_rolling_nonce(Hash::from(before[0].2.as_slice()), &pending[0].eta_vrf_0).to_vec()
                ),
                (3, vec![5; 32], expected_nonce.to_vec()),
            ]
        );
        assert!(
            store
                .redb_find_block_by_hash(&"02".repeat(32))
                .unwrap()
                .unwrap()
                .orphaned
        );
    }

    #[test]
    fn invalid_batches_do_not_mutate_chain() {
        let fixture = Fixture::new();
        let mut store = RedbBlockStore::new(&fixture.path()).unwrap();
        let genesis = "00".repeat(32);
        store.redb_save_block(&[], "not hex").unwrap();
        assert!(matches!(
            store.redb_save_block(&[header(0, 1, 1, 0)], &genesis),
            Err(Error::DataIntegrity(_))
        ));
        assert!(matches!(
            store.redb_save_block(&[header(1, 10, 1, 0)], "00"),
            Err(Error::DataIntegrity(_))
        ));
        store.redb_save_block(&[header(1, 10, 1, 0)], &genesis).unwrap();
        let before = active(&store);
        for pending in [
            vec![header(3, 30, 3, 2)],
            vec![header(2, 20, 2, 9)],
            vec![header(2, 10, 2, 1)],
            vec![header(2, 20, 2, 1), header(4, 30, 3, 2)],
            vec![header(2, 20, 2, 1), header(3, 20, 3, 2)],
            vec![header(2, 20, 2, 1), header(3, 30, 3, 9)],
            vec![header(u64::MAX, 20, 2, 1), header(1, 30, 3, 2)],
        ] {
            assert!(matches!(
                store.redb_save_block(&pending, &genesis),
                Err(Error::DataIntegrity(_))
            ));
            assert_eq!(active(&store), before);
        }
        let tx = store.db.begin_write().unwrap();
        {
            let mut table = tx.open_table(CHAIN_TABLE).unwrap();
            let (key, mut record) = {
                let row = table.iter().unwrap().next().unwrap().unwrap();
                (row.0.value(), row.1.value())
            };
            record.eta_v = vec![0; 31];
            table.insert(key, record).unwrap();
        }
        tx.commit().unwrap();
        let corrupt = active(&store);
        assert!(matches!(
            store.redb_save_block(&[header(2, 20, 2, 1)], &genesis),
            Err(Error::DataIntegrity(_))
        ));
        assert_eq!(active(&store), corrupt);
    }

    #[test]
    fn duplicate_active_slots_and_broken_index_fail_closed() {
        use pallas_network::miniprotocols::Point;
        for missing_record in [false, true] {
            let fixture = Fixture::new();
            let mut store = RedbBlockStore::new(&fixture.path()).unwrap();
            store.redb_save_block(&[header(1, 10, 1, 0)], &"00".repeat(32)).unwrap();
            let before = active(&store);
            let tx = store.db.begin_write().unwrap();
            {
                let mut table = tx.open_table(CHAIN_TABLE).unwrap();
                if !missing_record {
                    let record = table.iter().unwrap().next().unwrap().unwrap().1.value();
                    table.insert(0, record).unwrap();
                }
                tx.open_multimap_table(CHAIN_TABLE_SLOT_INDEX)
                    .unwrap()
                    .insert(10, 0)
                    .unwrap();
            }
            tx.commit().unwrap();
            let corrupted = active(&store);
            assert!(matches!(store.redb_load_blocks(), Err(Error::DataIntegrity(_))));
            assert!(matches!(store.redb_get_tip_slot_number(), Err(Error::DataIntegrity(_))));
            assert!(matches!(
                store.redb_get_eta_v_before_slot(11),
                Err(Error::DataIntegrity(_))
            ));
            assert!(matches!(
                store.redb_get_prev_hash_before_slot(11),
                Err(Error::DataIntegrity(_))
            ));
            assert!(matches!(
                store.redb_rollback_to(&Point::Specific(10, vec![1; 32])),
                Err(Error::DataIntegrity(_))
            ));
            assert_eq!(active(&store), corrupted);
            let tx = store.db.begin_write().unwrap();
            {
                tx.open_multimap_table(CHAIN_TABLE_SLOT_INDEX)
                    .unwrap()
                    .remove(10, 0)
                    .unwrap();
                if !missing_record {
                    tx.open_table(CHAIN_TABLE).unwrap().remove(0).unwrap();
                }
            }
            tx.commit().unwrap();
            assert_eq!(active(&store), before);
            assert_eq!(store.redb_get_tip_slot_number().unwrap(), 10);
        }
    }

    #[test]
    fn indexed_reads_use_slot_order_and_skip_orphan_history() {
        use pallas_network::miniprotocols::Point;
        let fixture = Fixture::new();
        let mut store = RedbBlockStore::new(&fixture.path()).unwrap();
        store
            .redb_save_block(&[header(1, 10, 1, 0), header(2, 1_000_000, 2, 1)], &"00".repeat(32))
            .unwrap();
        // Reverse physical key order while preserving both native indexes.
        let tx = store.db.begin_write().unwrap();
        {
            let mut table = tx.open_table(CHAIN_TABLE).unwrap();
            let records = table
                .iter()
                .unwrap()
                .map(|row| {
                    let (key, value) = row.unwrap();
                    (key.value(), value.value())
                })
                .collect::<Vec<_>>();
            let mut slots = tx.open_multimap_table(CHAIN_TABLE_SLOT_INDEX).unwrap();
            let mut hashes = tx.open_multimap_table(CHAIN_TABLE_HASH_INDEX).unwrap();
            for (key, record) in records {
                table.remove(key).unwrap();
                slots.remove(record.slot_number, key).unwrap();
                hashes.remove(record.hash.as_slice(), key).unwrap();
                let new_key = if record.slot_number == 10 { u128::MAX } else { 0 };
                slots.insert(record.slot_number, new_key).unwrap();
                hashes.insert(record.hash.as_slice(), new_key).unwrap();
                table.insert(new_key, record).unwrap();
            }
        }
        tx.commit().unwrap();
        assert_eq!(
            store.redb_load_blocks().unwrap(),
            vec![(1_000_000, vec![2; 32]), (10, vec![1; 32])]
        );
        assert_eq!(store.redb_get_tip_slot_number().unwrap(), 1_000_000);
        assert_eq!(
            store.redb_get_prev_hash_before_slot(1_000_001).unwrap(),
            Hash::from([1; 32])
        );
        assert_eq!(
            store.redb_get_prev_hash_before_slot(1_000_000).unwrap(),
            Hash::from([0; 32])
        );
        assert!(matches!(store.redb_get_eta_v_before_slot(10), Err(Error::DataNotFound)));
        let before = active(&store);
        assert!(matches!(
            store.redb_rollback_to(&Point::Specific(10, vec![9; 32])),
            Err(Error::DataIntegrity(message)) if message == "Rollback point not found in active chain"
        ));
        assert_eq!(active(&store), before);
        store.redb_rollback_to(&Point::Specific(10, vec![1; 32])).unwrap();
        assert_eq!(store.redb_get_tip_slot_number().unwrap(), 10);
        assert_eq!(store.redb_load_blocks().unwrap(), vec![(10, vec![1; 32])]);
        assert!(
            store
                .redb_find_block_by_hash(&hex::encode([2; 32]))
                .unwrap()
                .unwrap()
                .orphaned
        );
        let expected_nonce = generate_rolling_nonce(Hash::from([0; 32]), &[1; 32]);
        assert_eq!(store.redb_get_eta_v_before_slot(u64::MAX).unwrap(), expected_nonce);
        store.redb_rollback_to(&Point::Origin).unwrap();
        assert_eq!(store.redb_get_tip_slot_number().unwrap(), 0);
        assert!(store.redb_load_blocks().unwrap().is_empty());
        assert!(matches!(
            store.redb_get_prev_hash_before_slot(u64::MAX),
            Err(Error::DataNotFound)
        ));
    }

    #[test]
    fn future_and_missing_schema_metadata_fail_closed() {
        for metadata in [Some(2), None] {
            let fixture = Fixture::new();
            let mut store = RedbBlockStore::new(&fixture.path()).unwrap();
            store.redb_save_block(&[header(1, 10, 1, 0)], &"00".repeat(32)).unwrap();
            let before = active(&store);
            let tx = store.db.begin_write().unwrap();
            if let Some(version) = metadata {
                tx.open_table(VERSION_TABLE)
                    .unwrap()
                    .insert("version", version)
                    .unwrap();
            } else {
                tx.delete_table(VERSION_TABLE).unwrap();
            }
            tx.commit().unwrap();
            drop(store);
            let result = RedbBlockStore::new(&fixture.path());
            match metadata {
                Some(2) => assert!(matches!(
                    result,
                    Err(Error::UnsupportedSchemaVersion { found: 2, supported: 1 })
                )),
                None => assert!(matches!(result, Err(Error::DataIntegrity(_)))),
                _ => unreachable!(),
            }
            let raw = RedbBlockStore {
                db: Database::open(fixture.path()).unwrap(),
            };
            assert_eq!(active(&raw), before);
            let tx = raw.db.begin_read().unwrap();
            match metadata {
                Some(version) => assert_eq!(
                    tx.open_table(VERSION_TABLE)
                        .unwrap()
                        .get("version")
                        .unwrap()
                        .unwrap()
                        .value(),
                    version
                ),
                None => assert!(matches!(
                    tx.open_table(VERSION_TABLE),
                    Err(redb::TableError::TableDoesNotExist(_))
                )),
            }
        }
    }

    #[test]
    fn empty_version_table_is_corruption_and_version_zero_upgrades() {
        for version in [None, Some(0)] {
            let fixture = Fixture::new();
            let db = Database::create(fixture.path()).unwrap();
            let tx = db.begin_write().unwrap();
            {
                let mut table = tx.open_table(VERSION_TABLE).unwrap();
                if let Some(version) = version {
                    table.insert("version", version).unwrap();
                }
            }
            tx.commit().unwrap();
            drop(db);
            if version.is_none() {
                assert!(matches!(
                    RedbBlockStore::new(&fixture.path()),
                    Err(Error::DataIntegrity(_))
                ));
                let db = Database::open(fixture.path()).unwrap();
                let tx = db.begin_read().unwrap();
                assert!(tx.open_table(VERSION_TABLE).unwrap().get("version").unwrap().is_none());
                assert!(matches!(
                    tx.open_table(CHAIN_TABLE),
                    Err(redb::TableError::TableDoesNotExist(_))
                ));
            } else {
                let mut store = RedbBlockStore::new(&fixture.path()).unwrap();
                store.redb_save_block(&[header(1, 10, 1, 0)], &"00".repeat(32)).unwrap();
                assert_eq!(active(&store).len(), 1);
                store.redb_save_slots(1, "01", 2, "[10,20]", "02").unwrap();
                assert_eq!(store.redb_get_current_slots(1, "01").unwrap(), (2, "02".into()));
                let tx = store.db.begin_read().unwrap();
                assert_eq!(
                    tx.open_table(VERSION_TABLE)
                        .unwrap()
                        .get("version")
                        .unwrap()
                        .unwrap()
                        .value(),
                    1
                );
            }
        }
    }
}
