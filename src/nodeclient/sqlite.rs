use std::io;
use std::path::Path;

use blake2b_simd::Params;
use log::{debug, error, info};
use pallas_crypto::hash::Hash;
use pallas_crypto::nonce::NonceGenerator;
use pallas_crypto::nonce::rolling_nonce::RollingNonceGenerator;
use rusqlite::{Connection, named_params};
use thiserror::Error;

use crate::nodeclient::sync::BlockHeader;

#[derive(Error, Debug)]
pub enum Error {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Nonce error: {0}")]
    Nonce(#[from] pallas_crypto::nonce::Error),
}

pub trait BlockStore {
    fn save_block(&mut self, pending_blocks: &mut Vec<BlockHeader>, shelley_genesis_hash: &str) -> io::Result<()>;
    fn load_blocks(&mut self) -> Option<Vec<(i64, Vec<u8>)>>;
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
                        let vkey = node_vkey.unwrap();
                        let node_vkey_bytes = hex::decode(&vkey).unwrap();
                        let pool_id = hex::encode(
                            Params::new()
                                .hash_length(28)
                                .to_state()
                                .update(&node_vkey_bytes)
                                .finalize()
                                .as_bytes(),
                        );

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
            ).unwrap().as_slice()
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
                        ).unwrap().as_slice()
                    );
                }
                // calculate rolling nonce (eta_v)
                let mut rolling_nonce_generator = RollingNonceGenerator::new(prev_eta_v);
                rolling_nonce_generator.apply_block(&block.eta_vrf_0)?;
                prev_eta_v = rolling_nonce_generator.finalize()?;

                // blake2b 224 of node_vkey is the pool_id
                let pool_id = Params::new()
                    .hash_length(28)
                    .to_state()
                    .update(&block.node_vkey)
                    .finalize()
                    .as_bytes()
                    .to_vec();

                insert_stmt.execute(named_params! {
                    ":block_number" : block.block_number,
                    ":slot_number": block.slot_number,
                    ":hash" : hex::encode(block.hash),
                    ":prev_hash" : hex::encode(block.prev_hash),
                    ":pool_id" : hex::encode(pool_id),
                    ":eta_v" : hex::encode(prev_eta_v),
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
            }
        }

        tx.commit()?;
        Ok(())
    }
}

impl BlockStore for SqLiteBlockStore {
    fn save_block(&mut self, pending_blocks: &mut Vec<BlockHeader>, shelley_genesis_hash: &str) -> io::Result<()> {
        match self.sql_save_block(pending_blocks, shelley_genesis_hash) {
            Ok(_) => Ok(()),
            Err(error) => Err(io::Error::new(io::ErrorKind::Other, format!("Database error!: {:?}", error))),
        }
    }

    fn load_blocks(&mut self) -> Option<Vec<(i64, Vec<u8>)>> {
        let db = &self.db;
        let mut stmt = db
            .prepare("SELECT slot_number, hash FROM (SELECT slot_number, hash, orphaned FROM chain ORDER BY slot_number DESC LIMIT 100) WHERE orphaned = 0 ORDER BY slot_number DESC LIMIT 33;")
            .unwrap();
        let blocks = stmt
            .query_map([], |row| {
                let slot_result: Result<i64, rusqlite::Error> = row.get(0);
                let hash_result: Result<String, rusqlite::Error> = row.get(1);
                let slot = slot_result?;
                let hash = hash_result?;
                Ok((slot, hex::decode(hash).unwrap()))
            })
            .ok()?;
        Some(blocks.map(|item| item.unwrap()).collect())
    }
}
