use std::path::Path;

use thiserror::Error;

use crate::nodeclient::blockstore::redb::{is_redb_database, RedbBlockStore};
use crate::nodeclient::blockstore::sqlite::SqLiteBlockStore;
use crate::nodeclient::blockstore::{Block, BlockStore};

#[derive(Debug, Error)]
pub enum Error {
    #[error("Invalid path: {0}")]
    InvalidPath(std::path::PathBuf),

    #[error("Redb error: {0}")]
    Redb(#[from] crate::nodeclient::blockstore::redb::Error),

    #[error("Sqlite error: {0}")]
    Sqlite(#[from] crate::nodeclient::blockstore::sqlite::Error),

    #[error("Blockstore error: {0}")]
    Blockstore(#[from] crate::nodeclient::blockstore::Error),
}

pub fn validate_block(db_path: &Path, hash: &str) {
    let like = format!("{hash}%");
    match query_block(db_path, like) {
        Ok(block) => match block {
            Some(block) => {
                println!(
                    "{{\n\
                    \x20\"status\": \"{}\",\n\
                    \x20\"block_number\": \"{}\",\n\
                    \x20\"slot_number\": \"{}\",\n\
                    \x20\"pool_id\": \"{}\",\n\
                    \x20\"hash\": \"{}\",\n\
                    \x20\"prev_hash\": \"{}\",\n\
                    \x20\"leader_vrf\": \"{}\"\n\
                    }}",
                    if block.orphaned { "orphaned" } else { "ok" },
                    block.block_number,
                    block.slot_number,
                    block.pool_id,
                    block.hash,
                    block.prev_hash,
                    block.leader_vrf,
                );
            }
            None => {
                println!(
                    "{{\n\
                    \x20\"status\": \"error\",\n\
                    \x20\"errorMessage\": \"Block not found\"\n\
                    }}"
                );
            }
        },
        Err(error) => {
            println!(
                "{{\n\
            \x20\"status\": \"error\",\n\
            \x20\"errorMessage\": \"{error}\"\n\
            }}"
            );
        }
    }
}

fn query_block(db_path: &Path, hash_start: String) -> Result<Option<Block>, Error> {
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

    Ok(block_store.find_block_by_hash(&hash_start)?)
}
