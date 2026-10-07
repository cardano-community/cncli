use std::path::Path;

use thiserror::Error;

use crate::nodeclient::blockstore::redb::{is_redb_database, RedbBlockStore};
use crate::nodeclient::blockstore::sqlite::SqLiteBlockStore;
use crate::nodeclient::blockstore::{Block, BlockStore};

#[derive(Error, Debug)]
pub enum Error {
    #[error("Invalid path: {0}")]
    InvalidPath(std::path::PathBuf),

    #[error("Redb error: {0}")]
    Redb(#[source] Box<crate::nodeclient::blockstore::redb::Error>),

    #[error("Sqlite error: {0}")]
    Sqlite(#[from] crate::nodeclient::blockstore::sqlite::Error),

    #[error("Blockstore error: {0}")]
    Blockstore(#[source] Box<crate::nodeclient::blockstore::Error>),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Output error: {0}")]
    Output(#[from] crate::OutputError),

    #[error("Block not found")]
    BlockNotFound,
}

impl From<crate::nodeclient::blockstore::redb::Error> for Error {
    fn from(err: crate::nodeclient::blockstore::redb::Error) -> Self {
        Error::Redb(Box::new(err))
    }
}

impl From<crate::nodeclient::blockstore::Error> for Error {
    fn from(err: crate::nodeclient::blockstore::Error) -> Self {
        Error::Blockstore(Box::new(err))
    }
}

pub fn validate_block(db_path: &Path, hash: &str) -> Result<(), Error> {
    let block = query_block(db_path, hash)?.ok_or(Error::BlockNotFound)?;
    crate::write_json(
        &mut std::io::stdout().lock(),
        &serde_json::json!({
            "status": if block.orphaned { "orphaned" } else { "ok" },
            "block_number": block.block_number.to_string(),
            "slot_number": block.slot_number.to_string(),
            "pool_id": block.pool_id,
            "hash": block.hash,
            "prev_hash": block.prev_hash,
            "leader_vrf": block.leader_vrf,
        }),
    )?;
    Ok(())
}

fn query_block(db_path: &Path, hash_start: &str) -> Result<Option<Block>, Error> {
    if !db_path.exists() {
        return Err(Error::InvalidPath(db_path.to_path_buf()));
    }
    // check if db_path is a redb database based on magic number
    let use_redb = is_redb_database(db_path)?;

    let mut block_store: Box<dyn BlockStore + Send> = if use_redb {
        Box::new(RedbBlockStore::new(db_path)?)
    } else {
        Box::new(SqLiteBlockStore::new(db_path)?)
    };

    Ok(block_store.find_block_by_hash(hash_start)?)
}
