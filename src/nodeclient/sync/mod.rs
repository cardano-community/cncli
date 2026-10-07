use std::cmp::max;
use std::future::Future;
use std::ops::Sub;
use std::path::Path;
use std::time::{Duration, Instant};

use pallas_network::facades::{KeepAliveLoop, PeerClient, DEFAULT_KEEP_ALIVE_INTERVAL_SEC};
use pallas_network::miniprotocols::chainsync::{HeaderContent, NextResponse, Tip};
use pallas_network::miniprotocols::handshake::Confirmation;
use pallas_network::miniprotocols::{
    blockfetch, chainsync, handshake, keepalive, peersharing, txsubmission, Point, MAINNET_MAGIC,
    PROTOCOL_N2N_BLOCK_FETCH, PROTOCOL_N2N_CHAIN_SYNC, PROTOCOL_N2N_HANDSHAKE, PROTOCOL_N2N_KEEP_ALIVE,
    PROTOCOL_N2N_PEER_SHARING, PROTOCOL_N2N_TX_SUBMISSION,
};
use pallas_network::multiplexer::{Bearer, Plexer};
use pallas_traverse::MultiEraHeader;
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::nodeclient::blockstore;
use crate::nodeclient::blockstore::redb::RedbBlockStore;
use crate::nodeclient::blockstore::sqlite::SqLiteBlockStore;
use crate::nodeclient::blockstore::BlockStore;

pub(crate) mod pooltool;

const FIVE_SECS: Duration = Duration::from_secs(5);

const BOOTSTRAP_POINTS: [(u64, [u8; 32]); 4] = [
    (
        4492799,
        [
            0xf8, 0x08, 0x4c, 0x61, 0xb6, 0xa2, 0x38, 0xac, 0xec, 0x98, 0x5b, 0x59, 0x31, 0x0b, 0x6e, 0xce, 0xc4, 0x9c,
            0x0a, 0xb8, 0x35, 0x22, 0x49, 0xaf, 0xd7, 0x26, 0x8d, 0xa5, 0xcf, 0xf2, 0xa4, 0x57,
        ],
    ),
    (
        1598399,
        [
            0x7e, 0x16, 0x78, 0x1b, 0x40, 0xeb, 0xf8, 0xb6, 0xda, 0x18, 0xf7, 0xb5, 0xe8, 0xad, 0xe8, 0x55, 0xd6, 0x73,
            0x80, 0x95, 0xef, 0x2f, 0x1c, 0x58, 0xc7, 0x7e, 0x88, 0xb6, 0xe4, 0x59, 0x97, 0xa4,
        ],
    ),
    (
        719,
        [
            0xe5, 0x40, 0x0f, 0xaf, 0x19, 0xe7, 0x12, 0xeb, 0xc5, 0xff, 0x5b, 0x4b, 0x44, 0xce, 0xcb, 0x2b, 0x14, 0x0d,
            0x1c, 0xca, 0x25, 0xa0, 0x11, 0xe3, 0x6a, 0x91, 0xd8, 0x9e, 0x97, 0xf5, 0x3e, 0x2e,
        ],
    ),
    (
        359,
        [
            0x87, 0x88, 0x2b, 0x67, 0x78, 0xa8, 0x31, 0xd0, 0xf1, 0x9f, 0x03, 0xee, 0x3f, 0xb5, 0xe9, 0x50, 0x81, 0xaf,
            0xa8, 0x35, 0x97, 0x6a, 0xbc, 0x1b, 0x8d, 0xd6, 0xf7, 0xb6, 0x54, 0x21, 0xa8, 0x16,
        ],
    ),
];

fn bootstrap_points() -> Vec<Point> {
    BOOTSTRAP_POINTS
        .iter()
        .map(|(slot, hash)| Point::Specific(*slot, hash.to_vec()))
        .collect()
}

fn is_bootstrap_point(point: &Point) -> bool {
    match point {
        Point::Specific(slot, hash) => BOOTSTRAP_POINTS
            .iter()
            .any(|(known_slot, known_hash)| slot == known_slot && hash.as_slice() == known_hash),
        Point::Origin => false,
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("pallas_traverse error occurred: {0}")]
    PallasTraverse(#[from] pallas_traverse::Error),

    #[error("io error occurred: {0}")]
    Io(#[from] std::io::Error),

    #[error("keepalive error occurred: {0}")]
    KeepAlive(#[from] keepalive::ClientError),

    #[error("chainsync error occurred: {0}")]
    ChainSync(#[from] chainsync::ClientError),

    #[error("blockstore error occurred: {0}")]
    BlockStore(#[source] Box<blockstore::Error>),
    #[error("handshake refused: {0}")]
    Handshake(String),
    #[error("handshake protocol error: {0}")]
    HandshakeProtocol(#[from] handshake::Error),
    #[error("{0} timed out")]
    Timeout(&'static str),
    #[error("reporter configuration error: {0}")]
    Reporter(String),
    #[error("peer error: {0}")]
    Peer(#[from] pallas_network::facades::Error),
    #[error("task error: {0}")]
    Task(#[from] tokio::task::JoinError),
}

impl From<blockstore::Error> for Error {
    fn from(err: blockstore::Error) -> Self {
        Error::BlockStore(Box::new(err))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BlockHeader {
    pub block_number: u64,
    pub slot_number: u64,
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
    pub block_size: u64,
    pub block_body_hash: Vec<u8>,
    pub pool_opcert: Vec<u8>,
    pub unknown_0: u64,
    pub unknown_1: u64,
    pub unknown_2: Vec<u8>,
    pub protocol_major_version: u64,
    pub protocol_minor_version: u64,
}

enum ChainSink {
    Store {
        store: Box<dyn BlockStore + Send>,
        pending: Vec<BlockHeader>,
        shelley_genesis_hash: String,
    },
    Tip(tokio::sync::watch::Sender<Option<BlockHeader>>),
}

struct ChainObserver {
    sink: ChainSink,
    last_log_time: Instant,
    exit_when_tip_reached: bool,
}

impl ChainObserver {
    fn new(sink: ChainSink, exit_when_tip_reached: bool) -> Self {
        Self {
            sink,
            last_log_time: Instant::now().sub(Duration::from_secs(6)),
            exit_when_tip_reached,
        }
    }

    fn disconnected(&mut self) {
        match &mut self.sink {
            ChainSink::Store { pending, .. } => pending.clear(),
            ChainSink::Tip(sender) => {
                sender.send_replace(None);
            }
        }
    }

    fn on_header(&mut self, header: BlockHeader, tip: &Tip) -> Result<Continuation, Error> {
        let block_number = header.block_number;
        let is_tip = block_number >= tip.1;
        let log_due = is_tip || self.last_log_time.elapsed() > FIVE_SECS;
        match &mut self.sink {
            ChainSink::Store {
                store,
                pending,
                shelley_genesis_hash,
            } => {
                pending.push(header);
                if log_due || pending.len() >= 1024 {
                    store.save_block(pending, shelley_genesis_hash)?;
                    pending.clear();
                }
            }
            ChainSink::Tip(sender) => {
                sender.send_replace(Some(header));
            }
        }
        if log_due {
            let tip_number = max(block_number, tip.1);
            info!(
                "block {} of {}: {:.2}% sync'd",
                block_number,
                tip_number,
                if tip_number == 0 {
                    100.0
                } else {
                    (block_number as f64 / tip_number as f64 * 10000.0).floor() / 100.0
                }
            );
            self.last_log_time = Instant::now();
        }
        if is_tip {
            self.on_tip_reached()
        } else {
            Ok(Continuation::Proceed)
        }
    }
}

enum Continuation {
    Proceed,
    DropOut,
}

impl ChainObserver {
    fn on_roll_forward(&mut self, content: &HeaderContent, tip: &Tip) -> Result<Continuation, Error> {
        let mut result: Result<Continuation, Error> = Ok(Continuation::Proceed);
        match content.byron_prefix {
            None => {
                let multi_era_header = MultiEraHeader::decode(content.variant, None, &content.cbor)?;
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
                        result = self.on_header(
                            BlockHeader {
                                block_number: header.header_body.block_number,
                                slot_number: slot,
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
                                block_size: header.header_body.block_body_size,
                                block_body_hash: header.header_body.block_body_hash.to_vec(),
                                pool_opcert: header.header_body.operational_cert_hot_vkey.to_vec(),
                                unknown_0: header.header_body.operational_cert_sequence_number,
                                unknown_1: header.header_body.operational_cert_kes_period,
                                unknown_2: header.header_body.operational_cert_sigma.to_vec(),
                                protocol_major_version: header.header_body.protocol_major,
                                protocol_minor_version: header.header_body.protocol_minor,
                            },
                            tip,
                        );
                    }
                    MultiEraHeader::BabbageCompatible(header) => {
                        //sqlite only handles signed values so some casting is done here
                        result = self.on_header(
                            BlockHeader {
                                block_number: header.header_body.block_number,
                                slot_number: slot,
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
                                block_size: header.header_body.block_body_size,
                                block_body_hash: header.header_body.block_body_hash.to_vec(),
                                pool_opcert: header.header_body.operational_cert.operational_cert_hot_vkey.to_vec(),
                                unknown_0: header.header_body.operational_cert.operational_cert_sequence_number,
                                unknown_1: header.header_body.operational_cert.operational_cert_kes_period,
                                unknown_2: header.header_body.operational_cert.operational_cert_sigma.to_vec(),
                                protocol_major_version: header.header_body.protocol_version.0,
                                protocol_minor_version: header.header_body.protocol_version.1,
                            },
                            tip,
                        );
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
        let durable_point = if is_bootstrap_point(point) {
            &Point::Origin
        } else {
            point
        };
        match &mut self.sink {
            ChainSink::Store { store, pending, .. } => {
                if let Point::Specific(slot, hash) = durable_point {
                    if let Some(index) = pending
                        .iter()
                        .position(|header| header.slot_number == *slot && &header.hash == hash)
                    {
                        pending.truncate(index + 1);
                        return Ok(Continuation::Proceed);
                    }
                }
                store.rollback_to(durable_point)?;
                pending.clear();
            }
            ChainSink::Tip(sender) => {
                sender.send_replace(None);
            }
        }

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

fn get_intersect_blocks(block_store: &mut dyn BlockStore) -> Result<Vec<Point>, Error> {
    let start = Instant::now();
    debug!("get_intersect_blocks");

    let mut chain_blocks: Vec<Point> = vec![];

    /* Classic sync: Use blocks from store if available. */
    let blocks = block_store.load_blocks()?;
    for (i, (slot, hash)) in blocks.iter().enumerate() {
        // Tip, then exponentially spaced ancestors (including its immediate parent).
        if i == 0 || i.is_power_of_two() {
            chain_blocks.push(Point::Specific(*slot, hash.clone()));
        }
    }

    chain_blocks.extend(bootstrap_points());
    chain_blocks.push(Point::Origin);

    info!("get_intersect_blocks took: {:?}", start.elapsed());

    Ok(chain_blocks)
}

async fn do_chainsync(client: &mut chainsync::N2NClient, observer: &mut ChainObserver) -> Result<(), Error> {
    match &mut observer.sink {
        ChainSink::Store { store, .. } => {
            let points = get_intersect_blocks(store.as_mut())?;
            let (point, tip) = client.find_intersect(points).await?;
            let point = point.ok_or(chainsync::ClientError::IntersectionNotFound)?;
            observer.on_rollback(&point)?;
            if observer.exit_when_tip_reached && point == tip.0 {
                client.send_done().await?;
                return Ok(());
            }
        }
        ChainSink::Tip(_) => {
            client.intersect_tip().await?;
        }
    }
    let mut next = client.request_next().await?;
    loop {
        match &next {
            NextResponse::RollForward(content, tip) => match observer.on_roll_forward(content, tip)? {
                Continuation::Proceed => next = client.request_next().await?,
                Continuation::DropOut => {
                    client.send_done().await?;
                    return Ok(());
                }
            },
            NextResponse::RollBackward(point, _) => {
                observer.on_rollback(point)?;
                next = client.request_next().await?;
            }
            NextResponse::Await => next = client.recv_while_must_reply().await?,
        }
    }
}

// ponytail: locked Pallas 1.3.0 abort never suspends; replace this guard if abort gains an await.
// One poll aborts both plexer tasks when the owning command future is cancelled.
struct SessionCleanup {
    plexer: Option<pallas_network::multiplexer::RunningPlexer>,
    keepalive: Option<tokio::task::AbortHandle>,
}

impl SessionCleanup {
    async fn abort(&mut self) {
        if let Some(plexer) = self.plexer.take() {
            plexer.abort().await;
        }
    }
}

impl Drop for SessionCleanup {
    fn drop(&mut self) {
        if let Some(handle) = &self.keepalive {
            handle.abort();
        }
        if let Some(plexer) = self.plexer.take() {
            let abort = std::pin::pin!(plexer.abort());
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            let _ = Future::poll(abort, &mut context);
        }
    }
}

async fn connect_peer(host: &str, port: u16, network_magic: u64) -> Result<PeerClient, Error> {
    let bearer = tokio::time::timeout(FIVE_SECS, async {
        let addresses = tokio::net::lookup_host((host, port)).await?;
        let mut last_error = std::io::Error::new(std::io::ErrorKind::NotFound, "peer resolved to no addresses");
        for address in addresses {
            match Bearer::connect_tcp(address).await {
                Ok(bearer) => return Ok::<_, std::io::Error>(bearer),
                Err(error) => last_error = error,
            }
        }
        Err(last_error)
    })
    .await
    .map_err(|_| Error::Timeout("peer establishment"))??;
    let mut plexer = Plexer::new(bearer);
    let mut handshake = handshake::Client::new(plexer.subscribe_client(PROTOCOL_N2N_HANDSHAKE));
    let cs_channel = plexer.subscribe_client(PROTOCOL_N2N_CHAIN_SYNC);
    let bf_channel = plexer.subscribe_client(PROTOCOL_N2N_BLOCK_FETCH);
    let txsub_channel = plexer.subscribe_client(PROTOCOL_N2N_TX_SUBMISSION);
    let peersharing_channel = plexer.subscribe_client(PROTOCOL_N2N_PEER_SHARING);
    let keepalive = keepalive::Client::new(plexer.subscribe_client(PROTOCOL_N2N_KEEP_ALIVE));
    let mut cleanup = SessionCleanup {
        plexer: Some(plexer.spawn()),
        keepalive: None,
    };
    let confirmation = tokio::time::timeout(
        FIVE_SECS,
        handshake.handshake(handshake::n2n::VersionTable::v7_and_above(network_magic)),
    )
    .await;
    let result = match confirmation {
        Ok(Ok(Confirmation::Accepted(_, _))) => Ok(()),
        Ok(Ok(Confirmation::Rejected(reason))) => Err(Error::Handshake(format!("{reason:?}"))),
        Ok(Ok(Confirmation::QueryReply(_))) => Err(Error::Handshake("unexpected query reply".into())),
        Ok(Err(error)) => Err(error.into()),
        Err(_) => Err(Error::Timeout("peer handshake")),
    };
    if let Err(error) = result {
        cleanup.abort().await;
        return Err(error);
    }
    Ok(PeerClient {
        plexer: cleanup
            .plexer
            .take()
            .ok_or_else(|| Error::Reporter("missing peer transport".into()))?,
        keepalive: KeepAliveLoop::client(keepalive, Duration::from_secs(DEFAULT_KEEP_ALIVE_INTERVAL_SEC)).spawn(),
        chainsync: chainsync::Client::new(cs_channel),
        blockfetch: blockfetch::Client::new(bf_channel),
        txsubmission: txsubmission::Client::new(txsub_channel),
        peersharing: peersharing::Client::new(peersharing_channel),
    })
}

fn retryable(error: &Error) -> bool {
    match error {
        Error::Io(_) | Error::Timeout(_) => true,
        Error::ChainSync(chainsync::ClientError::Plexer(_) | chainsync::ClientError::InvalidInbound) => true,
        Error::HandshakeProtocol(handshake::Error::Plexer(_) | handshake::Error::InvalidInbound) => true,
        Error::KeepAlive(
            keepalive::ClientError::Plexer(_)
            | keepalive::ClientError::InvalidInbound
            | keepalive::ClientError::KeepAliveCookieMismatch,
        ) => true,
        Error::Peer(
            pallas_network::facades::Error::PlexerFailure(_) | pallas_network::facades::Error::ConnectFailure(_),
        ) => true,
        Error::Peer(pallas_network::facades::Error::KeepAliveClientLoop(error)) => matches!(
            error,
            keepalive::ClientError::Plexer(_)
                | keepalive::ClientError::InvalidInbound
                | keepalive::ClientError::KeepAliveCookieMismatch
        ),
        _ => false,
    }
}

async fn stop_peer(
    mut cleanup: SessionCleanup,
    keepalive: pallas_network::facades::KeepAliveHandle,
    keepalive_consumed: bool,
    outcome: Result<(), Error>,
) -> Result<(), Error> {
    keepalive.abort();
    let stopped = if keepalive_consumed {
        Ok(())
    } else {
        match keepalive.await {
            Err(error) if error.is_cancelled() => Ok(()),
            result => keepalive_result(result),
        }
    };
    cleanup.abort().await;
    match stopped {
        Err(error) if !retryable(&error) || outcome.is_ok() => Err(error),
        _ => outcome,
    }
}

async fn chain_session(peer: PeerClient, observer: &mut ChainObserver) -> Result<(), Error> {
    let PeerClient {
        plexer,
        mut keepalive,
        mut chainsync,
        ..
    } = peer;
    let cleanup = SessionCleanup {
        plexer: Some(plexer),
        keepalive: Some(keepalive.abort_handle()),
    };
    let mut keepalive_consumed = false;
    let result = tokio::select! {
        result = do_chainsync(&mut chainsync, observer) => result,
        result = &mut keepalive => {
            keepalive_consumed = true;
            keepalive_result(result)
        },
    };
    let result = stop_peer(cleanup, keepalive, keepalive_consumed, result).await;
    observer.disconnected();
    result
}

fn keepalive_result(
    result: Result<Result<(), pallas_network::facades::Error>, tokio::task::JoinError>,
) -> Result<(), Error> {
    match result {
        Ok(Ok(())) => Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "keepalive ended",
        ))),
        Ok(Err(error)) => Err(error.into()),
        Err(error) => Err(error.into()),
    }
}

async fn chain_loop(
    observer: &mut ChainObserver,
    host: &str,
    port: u16,
    network_magic: u64,
    no_service: bool,
) -> Result<(), Error> {
    loop {
        let result = match connect_peer(host, port, network_magic).await {
            Ok(peer) => chain_session(peer, observer).await,
            Err(error) => Err(error),
        };
        match result {
            Ok(()) if no_service => return Ok(()),
            Err(error) if no_service || !retryable(&error) => return Err(error),
            Err(error) => warn!(error = %error, "Disconnected; retry in 5 secs"),
            Ok(()) => warn!("Disconnected; retry in 5 secs"),
        }
        tokio::time::sleep(FIVE_SECS).await;
    }
}

pub(crate) async fn sync(
    db: &Path,
    host: &str,
    port: u16,
    network_magic: u64,
    shelley_genesis_hash: &str,
    no_service: bool,
    use_redb: bool,
) -> Result<(), Error> {
    if hex::decode(shelley_genesis_hash).map_or(true, |hash| hash.len() != 32) {
        return Err(Error::Reporter("Shelley genesis hash must be 32-byte hex".into()));
    }
    let store: Box<dyn BlockStore + Send> = if use_redb {
        Box::new(RedbBlockStore::new(db).map_err(blockstore::Error::from)?)
    } else {
        Box::new(SqLiteBlockStore::new(db).map_err(blockstore::Error::from)?)
    };
    let mut observer = ChainObserver::new(
        ChainSink::Store {
            store,
            pending: Vec::new(),
            shelley_genesis_hash: shelley_genesis_hash.to_owned(),
        },
        no_service,
    );
    chain_loop(&mut observer, host, port, network_magic, no_service).await
}

pub(crate) async fn sendtip(
    pool_name: String,
    pool_id: String,
    host: String,
    port: u16,
    api_key: String,
    cardano_node_path: &Path,
    client: reqwest::Client,
) -> Result<(), Error> {
    let mut notifier = pooltool::PoolToolNotifier::new(
        pool_name,
        pool_id,
        api_key,
        cardano_node_path.to_path_buf(),
        client,
        pooltool::POOLTOOL_BASE_URL.to_owned(),
    )?;
    let (sender, receiver) = tokio::sync::watch::channel(None);
    let mut observer = ChainObserver::new(ChainSink::Tip(sender), false);
    let reporter = notifier.run(receiver);
    tokio::pin!(reporter);
    loop {
        let connection = connect_peer(&host, port, MAINNET_MAGIC);
        tokio::pin!(connection);
        let connection = tokio::select! {
            result = &mut connection => result,
            result = &mut reporter => {
                // Finish the bounded handshake so its running plexer is never detached.
                if let Ok(peer) = connection.await {
                    let cleanup = SessionCleanup { plexer: Some(peer.plexer), keepalive: Some(peer.keepalive.abort_handle()) };
                    return stop_peer(cleanup, peer.keepalive, false, result).await;
                }
                return result;
            }
        };
        let result = match connection {
            Ok(peer) => {
                let PeerClient {
                    plexer,
                    mut keepalive,
                    mut chainsync,
                    ..
                } = peer;
                let cleanup = SessionCleanup {
                    plexer: Some(plexer),
                    keepalive: Some(keepalive.abort_handle()),
                };
                let mut keepalive_consumed = false;
                let mut reporter_finished = false;
                let result = tokio::select! {
                    result = do_chainsync(&mut chainsync, &mut observer) => result,
                    result = &mut keepalive => {
                        keepalive_consumed = true;
                        keepalive_result(result)
                    },
                    result = &mut reporter => {
                        reporter_finished = true;
                        result
                    },
                };
                let result = stop_peer(cleanup, keepalive, keepalive_consumed, result).await;
                observer.disconnected();
                if reporter_finished {
                    return result;
                }
                result
            }
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            if !retryable(&error) {
                return Err(error);
            }
            warn!(error = %error, "Disconnected; retry in 5 secs");
        }
        tokio::select! {
            _ = tokio::time::sleep(FIVE_SECS) => {},
            result = &mut reporter => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodeclient::blockstore::tests::{header, Fixture};

    #[tokio::test]
    async fn teardown_preserves_keepalive_failures_and_ignores_its_own_cancellation() {
        let panic_task = tokio::spawn(async {
            panic!("injected keepalive task failure");
            #[allow(unreachable_code)]
            Ok::<(), pallas_network::facades::Error>(())
        });
        tokio::time::timeout(Duration::from_secs(3), async {
            while !panic_task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let cleanup = SessionCleanup {
            plexer: None,
            keepalive: Some(panic_task.abort_handle()),
        };
        let transport_error = Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "peer closed",
        )));
        assert!(
            matches!(stop_peer(cleanup, panic_task, false, transport_error).await, Err(Error::Task(error)) if error.is_panic())
        );

        let failed_task = tokio::spawn(async {
            Err(pallas_network::facades::Error::KeepAliveClientLoop(
                keepalive::ClientError::AgencyIsOurs,
            ))
        });
        tokio::time::timeout(Duration::from_secs(3), async {
            while !failed_task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let cleanup = SessionCleanup {
            plexer: None,
            keepalive: Some(failed_task.abort_handle()),
        };
        assert!(matches!(
            stop_peer(cleanup, failed_task, false, Ok(())).await,
            Err(Error::Peer(_))
        ));

        let running_task = tokio::spawn(std::future::pending::<Result<(), pallas_network::facades::Error>>());
        let cleanup = SessionCleanup {
            plexer: None,
            keepalive: Some(running_task.abort_handle()),
        };
        assert!(stop_peer(cleanup, running_task, false, Ok(())).await.is_ok());
    }

    #[test]
    fn transport_failures_retry_but_local_protocol_configuration_errors_do_not() {
        assert!(retryable(&Error::Timeout("peer handshake")));
        assert!(retryable(&Error::ChainSync(chainsync::ClientError::InvalidInbound)));
        assert!(retryable(&Error::HandshakeProtocol(handshake::Error::InvalidInbound)));
        for error in [
            chainsync::ClientError::AgencyIsOurs,
            chainsync::ClientError::AgencyIsTheirs,
            chainsync::ClientError::InvalidOutbound,
            chainsync::ClientError::IntersectionNotFound,
        ] {
            assert!(!retryable(&Error::ChainSync(error)));
        }
        assert!(!retryable(&Error::Handshake("refused".into())));
        assert!(!retryable(&Error::Reporter("invalid configuration".into())));
    }

    #[test]
    fn tip_updates_replace_and_rollback_disconnect_invalidate() {
        let (sender, receiver) = tokio::sync::watch::channel(None);
        let mut observer = ChainObserver::new(ChainSink::Tip(sender), false);
        for number in 1..=10_000 {
            observer
                .on_header(header(number, number, 1, 0), &Tip(Point::Origin, 20_000))
                .unwrap();
        }
        assert_eq!(receiver.borrow().as_ref().unwrap().block_number, 10_000);
        observer.on_rollback(&Point::Origin).unwrap();
        assert!(receiver.borrow().is_none());
        observer.on_header(header(1, 1, 1, 0), &Tip(Point::Origin, 2)).unwrap();
        observer.disconnected();
        assert!(receiver.borrow().is_none());
    }

    #[test]
    fn store_batch_commits_at_1024_without_reaching_tip() {
        let fixture = Fixture::new();
        let store = SqLiteBlockStore::new(&fixture.0.join("chain.db")).unwrap();
        let mut observer = ChainObserver::new(
            ChainSink::Store {
                store: Box::new(store),
                pending: Vec::new(),
                shelley_genesis_hash: "00".repeat(32),
            },
            false,
        );
        observer.last_log_time = Instant::now();
        for number in 1..=1024 {
            observer
                .on_header(
                    header(number, number * 10, 1, if number == 1 { 0 } else { 1 }),
                    &Tip(Point::Origin, 2000),
                )
                .unwrap();
        }
        if let ChainSink::Store { store, pending, .. } = &mut observer.sink {
            assert!(pending.is_empty());
            assert_eq!(store.get_tip_slot_number().unwrap(), 10240);
        } else {
            panic!("expected store sink");
        }
    }

    #[test]
    fn pending_rollback_preserves_durable_prefix_and_errors_preserve_pending() {
        let fixture = Fixture::new();
        let mut store = SqLiteBlockStore::new(&fixture.0.join("chain.db")).unwrap();
        store.save_block(&[header(1, 10, 1, 0)], &"00".repeat(32)).unwrap();
        let mut observer = ChainObserver::new(
            ChainSink::Store {
                store: Box::new(store),
                pending: vec![header(2, 20, 2, 1), header(3, 30, 3, 2)],
                shelley_genesis_hash: "00".repeat(32),
            },
            false,
        );
        observer.on_rollback(&Point::Specific(20, vec![2; 32])).unwrap();
        if let ChainSink::Store { store, pending, .. } = &mut observer.sink {
            assert_eq!(pending.iter().map(|h| h.slot_number).collect::<Vec<_>>(), vec![20]);
            assert_eq!(store.get_tip_slot_number().unwrap(), 10);
        }
        assert!(observer.on_rollback(&Point::Specific(20, vec![99; 32])).is_err());
        if let ChainSink::Store { pending, .. } = &observer.sink {
            assert_eq!(pending[0].hash, vec![2; 32]);
        }
        observer.on_rollback(&Point::Specific(10, vec![1; 32])).unwrap();
        if let ChainSink::Store { store, pending, .. } = &mut observer.sink {
            assert!(pending.is_empty());
            assert_eq!(store.get_tip_slot_number().unwrap(), 10);
        }
    }

    #[test]
    fn exact_bootstrap_points_rollback_to_origin() {
        let expected = [
            (
                4492799,
                "f8084c61b6a238acec985b59310b6ecec49c0ab8352249afd7268da5cff2a457",
            ),
            (
                1598399,
                "7e16781b40ebf8b6da18f7b5e8ade855d6738095ef2f1c58c77e88b6e45997a4",
            ),
            (719, "e5400faf19e712ebc5ff5b4b44cecb2b140d1cca25a011e36a91d89e97f53e2e"),
            (359, "87882b6778a831d0f19f03ee3fb5e95081afa835976abc1b8dd6f7b65421a816"),
        ];
        for (slot, hash) in expected {
            let point = Point::Specific(slot, hex::decode(hash).unwrap());
            assert!(is_bootstrap_point(&point));
            assert!(!is_bootstrap_point(&Point::Specific(slot, vec![99; 32])));
            let fixture = Fixture::new();
            let mut store = SqLiteBlockStore::new(&fixture.0.join("chain.db")).unwrap();
            store.save_block(&[header(1, 10, 1, 0)], &"00".repeat(32)).unwrap();
            let mut observer = ChainObserver::new(
                ChainSink::Store {
                    store: Box::new(store),
                    pending: vec![header(2, 20, 2, 1)],
                    shelley_genesis_hash: "00".repeat(32),
                },
                false,
            );
            observer.on_rollback(&point).unwrap();
            if let ChainSink::Store { store, pending, .. } = &mut observer.sink {
                assert!(pending.is_empty());
                assert_eq!(store.get_tip_slot_number().unwrap(), 0);
            }
        }
    }

    #[tokio::test]
    async fn returned_intersection_rolls_back_before_request_next() {
        for redb in [false, true] {
            let fixture = Fixture::new();
            let path = fixture.0.join("chain.db");
            let mut store: Box<dyn BlockStore + Send> = if redb {
                Box::new(RedbBlockStore::new(&path).unwrap())
            } else {
                Box::new(SqLiteBlockStore::new(&path).unwrap())
            };
            store
                .save_block(
                    &[header(1, 10, 1, 0), header(2, 20, 2, 1), header(3, 100, 3, 2)],
                    &"00".repeat(32),
                )
                .unwrap();
            let mut observer = ChainObserver::new(
                ChainSink::Store {
                    store,
                    pending: Vec::new(),
                    shelley_genesis_hash: "00".repeat(32),
                },
                false,
            );
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let (client_bearer, server_bearer) = tokio::join!(
                Bearer::connect_tcp(listener.local_addr().unwrap()),
                Bearer::accept_tcp(&listener)
            );
            let mut client_plexer = Plexer::new(client_bearer.unwrap());
            let mut client = chainsync::Client::new(client_plexer.subscribe_client(PROTOCOL_N2N_CHAIN_SYNC));
            let client_plexer = client_plexer.spawn();
            let mut server_plexer = Plexer::new(server_bearer.unwrap().0);
            let mut server =
                chainsync::Server::<HeaderContent>::new(server_plexer.subscribe_server(PROTOCOL_N2N_CHAIN_SYNC));
            let server_plexer = server_plexer.spawn();
            let server_future = async {
                assert!(matches!(
                    server.recv_while_idle().await.unwrap(),
                    Some(chainsync::ClientRequest::Intersect(_))
                ));
                server
                    .send_intersect_found(
                        Point::Specific(20, vec![2; 32]),
                        Tip(Point::Specific(100, vec![3; 32]), 3),
                    )
                    .await
                    .unwrap();
                assert!(matches!(
                    server.recv_while_idle().await.unwrap(),
                    Some(chainsync::ClientRequest::RequestNext)
                ));
            };
            {
                let driver = do_chainsync(&mut client, &mut observer);
                tokio::pin!(driver);
                tokio::select! {
                    result = &mut driver => panic!("driver ended before peer stalled: {result:?}"),
                    result = tokio::time::timeout(Duration::from_secs(3), server_future) => result.unwrap(),
                }
            }
            client_plexer.abort().await;
            server_plexer.abort().await;
            drop(observer);
            let mut reopened: Box<dyn BlockStore + Send> = if redb {
                Box::new(RedbBlockStore::new(&path).unwrap())
            } else {
                Box::new(SqLiteBlockStore::new(&path).unwrap())
            };
            assert_eq!(reopened.get_tip_slot_number().unwrap(), 20);
            assert!(
                reopened
                    .find_block_by_hash(&hex::encode([3; 32]))
                    .unwrap()
                    .unwrap()
                    .orphaned
            );
        }
    }
}
