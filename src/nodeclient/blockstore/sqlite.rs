use crate::nodeclient::blockstore;
use crate::nodeclient::blockstore::{Block, BlockStore};
use crate::nodeclient::sync::BlockHeader;
use pallas_crypto::hash::{Hash, Hasher};
use pallas_crypto::nonce::generate_rolling_nonce;
use pallas_network::miniprotocols::Point;
use rusqlite::{named_params, Connection, OptionalExtension};
use std::path::Path;
use std::str::FromStr;
use thiserror::Error;
use tracing::{debug, info};

#[derive(Error, Debug)]
pub enum Error {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("FromHex error: {0}")]
    FromHex(#[from] hex::FromHexError),

    #[error("Data integrity error: {0}")]
    DataIntegrity(String),

    #[error("Unsupported schema version {found}; supported version is {supported}")]
    UnsupportedSchemaVersion { found: i64, supported: i64 },
}

pub struct SqLiteBlockStore {
    pub db: Connection,
}

impl SqLiteBlockStore {
    const DB_VERSION: i64 = 4;

    pub fn new(db_path: &Path) -> Result<SqLiteBlockStore, Error> {
        debug!("Opening database");
        let mut db = Connection::open(db_path)?;
        db.execute_batch("PRAGMA journal_mode=WAL")?;

        let tx = db.transaction()?;
        {
            debug!("Intialize database.");
            let version_table_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'db_version')",
                [],
                |row| row.get(0),
            )?;
            let has_data_tables: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name IN ('chain', 'slots'))",
                [],
                |row| row.get(0),
            )?;
            let stored_version = if version_table_exists {
                let mut stmt = tx.prepare("SELECT version FROM db_version")?;
                let mut rows = stmt.query([])?;
                let version = match rows.next()? {
                    None => None,
                    Some(row) => Some(
                        row.get::<_, i64>(0)
                            .map_err(|_| Error::DataIntegrity("Invalid schema version value".into()))?,
                    ),
                };
                if rows.next()?.is_some() {
                    return Err(Error::DataIntegrity("Multiple schema version rows".into()));
                }
                version
            } else {
                None
            };
            let version = match stored_version {
                None if has_data_tables => {
                    return Err(Error::DataIntegrity(
                        "Existing data tables without schema version".into(),
                    ));
                }
                None => 0,
                Some(version) if version < 0 => {
                    return Err(Error::DataIntegrity("Negative schema version".into()));
                }
                Some(version) if version > Self::DB_VERSION => {
                    return Err(Error::UnsupportedSchemaVersion {
                        found: version,
                        supported: Self::DB_VERSION,
                    });
                }
                Some(version) => version,
            };
            if !version_table_exists {
                tx.execute("CREATE TABLE db_version (version INTEGER PRIMARY KEY)", [])?;
            }

            // Upgrade their database to version 1
            if version < 1 {
                info!(" Create database at version 1...");
                tx.execute(
                    "CREATE TABLE IF NOT EXISTS chain (\
                    id INTEGER PRIMARY KEY AUTOINCREMENT, \
                    block_number INTEGER NOT NULL, \
                    slot_number INTEGER NOT NULL, \
                    hash TEXT NOT NULL, \
                    prev_hash TEXT NOT NULL, \
                    eta_v TEXT NOT NULL, \
                    node_vkey TEXT NOT NULL, \
                    node_vrf_vkey TEXT NOT NULL, \
                    eta_vrf_0 TEXT NOT NULL, \
                    eta_vrf_1 TEXT NOT NULL, \
                    leader_vrf_0 TEXT NOT NULL, \
                    leader_vrf_1 TEXT NOT NULL, \
                    block_size INTEGER NOT NULL, \
                    block_body_hash TEXT NOT NULL, \
                    pool_opcert TEXT NOT NULL, \
                    unknown_0 INTEGER NOT NULL, \
                    unknown_1 INTEGER NOT NULL, \
                    unknown_2 TEXT NOT NULL, \
                    protocol_major_version INTEGER NOT NULL, \
                    protocol_minor_version INTEGER NOT NULL, \
                    orphaned INTEGER NOT NULL DEFAULT 0 \
                    )",
                    [],
                )?;
                tx.execute(
                    "CREATE INDEX IF NOT EXISTS idx_chain_slot_number ON chain(slot_number)",
                    [],
                )?;
                tx.execute("CREATE INDEX IF NOT EXISTS idx_chain_orphaned ON chain(orphaned)", [])?;
                tx.execute("CREATE INDEX IF NOT EXISTS idx_chain_hash ON chain(hash)", [])?;
                tx.execute(
                    "CREATE INDEX IF NOT EXISTS idx_chain_block_number ON chain(block_number)",
                    [],
                )?;
            }

            // Upgrade their database to version 2
            if version < 2 {
                info!("Upgrade database to version 2...");
                tx.execute(
                    "CREATE TABLE IF NOT EXISTS slots (\
                    id INTEGER PRIMARY KEY AUTOINCREMENT, \
                    epoch INTEGER NOT NULL, \
                    pool_id TEXT NOT NULL, \
                    slot_qty INTEGER NOT NULL, \
                    slots TEXT NOT NULL, \
                    hash TEXT NOT NULL,
                    UNIQUE(epoch,pool_id)
                )",
                    [],
                )?;
            }

            if version < 3 {
                info!("Upgrade database to version 3...");
                tx.execute("CREATE INDEX IF NOT EXISTS idx_chain_node_vkey ON chain(node_vkey)", [])?;
                tx.execute("ALTER TABLE chain ADD COLUMN pool_id TEXT NOT NULL DEFAULT ''", [])?;
                tx.execute("CREATE INDEX IF NOT EXISTS idx_chain_pool_id ON chain(pool_id)", [])?;

                let count: i64 = tx.query_row("SELECT COUNT(DISTINCT node_vkey) from chain", [], |row| row.get(0))?;

                if count > 0 {
                    let mut stmt = tx.prepare("SELECT DISTINCT node_vkey FROM chain")?;
                    let vkeys = stmt.query_map([], |row| row.get::<_, String>(0))?;

                    info!("{} pool id records to process. Please be patient...", &count);
                    for (i, node_vkey) in vkeys.into_iter().enumerate() {
                        let vkey = node_vkey?;
                        let node_vkey_bytes = hex::decode(&vkey)?;
                        let pool_id = hex::encode(Hasher::<224>::hash(&node_vkey_bytes));

                        tx.execute(
                            "UPDATE chain SET pool_id=:pool_id WHERE node_vkey=:node_vkey",
                            named_params! {
                                ":pool_id": pool_id,
                                ":node_vkey": vkey
                            },
                        )?;

                        if i % 25 == 0 {
                            info!("Updated record {} of {}...", i, count);
                        }
                    }
                    info!("Updated record {} of {}...done!", count, count);
                }
            }

            if version < 4 {
                info!("Upgrade database to version 4...");
                tx.execute("ALTER TABLE chain ADD COLUMN block_vrf_0 TEXT NOT NULL DEFAULT ''", [])?;
                tx.execute("ALTER TABLE chain ADD COLUMN block_vrf_1 TEXT NOT NULL DEFAULT ''", [])?;
            }

            // Write metadata only after initialization or an upgrade succeeds.
            if stored_version.is_none() {
                tx.execute("INSERT INTO db_version (version) VALUES (?1)", [Self::DB_VERSION])?;
            } else if version < Self::DB_VERSION {
                tx.execute("UPDATE db_version SET version=?1", [Self::DB_VERSION])?;
            }
        }
        tx.commit()?;

        Ok(SqLiteBlockStore { db })
    }

    fn decode_hash(value: &str, context: &str) -> Result<Hash<32>, Error> {
        let bytes = hex::decode(value)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::DataIntegrity(format!("{context} must contain exactly 32 bytes")))?;
        Ok(Hash::from(bytes))
    }

    fn sql_save_block(&mut self, pending_blocks: &[BlockHeader], shelley_genesis_hash: &str) -> Result<(), Error> {
        let Some(first) = pending_blocks.first() else {
            return Ok(());
        };
        if first.block_number == 0 {
            return Err(Error::DataIntegrity("First block number must be positive".into()));
        }
        let genesis = Self::decode_hash(shelley_genesis_hash, "Shelley genesis hash")?;
        for pair in pending_blocks.windows(2) {
            if pair[0].block_number.checked_add(1) != Some(pair[1].block_number)
                || pair[0].slot_number >= pair[1].slot_number
                || pair[0].hash != pair[1].prev_hash
            {
                return Err(Error::DataIntegrity("Non-contiguous pending block batch".into()));
            }
        }
        let tx = self.db.transaction()?;
        let mut prev_eta_v = {
            let mut stmt =
                tx.prepare("SELECT hash, eta_v, slot_number FROM chain WHERE block_number = ?1 AND orphaned = 0")?;
            let mut rows = stmt.query([first.block_number - 1])?;
            match rows.next()? {
                Some(row) => {
                    let hash: String = row.get(0)?;
                    let nonce: String = row.get(1)?;
                    let slot: u64 = row.get(2)?;
                    if rows.next()?.is_some() {
                        return Err(Error::DataIntegrity("Multiple active predecessor blocks".into()));
                    }
                    if Self::decode_hash(&hash, "Predecessor hash")?.as_slice() != first.prev_hash.as_slice()
                        || slot >= first.slot_number
                    {
                        return Err(Error::DataIntegrity(
                            "Pending batch does not extend its active predecessor".into(),
                        ));
                    }
                    Self::decode_hash(&nonce, "Predecessor nonce")?
                }
                None => {
                    let has_active: bool =
                        tx.query_row("SELECT EXISTS(SELECT 1 FROM chain WHERE orphaned = 0)", [], |row| {
                            row.get(0)
                        })?;
                    if has_active {
                        return Err(Error::DataIntegrity(
                            "Missing predecessor in nonempty active chain".into(),
                        ));
                    }
                    genesis
                }
            }
        };
        {
            // scope for db transaction
            tx.execute(
                "UPDATE chain SET orphaned = 1 WHERE block_number >= ?1 AND orphaned = 0",
                [first.block_number],
            )?;
            let mut insert_stmt = tx.prepare(
                "INSERT INTO chain (\
            block_number, \
            slot_number, \
            hash, \
            prev_hash, \
            pool_id, \
            eta_v, \
            node_vkey, \
            node_vrf_vkey, \
            block_vrf_0, \
            block_vrf_1, \
            eta_vrf_0, \
            eta_vrf_1, \
            leader_vrf_0, \
            leader_vrf_1, \
            block_size, \
            block_body_hash, \
            pool_opcert, \
            unknown_0, \
            unknown_1, \
            unknown_2, \
            protocol_major_version, \
            protocol_minor_version) \
            VALUES (\
            :block_number, \
            :slot_number, \
            :hash, \
            :prev_hash, \
            :pool_id, \
            :eta_v, \
            :node_vkey, \
            :node_vrf_vkey, \
            :block_vrf_0, \
            :block_vrf_1, \
            :eta_vrf_0, \
            :eta_vrf_1, \
            :leader_vrf_0, \
            :leader_vrf_1, \
            :block_size, \
            :block_body_hash, \
            :pool_opcert, \
            :unknown_0, \
            :unknown_1, \
            :unknown_2, \
            :protocol_major_version, \
            :protocol_minor_version)",
            )?;

            for block in pending_blocks {
                // calculate rolling nonce (eta_v)
                let eta_v = generate_rolling_nonce(prev_eta_v, &block.eta_vrf_0);

                // blake2b 224 of node_vkey is the pool_id
                let pool_id = Hasher::<224>::hash(&block.node_vkey);

                insert_stmt.execute(named_params! {
                    ":block_number" : block.block_number,
                    ":slot_number": block.slot_number,
                    ":hash" : hex::encode(&block.hash),
                    ":prev_hash" : hex::encode(&block.prev_hash),
                    ":pool_id" : hex::encode(pool_id),
                    ":eta_v" : hex::encode(eta_v),
                    ":node_vkey" : hex::encode(&block.node_vkey),
                    ":node_vrf_vkey" : hex::encode(&block.node_vrf_vkey),
                    ":block_vrf_0": hex::encode(&block.block_vrf_0),
                    ":block_vrf_1": hex::encode(&block.block_vrf_1),
                    ":eta_vrf_0" : hex::encode(&block.eta_vrf_0),
                    ":eta_vrf_1" : hex::encode(&block.eta_vrf_1),
                    ":leader_vrf_0" : hex::encode(&block.leader_vrf_0),
                    ":leader_vrf_1" : hex::encode(&block.leader_vrf_1),
                    ":block_size" : block.block_size,
                    ":block_body_hash" : hex::encode(&block.block_body_hash),
                    ":pool_opcert" : hex::encode(&block.pool_opcert),
                    ":unknown_0" : block.unknown_0,
                    ":unknown_1" : block.unknown_1,
                    ":unknown_2" : hex::encode(&block.unknown_2),
                    ":protocol_major_version" : block.protocol_major_version,
                    ":protocol_minor_version" : block.protocol_minor_version,
                })?;

                prev_eta_v = eta_v;
            }
        }

        tx.commit()?;
        Ok(())
    }

    fn sql_rollback_to(&mut self, point: &Point) -> Result<(), Error> {
        let tx = self.db.transaction()?;
        match point {
            Point::Origin => {
                tx.execute("UPDATE chain SET orphaned = 1 WHERE orphaned = 0", [])?;
            }
            Point::Specific(slot, hash) => {
                let block_number: u64 = tx
                    .query_row(
                        "SELECT block_number FROM chain WHERE orphaned = 0 AND slot_number = ?1 AND hash = ?2",
                        rusqlite::params![slot, hex::encode(hash)],
                        |row| row.get(0),
                    )
                    .optional()?
                    .ok_or_else(|| Error::DataIntegrity("Rollback point not found in active chain".into()))?;
                tx.execute(
                    "UPDATE chain SET orphaned = 1 WHERE orphaned = 0 AND block_number > ?1",
                    [block_number],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn sql_load_blocks(&mut self) -> Result<Vec<(u64, Vec<u8>)>, Error> {
        let db = &self.db;
        let mut stmt = db
            .prepare("SELECT slot_number, hash FROM chain WHERE orphaned = 0 ORDER BY slot_number DESC LIMIT 262145")?;
        let blocks = stmt.query_map([], |row| Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?)))?;
        blocks
            .map(|item| {
                let (slot, hash) = item?;
                Ok((slot, hex::decode(hash)?))
            })
            .collect()
    }

    fn sql_find_block_by_hash(&mut self, hash_start: &str) -> Result<Option<Block>, Error> {
        let db = &self.db;
        let like = format!("{hash_start}%");
        Ok(db.query_row(
            "SELECT block_number,slot_number,hash,prev_hash,pool_id,leader_vrf_0,orphaned FROM chain WHERE hash LIKE ? ORDER BY orphaned ASC",
            [&like],
            |row| {
                Ok(Block {
                    block_number: row.get(0)?,
                    slot_number: row.get(1)?,
                    hash: row.get(2)?,
                    prev_hash: row.get(3)?,
                    pool_id: row.get(4)?,
                    leader_vrf: row.get(5)?,
                    orphaned: row.get(6)?,
                })
            },
        ).optional()?)
    }

    fn sql_get_tip_slot_number(&mut self) -> Result<u64, Error> {
        let db = &self.db;
        let tip_slot_number: u64 = db.query_row(
            "SELECT COALESCE(MAX(slot_number), 0) FROM chain WHERE orphaned = 0",
            [],
            |row| row.get(0),
        )?;
        Ok(tip_slot_number)
    }

    fn sql_get_eta_v_before_slot(&mut self, slot_number: u64) -> Result<Hash<32>, Error> {
        let db = &self.db;
        let eta_v_hex: String = db.query_row(
            "SELECT eta_v FROM chain WHERE orphaned = 0 AND slot_number < ?1 ORDER BY slot_number DESC LIMIT 1",
            [&slot_number],
            |row| row.get(0),
        )?;
        let eta_v: Hash<32> = Hash::from_str(&eta_v_hex)?;
        Ok(eta_v)
    }

    fn sql_get_prev_hash_before_slot(&mut self, slot_number: u64) -> Result<Hash<32>, Error> {
        let db = &self.db;
        let prev_hash_hex: String = db.query_row(
            "SELECT prev_hash FROM chain WHERE orphaned = 0 AND slot_number < ?1 ORDER BY slot_number DESC LIMIT 1",
            [&slot_number],
            |row| row.get(0),
        )?;
        let prev_hash: Hash<32> = Hash::from_str(&prev_hash_hex)?;
        Ok(prev_hash)
    }

    fn sql_save_slots(
        &mut self,
        epoch: u64,
        pool_id: &str,
        slot_qty: u64,
        slots: &str,
        hash: &str,
    ) -> Result<(), Error> {
        let db = &mut self.db;
        let tx = db.transaction()?;
        {
            let mut stmt = tx.prepare("INSERT INTO slots (epoch, pool_id, slot_qty, slots, hash) VALUES (:epoch, :pool_id, :slot_qty, :slots, :hash) ON CONFLICT (epoch,pool_id) DO UPDATE SET slot_qty=excluded.slot_qty, slots=excluded.slots, hash=excluded.hash")?;
            stmt.execute(named_params! {
                ":epoch" : epoch,
                ":pool_id" : pool_id,
                ":slot_qty" : slot_qty,
                ":slots" : slots,
                ":hash" : hash,
            })?;
        }
        tx.commit()?;
        Ok(())
    }

    fn sql_get_current_slots(&mut self, epoch: u64, pool_id: &str) -> Result<(u64, String), Error> {
        let db = &self.db;
        let mut stmt = db.prepare("SELECT slot_qty, hash FROM slots WHERE epoch = :epoch AND pool_id = :pool_id")?;
        Ok(stmt.query_row(
            named_params! {
                ":epoch" : epoch,
                ":pool_id" : pool_id,
            },
            |row| {
                let slot_qty: u64 = row.get(0)?;
                let hash: String = row.get(1)?;
                Ok((slot_qty, hash))
            },
        )?)
    }

    fn sql_get_previous_slots(&mut self, epoch: u64, pool_id: &str) -> Result<Option<String>, Error> {
        let db = &self.db;
        let mut stmt = db.prepare("SELECT slots FROM slots WHERE epoch = :epoch AND pool_id = :pool_id")?;
        Ok(stmt
            .query_row(
                named_params! {
                    ":epoch" : epoch,
                    ":pool_id" : pool_id,
                },
                |row| {
                    let slots: String = row.get(0)?;
                    Ok(slots)
                },
            )
            .optional()?)
    }
}

impl BlockStore for SqLiteBlockStore {
    fn save_block(
        &mut self,
        pending_blocks: &[BlockHeader],
        shelley_genesis_hash: &str,
    ) -> Result<(), blockstore::Error> {
        Ok(self.sql_save_block(pending_blocks, shelley_genesis_hash)?)
    }

    fn rollback_to(&mut self, point: &Point) -> Result<(), blockstore::Error> {
        Ok(self.sql_rollback_to(point)?)
    }

    fn load_blocks(&mut self) -> Result<Vec<(u64, Vec<u8>)>, blockstore::Error> {
        Ok(self.sql_load_blocks()?)
    }

    fn find_block_by_hash(&mut self, hash_start: &str) -> Result<Option<Block>, blockstore::Error> {
        Ok(self.sql_find_block_by_hash(hash_start)?)
    }

    fn get_tip_slot_number(&mut self) -> Result<u64, blockstore::Error> {
        Ok(self.sql_get_tip_slot_number()?)
    }

    fn get_eta_v_before_slot(&mut self, slot_number: u64) -> Result<Hash<32>, blockstore::Error> {
        Ok(self.sql_get_eta_v_before_slot(slot_number)?)
    }

    fn get_prev_hash_before_slot(&mut self, slot_number: u64) -> Result<Hash<32>, blockstore::Error> {
        Ok(self.sql_get_prev_hash_before_slot(slot_number)?)
    }

    fn save_slots(
        &mut self,
        epoch: u64,
        pool_id: &str,
        slot_qty: u64,
        slots: &str,
        hash: &str,
    ) -> Result<(), blockstore::Error> {
        Ok(self.sql_save_slots(epoch, pool_id, slot_qty, slots, hash)?)
    }

    fn get_current_slots(&mut self, epoch: u64, pool_id: &str) -> Result<(u64, String), blockstore::Error> {
        Ok(self.sql_get_current_slots(epoch, pool_id)?)
    }

    fn get_previous_slots(&mut self, epoch: u64, pool_id: &str) -> Result<Option<String>, blockstore::Error> {
        Ok(self.sql_get_previous_slots(epoch, pool_id)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("cncli-sqlite-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn db_path(&self) -> PathBuf {
            self.0.join("chain.db")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn header(number: u64, slot: u64, hash: u8, parent: u8) -> BlockHeader {
        BlockHeader {
            block_number: number,
            slot_number: slot,
            hash: vec![hash; 32],
            prev_hash: vec![parent; 32],
            node_vkey: vec![4; 32],
            node_vrf_vkey: vec![5; 32],
            block_vrf_0: vec![],
            block_vrf_1: vec![],
            eta_vrf_0: vec![hash; 32],
            eta_vrf_1: vec![6; 80],
            leader_vrf_0: vec![7; 32],
            leader_vrf_1: vec![8; 80],
            block_size: 0,
            block_body_hash: vec![9; 32],
            pool_opcert: vec![],
            unknown_0: 0,
            unknown_1: 0,
            unknown_2: vec![],
            protocol_major_version: 2,
            protocol_minor_version: 0,
        }
    }

    fn chain_state(store: &SqLiteBlockStore) -> Vec<(u64, String, bool)> {
        store
            .db
            .prepare("SELECT block_number, hash, orphaned FROM chain ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn aborted_rollback_preserves_active_chain_and_history() {
        let fixture = Fixture::new();
        let mut store = SqLiteBlockStore::new(&fixture.db_path()).unwrap();
        store
            .sql_save_block(
                &[header(1, 10, 1, 0), header(2, 20, 2, 1), header(3, 30, 3, 2)],
                &"00".repeat(32),
            )
            .unwrap();
        let before = chain_state(&store);
        store
            .db
            .execute_batch(
                "CREATE TRIGGER fail_rollback BEFORE UPDATE OF orphaned ON chain WHEN OLD.block_number = 3
             BEGIN SELECT RAISE(ABORT, 'injected rollback failure'); END;",
            )
            .unwrap();
        assert!(matches!(
            store.sql_rollback_to(&Point::Specific(10, vec![1; 32])),
            Err(Error::Sqlite(_))
        ));
        assert_eq!(chain_state(&store), before);
        assert!(matches!(store.sql_rollback_to(&Point::Origin), Err(Error::Sqlite(_))));
        assert_eq!(chain_state(&store), before);
        store.db.execute_batch("DROP TRIGGER fail_rollback").unwrap();
        store.sql_rollback_to(&Point::Specific(10, vec![1; 32])).unwrap();
        assert_eq!(store.sql_get_tip_slot_number().unwrap(), 10);
        assert_eq!(store.sql_load_blocks().unwrap(), vec![(10, vec![1; 32])]);
        assert!(
            store
                .sql_find_block_by_hash(&"03".repeat(32))
                .unwrap()
                .unwrap()
                .orphaned
        );
        assert!(store.sql_find_block_by_hash(&"ff".repeat(32)).unwrap().is_none());
        let after = chain_state(&store);
        assert!(matches!(
            store.sql_rollback_to(&Point::Specific(30, vec![3; 32])),
            Err(Error::DataIntegrity(message)) if message == "Rollback point not found in active chain"
        ));
        assert_eq!(chain_state(&store), after);
        store.sql_rollback_to(&Point::Origin).unwrap();
        assert_eq!(store.sql_get_tip_slot_number().unwrap(), 0);
        assert!(store.sql_load_blocks().unwrap().is_empty());
        assert_eq!(chain_state(&store).len(), 3);
    }

    #[test]
    fn aborted_second_insert_preserves_suffix_and_input_then_retries() {
        let fixture = Fixture::new();
        let mut store = SqLiteBlockStore::new(&fixture.db_path()).unwrap();
        let genesis = "00".repeat(32);
        store
            .sql_save_block(&[header(1, 10, 1, 0), header(2, 20, 2, 1)], &genesis)
            .unwrap();
        let before = chain_state(&store);
        store
            .db
            .execute_batch(
                "CREATE TRIGGER fail_second BEFORE INSERT ON chain WHEN NEW.block_number = 3
             BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
            )
            .unwrap();
        let pending = vec![header(2, 30, 12, 1), header(3, 40, 13, 12)];
        assert!(matches!(
            store.sql_save_block(&pending, &genesis),
            Err(Error::Sqlite(_))
        ));
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].hash, vec![12; 32]);
        assert_eq!(pending[1].hash, vec![13; 32]);
        assert_eq!(chain_state(&store), before);
        store.db.execute_batch("DROP TRIGGER fail_second").unwrap();
        store.sql_save_block(&pending, &genesis).unwrap();
        let state = chain_state(&store);
        assert_eq!(state.len(), 4);
        assert!(state[1].2);
        assert!(!state[2].2 && !state[3].2);
        let expected = pending
            .iter()
            .fold(generate_rolling_nonce(Hash::from([0; 32]), &[1; 32]), |nonce, block| {
                generate_rolling_nonce(nonce, &block.eta_vrf_0)
            });
        assert_eq!(store.sql_get_eta_v_before_slot(41).unwrap(), expected);
    }

    #[test]
    fn invalid_batches_and_predecessors_do_not_mutate() {
        let fixture = Fixture::new();
        let mut store = SqLiteBlockStore::new(&fixture.db_path()).unwrap();
        let genesis = "00".repeat(32);
        store.sql_save_block(&[], "not hex").unwrap();
        assert!(store.sql_save_block(&[header(0, 10, 1, 0)], &genesis).is_err());
        assert!(store.sql_save_block(&[header(1, 10, 1, 0)], &"00".repeat(31)).is_err());
        store.sql_save_block(&[header(1, 10, 1, 0)], &genesis).unwrap();
        let before = chain_state(&store);
        for batch in [
            vec![header(3, 30, 3, 1)],
            vec![header(2, 20, 2, 9)],
            vec![header(2, 10, 2, 1)],
            vec![header(2, 20, 2, 1), header(4, 40, 4, 2)],
            vec![header(2, 20, 2, 1), header(3, 19, 3, 2)],
            vec![header(2, 20, 2, 1), header(3, 30, 3, 9)],
            vec![header(u64::MAX, 20, 2, 1), header(1, 30, 3, 2)],
        ] {
            assert!(matches!(
                store.sql_save_block(&batch, &genesis),
                Err(Error::DataIntegrity(_))
            ));
            assert_eq!(chain_state(&store), before);
        }
        store
            .db
            .execute("UPDATE chain SET eta_v = ?1", ["00".repeat(31)])
            .unwrap();
        assert!(matches!(
            store.sql_save_block(&[header(2, 20, 2, 1)], &genesis),
            Err(Error::DataIntegrity(_))
        ));
        assert_eq!(chain_state(&store), before);
        store.db.execute("UPDATE chain SET hash = 'invalid hex'", []).unwrap();
        assert!(matches!(store.sql_load_blocks(), Err(Error::FromHex(_))));
    }

    #[test]
    fn schema_metadata_fails_closed() {
        for marker in [
            "UPDATE db_version SET version = 5",
            "DROP TABLE db_version",
            "DELETE FROM db_version",
            "INSERT INTO db_version VALUES (3)",
            "UPDATE db_version SET version = -1",
            "DROP TABLE db_version; CREATE TABLE db_version(version); INSERT INTO db_version VALUES ('invalid')",
        ] {
            let fixture = Fixture::new();
            let mut store = SqLiteBlockStore::new(&fixture.db_path()).unwrap();
            store.sql_save_block(&[header(1, 10, 1, 0)], &"00".repeat(32)).unwrap();
            store.db.execute_batch(marker).unwrap();
            let before = chain_state(&store);
            let schema: String = store
                .db
                .query_row(
                    "SELECT group_concat(sql, ';') FROM (SELECT sql FROM sqlite_schema ORDER BY name)",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let metadata: Vec<String> = if marker == "DROP TABLE db_version" {
                vec![]
            } else {
                store
                    .db
                    .prepare("SELECT quote(version) FROM db_version ORDER BY version")
                    .unwrap()
                    .query_map([], |row| row.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap()
            };
            drop(store);
            let result = SqLiteBlockStore::new(&fixture.db_path());
            if marker == "UPDATE db_version SET version = 5" {
                assert!(matches!(
                    result,
                    Err(Error::UnsupportedSchemaVersion { found: 5, supported: 4 })
                ));
            } else {
                assert!(matches!(result, Err(Error::DataIntegrity(_))));
            }
            let db = Connection::open(fixture.db_path()).unwrap();
            let store = SqLiteBlockStore { db };
            assert_eq!(chain_state(&store), before);
            assert_eq!(
                store
                    .db
                    .query_row::<String, _, _>(
                        "SELECT group_concat(sql, ';') FROM (SELECT sql FROM sqlite_schema ORDER BY name)",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap(),
                schema
            );
            if marker != "DROP TABLE db_version" {
                let after: Vec<String> = store
                    .db
                    .prepare("SELECT quote(version) FROM db_version ORDER BY version")
                    .unwrap()
                    .query_map([], |row| row.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                assert_eq!(after, metadata);
            }
        }
    }

    #[test]
    fn legacy_versions_zero_through_three_migrate_headers_and_slots() {
        for version in 0..=3 {
            let fixture = Fixture::new();
            let db = Connection::open(fixture.db_path()).unwrap();
            db.execute_batch("CREATE TABLE db_version(version INTEGER PRIMARY KEY)")
                .unwrap();
            db.execute("INSERT INTO db_version VALUES (?1)", [version]).unwrap();
            if version >= 1 {
                db.execute_batch(
                    "CREATE TABLE chain (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     block_number INTEGER NOT NULL, slot_number INTEGER NOT NULL,
                     hash TEXT NOT NULL, prev_hash TEXT NOT NULL, eta_v TEXT NOT NULL,
                     node_vkey TEXT NOT NULL, node_vrf_vkey TEXT NOT NULL,
                     eta_vrf_0 TEXT NOT NULL, eta_vrf_1 TEXT NOT NULL,
                     leader_vrf_0 TEXT NOT NULL, leader_vrf_1 TEXT NOT NULL,
                     block_size INTEGER NOT NULL, block_body_hash TEXT NOT NULL,
                     pool_opcert TEXT NOT NULL, unknown_0 INTEGER NOT NULL,
                     unknown_1 INTEGER NOT NULL, unknown_2 TEXT NOT NULL,
                     protocol_major_version INTEGER NOT NULL, protocol_minor_version INTEGER NOT NULL,
                     orphaned INTEGER NOT NULL DEFAULT 0);
                     CREATE INDEX idx_chain_slot_number ON chain(slot_number);
                     CREATE INDEX idx_chain_orphaned ON chain(orphaned);
                     CREATE INDEX idx_chain_hash ON chain(hash);
                     CREATE INDEX idx_chain_block_number ON chain(block_number);",
                )
                .unwrap();
                db.execute(
                    "INSERT INTO chain (block_number, slot_number, hash, prev_hash, eta_v,
                     node_vkey, node_vrf_vkey, eta_vrf_0, eta_vrf_1, leader_vrf_0, leader_vrf_1,
                     block_size, block_body_hash, pool_opcert, unknown_0, unknown_1, unknown_2,
                     protocol_major_version, protocol_minor_version)
                     VALUES (1, 10, ?1, ?2, ?2, ?3, '', '', '', '', '', 0, '', '', 0, 0, '', 2, 0)",
                    [hex::encode([1; 32]), hex::encode([0; 32]), hex::encode([4; 32])],
                )
                .unwrap();
            }
            if version >= 2 {
                db.execute_batch(
                    "CREATE TABLE slots (
                     id INTEGER PRIMARY KEY AUTOINCREMENT, epoch INTEGER NOT NULL,
                     pool_id TEXT NOT NULL, slot_qty INTEGER NOT NULL, slots TEXT NOT NULL,
                     hash TEXT NOT NULL, UNIQUE(epoch,pool_id));",
                )
                .unwrap();
                db.execute(
                    "INSERT INTO slots(epoch,pool_id,slot_qty,slots,hash) VALUES (1,'pool',1,'[10]','old')",
                    [],
                )
                .unwrap();
            }
            if version >= 3 {
                db.execute_batch(
                    "CREATE INDEX idx_chain_node_vkey ON chain(node_vkey);
                     ALTER TABLE chain ADD COLUMN pool_id TEXT NOT NULL DEFAULT '';
                     CREATE INDEX idx_chain_pool_id ON chain(pool_id);",
                )
                .unwrap();
                db.execute(
                    "UPDATE chain SET pool_id=?1",
                    [hex::encode(Hasher::<224>::hash(&[4; 32]))],
                )
                .unwrap();
            }
            drop(db);
            let mut store = SqLiteBlockStore::new(&fixture.db_path()).unwrap();
            assert_eq!(
                store
                    .db
                    .query_row::<i64, _, _>("SELECT version FROM db_version", [], |row| row.get(0))
                    .unwrap(),
                4
            );
            if version >= 1 {
                assert_eq!(store.sql_load_blocks().unwrap(), vec![(10, vec![1; 32])]);
                let block = store.sql_find_block_by_hash(&hex::encode([1; 32])).unwrap().unwrap();
                assert_eq!(block.block_number, 1);
                assert_eq!(block.pool_id, hex::encode(Hasher::<224>::hash(&[4; 32])));
                store.sql_save_block(&[header(2, 20, 2, 1)], &"00".repeat(32)).unwrap();
            } else {
                assert!(store.sql_load_blocks().unwrap().is_empty());
                store.sql_save_block(&[header(1, 10, 1, 0)], &"00".repeat(32)).unwrap();
            }
            if version >= 2 {
                assert_eq!(store.sql_get_previous_slots(1, "pool").unwrap(), Some("[10]".into()));
            }
            store.sql_save_slots(1, "pool", 2, "[20,30]", "new").unwrap();
            assert_eq!(store.sql_get_current_slots(1, "pool").unwrap(), (2, "new".into()));
            assert_eq!(store.sql_get_previous_slots(1, "pool").unwrap(), Some("[20,30]".into()));
        }
    }
}
