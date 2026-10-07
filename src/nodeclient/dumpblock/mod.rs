use hex::ToHex;
use pallas_network::facades::{KeepAliveLoop, PeerClient, DEFAULT_KEEP_ALIVE_INTERVAL_SEC};
use pallas_network::miniprotocols::chainsync::{HeaderContent, NextResponse};
use pallas_network::miniprotocols::{
    blockfetch, chainsync, handshake, keepalive, peersharing, txsubmission, Point, PROTOCOL_N2N_BLOCK_FETCH,
    PROTOCOL_N2N_CHAIN_SYNC, PROTOCOL_N2N_HANDSHAKE, PROTOCOL_N2N_KEEP_ALIVE, PROTOCOL_N2N_PEER_SHARING,
    PROTOCOL_N2N_TX_SUBMISSION,
};
use pallas_network::multiplexer::{Bearer, Plexer};
use std::io;
use std::net::ToSocketAddrs;
use std::time::Duration;
use structopt::StructOpt;
use thiserror::Error;
use tracing::debug;

use crate::nodeclient::dumpblock::hash_utils::{extract_slot, header_hash};

mod hash_utils;

#[derive(Error, Debug)]
pub enum DumpBlockError {
    #[error("Invalid hex: {0}")]
    InvalidHex(#[from] hex::FromHexError),

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Output error: {0}")]
    Output(#[from] crate::OutputError),

    #[error("Peer error: {0}")]
    Peer(#[from] pallas_network::facades::Error),

    #[error("Task error: {0}")]
    Task(#[from] tokio::task::JoinError),

    #[error("Unable to resolve host")]
    UnableToResolveHost,

    #[error("Handshake error: {0}")]
    Handshake(String),

    #[error("Handshake rejected: {0:?}")]
    HandshakeRejected(String),

    #[error("Chain sync error: {0}")]
    ChainSync(#[from] chainsync::ClientError),

    #[error("Block fetch error: {0}")]
    BlockFetch(#[from] blockfetch::ClientError),

    #[error("Pallas traverse error: {0}")]
    PallasTraverse(#[from] pallas_traverse::Error),

    #[error("Intersection not found")]
    IntersectionNotFound,

    #[error("Unexpected await from chainsync")]
    UnexpectedAwait,
}

#[derive(Debug, StructOpt)]
pub struct Args {
    #[structopt(long, help = "Slot number of the intersect point")]
    pub intersect_slot: u64,
    #[structopt(long, help = "Block hash of the intersect point (hex)")]
    pub intersect_hash: String,
    #[structopt(short, long, help = "cardano-node hostname to connect to")]
    pub host: String,
    #[structopt(short, long, default_value = "3001", help = "cardano-node port")]
    pub port: u16,
    #[structopt(long, default_value = "764824073", help = "network magic.")]
    pub network_magic: u64,
}

pub async fn run(args: Args) -> Result<(), DumpBlockError> {
    let point = Point::Specific(args.intersect_slot, hex::decode(&args.intersect_hash)?);

    let addr = format!("{}:{}", args.host, args.port)
        .to_socket_addrs()?
        .next()
        .ok_or(DumpBlockError::UnableToResolveHost)?;

    let bearer = Bearer::connect_tcp(addr).await?;
    let mut plexer = Plexer::new(bearer);

    let hs_channel = plexer.subscribe_client(PROTOCOL_N2N_HANDSHAKE);
    let cs_channel = plexer.subscribe_client(PROTOCOL_N2N_CHAIN_SYNC);
    let bf_channel = plexer.subscribe_client(PROTOCOL_N2N_BLOCK_FETCH);
    let ka_channel = plexer.subscribe_client(PROTOCOL_N2N_KEEP_ALIVE);
    let txsub_channel = plexer.subscribe_client(PROTOCOL_N2N_TX_SUBMISSION);
    let peersharing_channel = plexer.subscribe_client(PROTOCOL_N2N_PEER_SHARING);

    let keepalive = keepalive::Client::new(ka_channel);

    let plexer = plexer.spawn();

    let mut handshake = handshake::Client::new(hs_channel);
    let versions = handshake::n2n::VersionTable::v7_and_above(args.network_magic);
    let confirmation = match handshake.handshake(versions).await {
        Ok(confirmation) => confirmation,
        Err(error) => {
            plexer.abort().await;
            return Err(DumpBlockError::Handshake(format!("{error:?}")));
        }
    };

    let block_hex = match confirmation {
        handshake::Confirmation::Accepted(_, _) => {
            let keepalive =
                KeepAliveLoop::client(keepalive, Duration::from_secs(DEFAULT_KEEP_ALIVE_INTERVAL_SEC)).spawn();
            let peer = PeerClient {
                plexer,
                keepalive,
                chainsync: chainsync::Client::new(cs_channel),
                blockfetch: blockfetch::Client::new(bf_channel),
                txsubmission: txsubmission::Client::new(txsub_channel),
                peersharing: peersharing::Client::new(peersharing_channel),
            };
            let PeerClient {
                mut chainsync,
                mut blockfetch,
                plexer,
                keepalive,
                txsubmission: _txsubmission,
                peersharing: _peersharing,
            } = peer;
            let res = fetch_block(&mut chainsync, &mut blockfetch, point).await;
            keepalive.abort();
            let stopped = keepalive.await;
            plexer.abort().await;
            match stopped {
                Err(error) if error.is_cancelled() => {}
                Err(error) => return Err(error.into()),
                Ok(Err(error)) => return Err(error.into()),
                Ok(Ok(())) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "keepalive ended").into()),
            }
            res
        }
        handshake::Confirmation::Rejected(reason) => {
            plexer.abort().await;
            return Err(DumpBlockError::HandshakeRejected(format!("{reason:?}")));
        }
        handshake::Confirmation::QueryReply(_) => {
            plexer.abort().await;
            return Err(DumpBlockError::HandshakeRejected("Unexpected QueryReply".to_string()));
        }
    }?;

    crate::write_line(&mut io::stdout().lock(), &block_hex)?;
    Ok(())
}

async fn fetch_block(
    chainsync: &mut chainsync::Client<HeaderContent>,
    blockfetch: &mut blockfetch::Client,
    point: Point,
) -> Result<String, DumpBlockError> {
    let (intersection, _) = chainsync.find_intersect(vec![point.clone()]).await?;

    let intersect_point = intersection.ok_or(DumpBlockError::IntersectionNotFound)?;
    debug!(?intersect_point, "intersected");

    let block_point = loop {
        match chainsync.request_next().await? {
            NextResponse::RollForward(header, _) => break point_from_header(&header)?,
            NextResponse::RollBackward(_, _) => {
                debug!("received RollBackward, requesting next block");
                continue;
            }
            NextResponse::Await => return Err(DumpBlockError::UnexpectedAwait),
        }
    };

    let body = blockfetch.fetch_single(block_point).await?;

    Ok(body.encode_hex::<String>())
}

fn point_from_header(header: &HeaderContent) -> Result<Point, DumpBlockError> {
    let slot = extract_slot(header)?;
    let hash = header_hash(header)?;
    Ok(Point::Specific(slot, hash))
}
