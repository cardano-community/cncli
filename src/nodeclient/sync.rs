use std::path::PathBuf;

use cardano_ouroboros_network::{
    mux,
    protocols::{
        chainsync::{ChainSyncProtocol, Mode},
        transaction::TxSubmissionProtocol,
    },
};
use futures::{
    executor::block_on,
    try_join,
};
use log::error;

use crate::nodeclient::pooltool;
use crate::nodeclient::sqlite;

pub(crate) fn sync(db: &PathBuf, host: &str, port: u16, network_magic: u32) {
    block_on(async {
        let block_store = sqlite::SQLiteBlockStore::new(db).unwrap();
        match mux::tcp::connect(host, port).await {
            Ok(channel) => {
                match channel.handshake(network_magic).await {
                    Ok(_) => {
                        try_join!(
                            channel.execute(TxSubmissionProtocol::default()),
                            channel.execute({ChainSyncProtocol {
                                mode: Mode::Sync,
                                network_magic: network_magic,
                                store: Some(Box::new(block_store)),
                                ..Default::default()
                            }}),
                        ).unwrap();
                    }
                    Err(error) => { error!("{}", error); }
                }
            }
            Err(error) => { error!("{:?}", error); }
        }
    });
}

pub(crate) fn sendtip(pool_name: String, pool_id: String, host: String, port: u16, api_key: String, cardano_node_path: PathBuf) {
    block_on(async {
        let pooltool_notifier = pooltool::PoolToolNotifier {
            pool_name,
            pool_id,
            api_key,
            cardano_node_path,
            ..Default::default()
        };
        match mux::tcp::connect(&*host, port).await {
            Ok(channel) => {
                match channel.handshake(764824073).await {
                    Ok(_) => {
                        try_join!(
                            channel.execute(TxSubmissionProtocol::default()),
                            channel.execute({ChainSyncProtocol {
                                mode: Mode::SendTip,
                                network_magic: 764824073, // hardcoded to mainnet for pooltool
                                notify: Some(Box::new(pooltool_notifier)),
                                ..Default::default()
                            }}),
                        ).unwrap();
                    }
                    Err(error) => { error!("{}", error); }
                }
            }
            Err(error) => { error!("{:?}", error); }
        }
    });
}