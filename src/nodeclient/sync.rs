use std::cmp::max;
use std::net::ToSocketAddrs;
use std::ops::Sub;
use std::path::Path;
use std::time::{Duration, Instant};

use thiserror::Error;

use log::{debug, error, info, warn};
use pallas_network::facades::{KeepAliveLoop, PeerClient, DEFAULT_KEEP_ALIVE_INTERVAL_SEC};
use pallas_network::miniprotocols::chainsync::{HeaderContent, NextResponse, Tip};
use pallas_network::miniprotocols::handshake::Confirmation;
use pallas_network::miniprotocols::{
    blockfetch, chainsync, handshake, keepalive, txsubmission, Point, MAINNET_MAGIC, PROTOCOL_N2N_BLOCK_FETCH,
    PROTOCOL_N2N_CHAIN_SYNC, PROTOCOL_N2N_HANDSHAKE, PROTOCOL_N2N_KEEP_ALIVE, PROTOCOL_N2N_TX_SUBMISSION,
};
use pallas_network::multiplexer::{Bearer, Plexer};
use pallas_traverse::MultiEraHeader;

use crate::nodeclient::pooltool;
use crate::nodeclient::sqlite;
use crate::nodeclient::sqlite::BlockStore;

use super::sqlite::SqLiteBlockStore;

const FIVE_SECS: Duration = Duration::from_secs(5);

#[derive(Error, Debug)]
pub enum Error {
    #[error("loggingobserver error occurred")]
    LoggingObserverError(String),

    #[error("pallas_traverse error occurred")]
    PallasTraverseError(#[from] pallas_traverse::Error),

    #[error("io error occurred")]
    IoError(#[from] std::io::Error),

    #[error("keepalive error occurred")]
    KeepAliveError(#[from] keepalive::ClientError),

    #[error("chainsync error occurred")]
    ChainSyncError(#[from] chainsync::ClientError),

    #[error("chainsync canceled")]
    ChainSyncCanceled,
}

#[derive(Debug, Clone)]
pub struct BlockHeader {
    pub block_number: i64,
    pub slot_number: i64,
    pub hash: Vec<u8>,
    pub prev_hash: Vec<u8>,
    pub node_vkey: Vec<u8>,
    pub node_vrf_vkey: Vec<u8>,
    pub block_vrf_0: Vec<u8>,
    pub block_vrf_1: Vec<u8>,
    pub eta_vrf_0: Vec<u8>,
    pub eta_vrf_1: Vec<u8>,
    pub leader_vrf_0: Vec<u8>,
    pub leader_vrf_1: Vec<u8>,
    pub block_size: i64,
    pub block_body_hash: Vec<u8>,
    pub pool_opcert: Vec<u8>,
    pub unknown_0: i64,
    pub unknown_1: i64,
    pub unknown_2: Vec<u8>,
    pub protocol_major_version: i64,
    pub protocol_minor_version: i64,
}

struct LoggingObserver {
    pub last_log_time: Instant,
    pub exit_when_tip_reached: bool,
    pub block_store: Option<Box<dyn BlockStore + Send>>,
    pub shelley_genesis_hash: String,
    pub pending_blocks: Vec<BlockHeader>,
}

impl Default for LoggingObserver {
    fn default() -> Self {
        LoggingObserver {
            last_log_time: Instant::now().sub(Duration::from_secs(6)),
            exit_when_tip_reached: false,
            block_store: None,
            shelley_genesis_hash: String::from("1a3be38bcbb7911969283716ad7aa550250226b76a61fc51cc9a9a35d9276d81"),
            pending_blocks: Vec::new(),
        }
    }
}

enum Continuation {
    Proceed,
    DropOut,
}

trait Observer<V> {
    fn on_roll_forward(&mut self, content: &HeaderContent, tip: &Tip) -> Result<Continuation, Error>;
    fn on_rollback(&mut self, point: &Point) -> Result<Continuation, Error>;
    fn on_tip_reached(&mut self) -> Result<Continuation, Error>;
}

impl Observer<HeaderContent> for LoggingObserver {
    fn on_roll_forward(&mut self, content: &HeaderContent, tip: &Tip) -> Result<Continuation, Error> {
        let mut result: Result<Continuation, Error> = Ok(Continuation::Proceed);
        match content.byron_prefix {
            None => {
                let multi_era_header = MultiEraHeader::decode(content.variant, None, &content.cbor);
                match multi_era_header {
                    Ok(multi_era_header) => {
                        let hash = multi_era_header.hash();
                        let slot = multi_era_header.slot();
                        let nonce_vrf_output = multi_era_header.nonce_vrf_output()?;
                        let leader_vrf_output = multi_era_header.leader_vrf_output()?;
                        match &multi_era_header {
                            MultiEraHeader::EpochBoundary(_epoch_boundary_header) => {
                                warn!("skipping epoch boundary header!")
                            }
                            MultiEraHeader::Byron(_byron_header) => {
                                warn!("skipping byron block header!");
                            }
                            MultiEraHeader::ShelleyCompatible(header) => {
                                //sqlite only handles signed values so some casting is done here
                                self.pending_blocks.push(BlockHeader {
                                    block_number: header.header_body.block_number as i64,
                                    slot_number: slot as i64,
                                    hash: hash.to_vec(),
                                    prev_hash: match header.header_body.prev_hash {
                                        None => vec![],
                                        Some(prev_hash) => prev_hash.to_vec(),
                                    },
                                    node_vkey: header.header_body.issuer_vkey.to_vec(),
                                    node_vrf_vkey: header.header_body.vrf_vkey.to_vec(),
                                    block_vrf_0: vec![],
                                    block_vrf_1: vec![],
                                    eta_vrf_0: nonce_vrf_output,
                                    eta_vrf_1: header.header_body.nonce_vrf.1.to_vec(),
                                    leader_vrf_0: leader_vrf_output,
                                    leader_vrf_1: header.header_body.leader_vrf.1.to_vec(),
                                    block_size: header.header_body.block_body_size as i64,
                                    block_body_hash: header.header_body.block_body_hash.to_vec(),
                                    pool_opcert: header.header_body.operational_cert_hot_vkey.to_vec(),
                                    unknown_0: header.header_body.operational_cert_sequence_number as i64,
                                    unknown_1: header.header_body.operational_cert_kes_period as i64,
                                    unknown_2: header.header_body.operational_cert_sigma.to_vec(),
                                    protocol_major_version: header.header_body.protocol_major as i64,
                                    protocol_minor_version: header.header_body.protocol_minor as i64,
                                });
                                let block_number: f64 = header.header_body.block_number as f64;
                                let tip_block_number: f64 = tip.1 as f64;
                                let is_tip = header.header_body.block_number >= tip.1;
                                if is_tip || self.last_log_time.elapsed() > FIVE_SECS {
                                    match self.block_store.as_mut() {
                                        None => {}
                                        Some(store) => {
                                            store.save_block(&mut self.pending_blocks, &self.shelley_genesis_hash)?;
                                        }
                                    }

                                    info!(
                                        "block {} of {}: {:>6.*}% sync'd",
                                        header.header_body.block_number,
                                        tip.1,
                                        2,
                                        (block_number / tip_block_number * 10000.0).floor() / 100.0,
                                    );
                                    self.last_log_time = Instant::now();
                                }
                                if is_tip {
                                    result = self.on_tip_reached();
                                }
                            }
                            MultiEraHeader::BabbageCompatible(header) => {
                                //sqlite only handles signed values so some casting is done here
                                self.pending_blocks.push(BlockHeader {
                                    block_number: header.header_body.block_number as i64,
                                    slot_number: slot as i64,
                                    hash: hash.to_vec(),
                                    prev_hash: match header.header_body.prev_hash {
                                        None => vec![],
                                        Some(prev_hash) => prev_hash.to_vec(),
                                    },
                                    node_vkey: header.header_body.issuer_vkey.to_vec(),
                                    node_vrf_vkey: header.header_body.vrf_vkey.to_vec(),
                                    block_vrf_0: header.header_body.vrf_result.0.to_vec(),
                                    block_vrf_1: header.header_body.vrf_result.1.to_vec(),
                                    eta_vrf_0: nonce_vrf_output,
                                    eta_vrf_1: vec![],
                                    leader_vrf_0: leader_vrf_output,
                                    leader_vrf_1: vec![],
                                    block_size: header.header_body.block_body_size as i64,
                                    block_body_hash: header.header_body.block_body_hash.to_vec(),
                                    pool_opcert: header.header_body.operational_cert.operational_cert_hot_vkey.to_vec(),
                                    unknown_0: header.header_body.operational_cert.operational_cert_sequence_number
                                        as i64,
                                    unknown_1: header.header_body.operational_cert.operational_cert_kes_period as i64,
                                    unknown_2: header.header_body.operational_cert.operational_cert_sigma.to_vec(),
                                    protocol_major_version: header.header_body.protocol_version.0 as i64,
                                    protocol_minor_version: header.header_body.protocol_version.1 as i64,
                                });
                                let block_number: f64 = header.header_body.block_number as f64;
                                let tip_block_number = max(header.header_body.block_number, tip.1);
                                let tip_f64: f64 = tip_block_number as f64;
                                let is_tip = header.header_body.block_number >= tip.1;
                                if is_tip || self.last_log_time.elapsed() > FIVE_SECS {
                                    match self.block_store.as_mut() {
                                        None => {}
                                        Some(store) => {
                                            store.save_block(&mut self.pending_blocks, &self.shelley_genesis_hash)?;
                                        }
                                    }

                                    info!(
                                        "block {} of {}: {:>6.*}% sync'd",
                                        header.header_body.block_number,
                                        tip_block_number,
                                        2,
                                        (block_number / tip_f64 * 10000.0).floor() / 100.0,
                                    );
                                    self.last_log_time = Instant::now();
                                }
                                if is_tip {
                                    result = self.on_tip_reached();
                                }
                            }
                        }
                    }
                    Err(error) => {
                        error!("{:?}", error);
                        std::process::exit(1);
                    }
                }
            }
            Some(_) => {
                warn!("skipping byron block!");
            }
        }

        result
    }

    fn on_rollback(&mut self, point: &Point) -> Result<Continuation, Error> {
        debug!("asked to roll back {:?}", point);

        Ok(Continuation::Proceed)
    }

    fn on_tip_reached(&mut self) -> Result<Continuation, Error> {
        debug!("tip was reached");
        if self.exit_when_tip_reached {
            info!("Exiting...");
            Ok(Continuation::DropOut)
        } else {
            Ok(Continuation::Proceed)
        }
    }
}

fn get_intersect_blocks(block_store: &mut SqLiteBlockStore) -> Result<Vec<Point>, Error> {
    let start = Instant::now();
    debug!("get_intersect_blocks");

    let mut chain_blocks: Vec<Point> = vec![];

    /* Classic sync: Use blocks from store if available. */
    match block_store.load_blocks() {
        None => {}
        Some(blocks) => {
            for (i, block) in blocks.iter().enumerate() {
                // all powers of 2 including 0th element 0, 2, 4, 8, 16, 32
                if (i == 0) || ((i > 1) && (i & (i - 1) == 0)) {
                    chain_blocks.push(Point::Specific(block.0 as u64, block.1.clone()));
                }
            }
        }
    };

    // add known points
    chain_blocks.push(
        // Last byron block of mainnet
        Point::Specific(
            4492799,
            hex::decode("f8084c61b6a238acec985b59310b6ecec49c0ab8352249afd7268da5cff2a457").unwrap(),
        ),
    );
    chain_blocks.push(
        // Last byron block of testnet
        Point::Specific(
            1598399,
            hex::decode("7e16781b40ebf8b6da18f7b5e8ade855d6738095ef2f1c58c77e88b6e45997a4").unwrap(),
        ),
    );
    chain_blocks.push(
        // Last byron block of guild
        Point::Specific(
            719,
            hex::decode("e5400faf19e712ebc5ff5b4b44cecb2b140d1cca25a011e36a91d89e97f53e2e").unwrap(),
        ),
    );
    chain_blocks.push(
        // Last byron block of vasil-dev
        Point::Specific(
            359,
            hex::decode("87882b6778a831d0f19f03ee3fb5e95081afa835976abc1b8dd6f7b65421a816").unwrap(),
        ),
    );
    chain_blocks.push(Point::Origin);

    info!("get_intersect_blocks took: {:?}", start.elapsed());

    Ok(chain_blocks)
}

async fn do_chainsync(
    mut client: chainsync::N2NClient,
    skip_to_tip: bool,
    exit_when_tip_reached: bool,
    chain_blocks: Option<Vec<Point>>,
    block_store: Option<Box<dyn BlockStore + 'static + Send>>,
    shelley_genesis_hash: String,
) -> Result<(), Error> {
    if skip_to_tip {
        client.intersect_tip().await?;
    } else {
        client.find_intersect(chain_blocks.unwrap()).await?;
    }

    let mut logging_observer = LoggingObserver {
        exit_when_tip_reached,
        block_store,
        shelley_genesis_hash,
        ..Default::default()
    };
    let mut next = client.request_next().await?;
    loop {
        match &next {
            NextResponse::RollForward(header_content, tip) => {
                match logging_observer.on_roll_forward(header_content, tip)? {
                    Continuation::Proceed => next = client.request_next().await?,
                    Continuation::DropOut => {
                        client.send_done().await?;
                        return Ok(());
                    }
                }
            }
            NextResponse::RollBackward(point, _tip) => match logging_observer.on_rollback(point)? {
                Continuation::Proceed => next = client.request_next().await?,
                Continuation::DropOut => {
                    client.send_done().await?;
                    return Ok(());
                }
            },
            NextResponse::Await => next = client.recv_while_must_reply().await?,
        }
    }
}

pub(crate) async fn sync(
    db: &Path,
    host: &str,
    port: u16,
    network_magic: u64,
    shelley_genesis_hash: &str,
    no_service: bool,
) {
    loop {
        // Retry to establish connection forever
        let mut block_store = sqlite::SqLiteBlockStore::new(db).unwrap();
        let chain_blocks = get_intersect_blocks(&mut block_store).unwrap();
        match Bearer::connect_tcp_timeout(
            &format!("{host}:{port}").to_socket_addrs().unwrap().next().unwrap(),
            FIVE_SECS,
        )
        .await
        {
            Ok(bearer) => {
                let mut plexer = Plexer::new(bearer);

                let channel = plexer.subscribe_client(PROTOCOL_N2N_HANDSHAKE);
                let mut handshake = handshake::Client::new(channel);

                let cs_channel = plexer.subscribe_client(PROTOCOL_N2N_CHAIN_SYNC);
                let bf_channel = plexer.subscribe_client(PROTOCOL_N2N_BLOCK_FETCH);
                let txsub_channel = plexer.subscribe_client(PROTOCOL_N2N_TX_SUBMISSION);

                let ka_channel = plexer.subscribe_client(PROTOCOL_N2N_KEEP_ALIVE);
                let keepalive = keepalive::Client::new(ka_channel);

                let plexer = plexer.spawn();

                let versions = handshake::n2n::VersionTable::v7_and_above(network_magic);
                let handshake = handshake.handshake(versions).await;

                match handshake {
                    Ok(confirmation) => match confirmation {
                        Confirmation::Accepted(_, _) => {
                            let keepalive =
                                KeepAliveLoop::client(keepalive, Duration::from_secs(DEFAULT_KEEP_ALIVE_INTERVAL_SEC))
                                    .spawn();

                            let peer = PeerClient {
                                plexer,
                                keepalive,
                                chainsync: chainsync::Client::new(cs_channel),
                                blockfetch: blockfetch::Client::new(bf_channel),
                                txsubmission: txsubmission::Client::new(txsub_channel),
                            };

                            let PeerClient {
                                plexer,
                                keepalive: _keepalive,
                                chainsync,
                                blockfetch: _blockfetch,
                                txsubmission: _txsubmission,
                            } = peer;

                            let shelley_genesis_hash = shelley_genesis_hash.to_string();
                            do_chainsync(
                                chainsync,
                                false,
                                no_service,
                                Some(chain_blocks),
                                Some(Box::new(block_store)),
                                shelley_genesis_hash,
                            )
                            .await
                            .unwrap();

                            plexer.abort().await;
                        }
                        Confirmation::Rejected(refuse_reason) => {
                            error!("{:?}", refuse_reason);
                        }
                        Confirmation::QueryReply(_) => {
                            error!("Unexpected QueryReply");
                        }
                    },
                    Err(error) => {
                        error!("{:?}", error);
                    }
                }
            }
            Err(error) => {
                error!("{}", error);
            }
        }

        if no_service {
            break;
        }

        warn!("Disconnected... retry in 5 secs...");
        tokio::time::sleep(FIVE_SECS).await;
    }
}

pub(crate) async fn sendtip(
    pool_name: String,
    pool_id: String,
    host: String,
    port: u16,
    api_key: String,
    cardano_node_path: &Path,
) {
    loop {
        let pooltool_notifier = pooltool::PoolToolNotifier {
            pool_name: pool_name.clone(),
            pool_id: pool_id.clone(),
            api_key: api_key.clone(),
            cardano_node_path: cardano_node_path.to_path_buf(),
            ..Default::default()
        };
        match Bearer::connect_tcp_timeout(
            &format!("{host}:{port}").to_socket_addrs().unwrap().next().unwrap(),
            FIVE_SECS,
        )
        .await
        {
            Ok(bearer) => {
                let mut plexer = Plexer::new(bearer);

                let channel = plexer.subscribe_client(PROTOCOL_N2N_HANDSHAKE);
                let mut handshake = handshake::Client::new(channel);

                let cs_channel = plexer.subscribe_client(PROTOCOL_N2N_CHAIN_SYNC);
                let bf_channel = plexer.subscribe_client(PROTOCOL_N2N_BLOCK_FETCH);
                let txsub_channel = plexer.subscribe_client(PROTOCOL_N2N_TX_SUBMISSION);

                let ka_channel = plexer.subscribe_client(PROTOCOL_N2N_KEEP_ALIVE);
                let keepalive = keepalive::Client::new(ka_channel);

                let plexer = plexer.spawn();

                let versions = handshake::n2n::VersionTable::v7_and_above(MAINNET_MAGIC);
                let handshake = handshake.handshake(versions).await;

                match handshake {
                    Ok(confirmation) => match confirmation {
                        Confirmation::Accepted(_, _) => {
                            let keepalive =
                                KeepAliveLoop::client(keepalive, Duration::from_secs(DEFAULT_KEEP_ALIVE_INTERVAL_SEC))
                                    .spawn();

                            let peer = PeerClient {
                                plexer,
                                keepalive,
                                chainsync: chainsync::Client::new(cs_channel),
                                blockfetch: blockfetch::Client::new(bf_channel),
                                txsubmission: txsubmission::Client::new(txsub_channel),
                            };

                            let PeerClient {
                                plexer,
                                keepalive: _keepalive,
                                chainsync,
                                blockfetch: _blockfetch,
                                txsubmission: _txsubmission,
                            } = peer;

                            do_chainsync(
                                chainsync,
                                false,
                                false,
                                None,
                                Some(Box::new(pooltool_notifier)),
                                "1a3be38bcbb7911969283716ad7aa550250226b76a61fc51cc9a9a35d9276d81".to_string(),
                            )
                            .await
                            .unwrap();

                            plexer.abort().await;
                        }
                        Confirmation::Rejected(refuse_reason) => {
                            error!("{:?}", refuse_reason);
                        }
                        Confirmation::QueryReply(_) => {
                            error!("Unexpected QueryReply");
                        }
                    },
                    Err(error) => {
                        error!("{:?}", error);
                    }
                }
            }
            Err(error) => {
                error!("{}", error);
            }
        }

        warn!("Disconnected... retry in 5 secs...");
        tokio::time::sleep(FIVE_SECS).await;
    }
}
