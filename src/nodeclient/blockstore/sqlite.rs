use crate::nodeclient::blockstore;
use crate::nodeclient::blockstore::{Block, BlockStore};
use crate::nodeclient::sync::BlockHeader;
use pallas_crypto::hash::{Hash, Hasher};
use pallas_crypto::nonce::generate_rolling_nonce;
use rusqlite::{named_params, Connection, OptionalExtension};
use std::path::Path;
use std::str::FromStr;
use thiserror::Error;
use tracing::{debug, error, info};

#[derive(Error, Debug)]
pub enum Error {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("FromHex error: {0}")]
    FromHex(#[from] hex::FromHexError),
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
            tx.execute(
                "CREATE TABLE IF NOT EXISTS db_version (version INTEGER PRIMARY KEY)",
                [],
            )?;
            let mut stmt = tx.prepare("SELECT version FROM db_version")?;
            let mut rows = stmt.query([])?;
            let version: i64 = match rows.next()? {
                None => -1,
                Some(row) => row.get(0)?,
            };

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
                    let vkeys = stmt
                        .query_map([], |row| {
                            let node_vkey_result: Result<String, rusqlite::Error> = row.get(0);
                            let node_vkey = node_vkey_result?;
                            Ok(node_vkey)
                        })
                        .ok()
                        .unwrap();

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

            // Update the db version now that we've upgraded the user's database fully
            if version < 0 {
                tx.execute(
                    "INSERT INTO db_version (version) VALUES (?1)",
                    [&SqLiteBlockStore::DB_VERSION],
                )?;
            } else {
                tx.execute("UPDATE db_version SET version=?1", [&SqLiteBlockStore::DB_VERSION])?;
            }
        }
        tx.commit()?;

        Ok(SqLiteBlockStore { db })
    }

    fn sql_save_block(
        &mut self,
        pending_blocks: &mut Vec<BlockHeader>,
        shelley_genesis_hash: &str,
    ) -> Result<(), Error> {
        let db = &mut self.db;

        // get the last block eta_v (nonce) in the db
        let mut prev_eta_v: Hash<32> = Hash::from(
            hex::decode(
                match db.query_row(
                    "SELECT eta_v, block_number FROM chain WHERE block_number = ?1 and orphaned = 0",
                    [&(pending_blocks.first().unwrap().block_number - 1)],
                    |row| row.get(0),
                ) {
                    Ok(eta_v) => eta_v,
                    Err(_) => {
                        info!(
                            "Start nonce calculation with shelley_genesis_hash: {:?}",
                            shelley_genesis_hash
                        );
                        shelley_genesis_hash.to_string()
                    }
                },
            )
            .unwrap()
            .as_slice(),
        );

        let tx = db.transaction()?;
        {
            // scope for db transaction
            let mut orphan_stmt = tx.prepare("UPDATE chain SET orphaned = 1 WHERE block_number >= ?1")?;
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

            for block in pending_blocks.drain(..) {
                // Set any necessary blocks as orphans
                let orphan_num = orphan_stmt.execute([&block.block_number])?;

                if orphan_num > 0 {
                    // get the last block eta_v (nonce) in the db
                    prev_eta_v = Hash::from(
                        hex::decode(
                            match tx.query_row(
                                "SELECT eta_v, block_number FROM chain WHERE block_number = ?1 and orphaned = 0",
                                [&(block.block_number - 1)],
                                |row| row.get(0),
                            ) {
                                Ok(eta_v) => eta_v,
                                Err(_) => {
                                    error!("Missing eta_v for block {:?}", block.block_number - 1);
                                    shelley_genesis_hash.to_string()
                                }
                            },
                        )
                        .unwrap()
                        .as_slice(),
                    );
                }
                // calculate rolling nonce (eta_v)
                let eta_v = generate_rolling_nonce(prev_eta_v, &block.eta_vrf_0);

                // blake2b 224 of node_vkey is the pool_id
                let pool_id = hex::encode(Hasher::<224>::hash(&block.node_vkey));

                insert_stmt.execute(named_params! {
                    ":block_number" : block.block_number,
                    ":slot_number": block.slot_number,
                    ":hash" : hex::encode(block.hash),
                    ":prev_hash" : hex::encode(block.prev_hash),
                    ":pool_id" : hex::encode(pool_id),
                    ":eta_v" : hex::encode(eta_v),
                    ":node_vkey" : hex::encode(block.node_vkey),
                    ":node_vrf_vkey" : hex::encode(block.node_vrf_vkey),
                    ":block_vrf_0": hex::encode(block.block_vrf_0),
                    ":block_vrf_1": hex::encode(block.block_vrf_1),
                    ":eta_vrf_0" : hex::encode(block.eta_vrf_0),
                    ":eta_vrf_1" : hex::encode(block.eta_vrf_1),
                    ":leader_vrf_0" : hex::encode(block.leader_vrf_0),
                    ":leader_vrf_1" : hex::encode(block.leader_vrf_1),
                    ":block_size" : block.block_size,
                    ":block_body_hash" : hex::encode(block.block_body_hash),
                    ":pool_opcert" : hex::encode(block.pool_opcert),
                    ":unknown_0" : block.unknown_0,
                    ":unknown_1" : block.unknown_1,
                    ":unknown_2" : hex::encode(block.unknown_2),
                    ":protocol_major_version" : block.protocol_major_version,
                    ":protocol_minor_version" : block.protocol_minor_version,
                })?;

                prev_eta_v = eta_v;
            }
        }

        tx.commit()?;
        Ok(())
    }

    fn sql_load_blocks(&mut self) -> Result<Vec<(u64, Vec<u8>)>, Error> {
        let db = &self.db;
        let mut stmt = db
            .prepare("SELECT slot_number, hash FROM (SELECT slot_number, hash, orphaned FROM chain ORDER BY slot_number DESC LIMIT 100) WHERE orphaned = 0 ORDER BY slot_number DESC LIMIT 33;")?;
        let blocks = stmt.query_map([], |row| {
            let slot_result: Result<u64, rusqlite::Error> = row.get(0);
            let hash_result: Result<String, rusqlite::Error> = row.get(1);
            let slot = slot_result?;
            let hash = hash_result?;
            Ok((slot, hex::decode(hash).unwrap()))
        })?;
        Ok(blocks.map(|item| item.unwrap()).collect())
    }

    fn sql_find_block_by_hash(&mut self, hash_start: &str) -> Result<Option<Block>, Error> {
        let db = &self.db;
        let like = format!("{hash_start}%");
        Ok(db.query_row(
            "SELECT block_number,slot_number,hash,prev_hash,pool_id,leader_vrf_0,orphaned FROM chain WHERE hash LIKE ? ORDER BY orphaned ASC",
            [&like],
            |row| {
                Ok(Some(Block {
                    block_number: row.get(0)?,
                    slot_number: row.get(1)?,
                    hash: row.get(2)?,
                    prev_hash: row.get(3)?,
                    pool_id: row.get(4)?,
                    leader_vrf: row.get(5)?,
                    orphaned: row.get(6)?,
                }))
            },
        )?)
    }

    fn sql_get_tip_slot_number(&mut self) -> Result<u64, Error> {
        let db = &self.db;
        let tip_slot_number: u64 = db.query_row("SELECT MAX(slot_number) FROM chain", [], |row| row.get(0))?;
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
        pending_blocks: &mut Vec<BlockHeader>,
        shelley_genesis_hash: &str,
    ) -> Result<(), blockstore::Error> {
        Ok(self.sql_save_block(pending_blocks, shelley_genesis_hash)?)
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
