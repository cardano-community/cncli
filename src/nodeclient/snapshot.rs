use std::io::Write;
use std::path::PathBuf;

use bech32::{Bech32, Hrp};
use log::debug;
use minicbor::data::Type;
use pallas_network::facades::NodeClient;
use pallas_network::miniprotocols::localstate::queries_v16::BlockQuery;
use pallas_network::miniprotocols::localstate::{queries_v16, ClientError};
use thiserror::Error;

use crate::nodeclient::snapshot::Error::UnexpectedCborType;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Error in Client")]
    ClientFailure(#[from] ClientError),

    #[error(transparent)]
    CborDecode(#[from] minicbor::decode::Error),

    #[error("Unexpected array length: expected {expected}, got {actual}")]
    UnexpectedArrayLength { expected: u64, actual: u64 },

    #[error("Unexpected map length: expected {expected}, got {actual}")]
    UnexpectedMapLength { expected: u64, actual: u64 },

    #[error("Unexpected Cbor Type: {value:?}")]
    UnexpectedCborType { value: Type },

    #[error(transparent)]
    Bech32Error(#[from] bech32::primitives::hrp::Error),

    #[error(transparent)]
    Bech32EncodingError(#[from] bech32::EncodeError),

    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error("Snapshot error: {0}")]
    SnapshotError(String),
}

#[derive(Debug)]
enum Snapshot {
    Mark,
    Set,
    Go,
}

pub(crate) async fn dump(
    socket_path: &PathBuf,
    network_magic: u64,
    name: &str,
    network_id: u8,
    stake_prefix: &str,
    output_file: &str,
) -> Result<(), Error> {
    let mut client = NodeClient::connect(socket_path, network_magic).await.unwrap();

    // convert name into a Snapshot enum
    let snapshot = match name {
        "mark" => Snapshot::Mark,
        "set" => Snapshot::Set,
        "go" => Snapshot::Go,
        _ => return Err(Error::SnapshotError(format!("Unknown snapshot name: {}", name))),
    };

    let client = client.statequery();

    client.acquire(None).await?;

    let era = queries_v16::get_current_era(client).await?;
    debug!("Current era: {}", era);

    let cbor = queries_v16::get_cbor(client, era, BlockQuery::DebugNewEpochState).await?;
    client.send_release().await?;

    // Save the CBOR to a file
    let cbor_bytes = &cbor[0].0;
    // std::fs::write(output_file, cbor_bytes.as_slice()).unwrap();

    let mut decoder = minicbor::Decoder::new(cbor_bytes);
    // top level is an array
    let stake_array_len = decoder
        .array()?
        .ok_or(Error::UnexpectedArrayLength { expected: 7, actual: 0 })?;
    if stake_array_len != 7 {
        return Err(Error::UnexpectedArrayLength {
            expected: 7,
            actual: stake_array_len,
        });
    }
    decoder.skip()?; // skip the 0th element
    decoder.skip()?; // skip the 1st element
    decoder.skip()?; // skip the 2nd element
                     // array element [3]
    let snapshots_array_len = decoder
        .array()?
        .ok_or(Error::UnexpectedArrayLength { expected: 4, actual: 0 })?;
    if snapshots_array_len != 4 {
        return Err(Error::UnexpectedArrayLength {
            expected: 4,
            actual: snapshots_array_len,
        });
    }
    decoder.skip()?; // skip the 0th element
    decoder.skip()?; // skip the 1st element
                     // array element [3][2]
    let inner_array_len = decoder
        .array()?
        .ok_or(Error::UnexpectedArrayLength { expected: 4, actual: 0 })?;
    if inner_array_len != 4 {
        return Err(Error::UnexpectedArrayLength {
            expected: 4,
            actual: inner_array_len,
        });
    }

    match snapshot {
        Snapshot::Mark => {
            // mark snapshot is at index 0 so no skips needed
        }
        Snapshot::Set => {
            // set snapshot is at index 1 so skip the mark snapshot
            decoder.skip()?;
        }
        Snapshot::Go => {
            // go snapshot is at index 2 so skip the mark and set snapshots
            decoder.skip()?;
            decoder.skip()?;
        }
    }

    // array element [3][2][snapshot]
    let snapshot_array_len = decoder
        .array()?
        .ok_or(Error::UnexpectedArrayLength { expected: 3, actual: 0 })?;
    if snapshot_array_len != 3 {
        return Err(Error::UnexpectedArrayLength {
            expected: 3,
            actual: snapshot_array_len,
        });
    }

    let output_file = std::fs::File::create(output_file)?;
    let mut output_file = std::io::BufWriter::new(output_file);

    let hrp = Hrp::parse(stake_prefix)?;

    // loop through each map item
    // array element [3][2][snapshot][0] is an indeterminate-length map
    decoder.map()?;
    loop {
        let datatype = decoder.datatype()?;
        match datatype {
            Type::Array => {
                decoder.array()?;
                let address_type = decoder.u8()?; // the type of stake address
                let stake_key_prefix = [match address_type {
                    0 => 0xe0u8, // key-based stake address
                    1 => 0xf0u8, // script-based stake address
                    _ => return Err(Error::SnapshotError(format!("Unknown address type: {}", address_type))),
                } | network_id];
                let stake_key_bytes = decoder.bytes()?;
                let stake_key_bytes = [&stake_key_prefix, stake_key_bytes].concat();
                let stake_address = encode_bech32(&stake_key_bytes, hrp)?;
                let lovelace = decoder.u64()?;
                writeln!(output_file, "{},{},", stake_address, lovelace)?;
            }
            Type::Break => {
                break;
            }
            _ => {
                return Err(UnexpectedCborType { value: datatype });
            }
        }
    }

    output_file.flush()?;

    Ok(())
}

fn encode_bech32(addr: &[u8], hrp: Hrp) -> Result<String, Error> {
    let encoded = bech32::encode::<Bech32>(hrp, addr)?;
    Ok(encoded)
}
