use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use bech32::{Bech32, Hrp};
use minicbor::data::Type;
use pallas_network::facades::NodeClient;
use pallas_network::miniprotocols::localstate::queries_v16::BlockQuery;
use pallas_network::miniprotocols::localstate::{queries_v16, ClientError, State};
use thiserror::Error;
use tracing::debug;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Error in Client")]
    ClientFailure(#[from] ClientError),
    #[error(transparent)]
    Node(#[from] pallas_network::facades::Error),
    #[error(transparent)]
    CborDecode(#[from] minicbor::decode::Error),
    #[error("Unexpected array length: expected {expected}, got {actual}")]
    UnexpectedArrayLength { expected: u64, actual: u64 },
    #[error(transparent)]
    Bech32(#[from] bech32::primitives::hrp::Error),
    #[error(transparent)]
    Bech32Encoding(#[from] bech32::EncodeError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Snapshot error: {0}")]
    Snapshot(String),
}

#[derive(Clone, Copy)]
enum Snapshot {
    Mark,
    Set,
    Go,
}

impl Snapshot {
    fn parse(name: &str) -> Result<Self, Error> {
        match name {
            "mark" => Ok(Self::Mark),
            "set" => Ok(Self::Set),
            "go" => Ok(Self::Go),
            _ => Err(Error::Snapshot(format!("Unknown snapshot name: {}", name))),
        }
    }
}

async fn fetch_snapshot(socket_path: &PathBuf, network_magic: u64) -> Result<Vec<u8>, Error> {
    let mut node = NodeClient::connect(socket_path, network_magic).await?;
    let client = node.statequery();
    let result = async {
        client.acquire(None).await?;
        let era = queries_v16::get_current_era(client).await?;
        debug!("Current era: {}", era);
        let cbor = queries_v16::get_cbor(client, era, BlockQuery::DebugNewEpochState).await?;
        cbor.first()
            .map(|value| value.0.to_vec())
            .ok_or_else(|| Error::Snapshot("Node returned no snapshot CBOR".into()))
    }
    .await;
    // Release only when the protocol permits it; a failed in-flight query must
    // instead be terminated by aborting the owning multiplexer below.
    let cleanup = async {
        if matches!(client.state(), State::Acquired) {
            client.send_release().await?;
        }
        if matches!(client.state(), State::Idle) {
            client.send_done().await?;
        }
        Ok::<(), Error>(())
    }
    .await;
    node.abort().await;
    let bytes = result?;
    cleanup?;
    Ok(bytes)
}

pub(crate) async fn dump(
    socket_path: &PathBuf,
    network_magic: u64,
    name: &str,
    network_id: u8,
    stake_prefix: &str,
    output_file: &str,
) -> Result<(), Error> {
    let snapshot = Snapshot::parse(name)?;
    let hrp = Hrp::parse(stake_prefix)?;
    let bytes = fetch_snapshot(socket_path, network_magic).await?;
    write_stake_snapshot(Path::new(output_file), &bytes, snapshot, network_id, hrp)
}

pub(crate) async fn pool_stake_dump(
    socket_path: &PathBuf,
    network_magic: u64,
    name: &str,
    network_id: u8,
    output_file: &str,
) -> Result<(), Error> {
    let snapshot = Snapshot::parse(name)?;
    let bytes = fetch_snapshot(socket_path, network_magic).await?;
    write_pool_snapshot(Path::new(output_file), &bytes, snapshot, network_id)
}

fn expect_array(decoder: &mut minicbor::Decoder<'_>, expected: u64) -> Result<(), Error> {
    let actual = decoder.array()?.unwrap_or(0);
    if actual != expected {
        return Err(Error::UnexpectedArrayLength { expected, actual });
    }
    Ok(())
}

fn snapshot_decoder(bytes: &[u8], snapshot: Snapshot) -> Result<minicbor::Decoder<'_>, Error> {
    // Check the complete envelope, not just the selected prefix, before any file work.
    let mut complete = minicbor::Decoder::new(bytes);
    complete.skip()?;
    if complete.position() != bytes.len() {
        return Err(Error::Snapshot("Trailing snapshot CBOR data".into()));
    }
    let mut decoder = minicbor::Decoder::new(bytes);
    expect_array(&mut decoder, 7)?;
    for _ in 0..3 {
        decoder.skip()?;
    }
    expect_array(&mut decoder, 4)?;
    decoder.skip()?;
    decoder.skip()?;
    expect_array(&mut decoder, 4)?;
    let index = match snapshot {
        Snapshot::Mark => 0,
        Snapshot::Set => 1,
        Snapshot::Go => 2,
    };
    for _ in 0..index {
        decoder.skip()?;
    }
    expect_array(&mut decoder, 3)?;
    Ok(decoder)
}

fn stake_key(decoder: &mut minicbor::Decoder<'_>, network_id: u8) -> Result<Vec<u8>, Error> {
    expect_array(decoder, 2)?;
    let prefix = match decoder.u8()? {
        0 => 0xe0,
        1 => 0xf0,
        value => return Err(Error::Snapshot(format!("Unknown address type: {}", value))),
    } | network_id;
    let bytes = decoder.bytes()?;
    let mut key = Vec::with_capacity(bytes.len() + 1);
    key.push(prefix);
    key.extend_from_slice(bytes);
    Ok(key)
}

fn map_entry(decoder: &mut minicbor::Decoder<'_>, remaining: &mut Option<u64>) -> Result<bool, Error> {
    if let Some(count) = remaining {
        if *count == 0 {
            return Ok(false);
        }
        *count -= 1;
    } else if decoder.datatype()? == Type::Break {
        decoder.skip()?;
        return Ok(false);
    }
    Ok(true)
}

fn stake_amounts(decoder: &mut minicbor::Decoder<'_>, network_id: u8) -> Result<Vec<(Vec<u8>, u64)>, Error> {
    let mut remaining = decoder.map()?;
    let mut amounts = Vec::new();
    while map_entry(decoder, &mut remaining)? {
        let key = stake_key(decoder, network_id)?;
        amounts.push((key, decoder.u64()?));
    }
    Ok(amounts)
}

fn write_stake_snapshot(
    output: &Path,
    bytes: &[u8],
    snapshot: Snapshot,
    network_id: u8,
    hrp: Hrp,
) -> Result<(), Error> {
    let mut decoder = snapshot_decoder(bytes, snapshot)?;
    let amounts = stake_amounts(&mut decoder, network_id)?;
    write_snapshot_atomic(output, |out| {
        for (key, amount) in amounts {
            let address = bech32::encode::<Bech32>(hrp, &key)?;
            writeln!(out, "{},{},", address, amount)?;
        }
        Ok(())
    })
}

fn write_pool_snapshot(output: &Path, bytes: &[u8], snapshot: Snapshot, network_id: u8) -> Result<(), Error> {
    let mut decoder = snapshot_decoder(bytes, snapshot)?;
    let amounts: HashMap<_, _> = stake_amounts(&mut decoder, network_id)?.into_iter().collect();
    let mut remaining = decoder.map()?;
    let mut pools: HashMap<Vec<u8>, u64> = HashMap::new();
    let mut total = 0u64;
    while map_entry(&mut decoder, &mut remaining)? {
        let key = stake_key(&mut decoder, network_id)?;
        let pool = decoder.bytes()?;
        let amount = amounts
            .get(&key)
            .ok_or_else(|| Error::Snapshot("Missing stake amount for delegated address".into()))?;
        let pool_total = pools.entry(pool.to_vec()).or_default();
        *pool_total = pool_total
            .checked_add(*amount)
            .ok_or_else(|| Error::Snapshot("Pool stake overflow".into()))?;
        total = total
            .checked_add(*amount)
            .ok_or_else(|| Error::Snapshot("Total stake overflow".into()))?;
    }
    let mut pools: Vec<_> = pools.into_iter().collect();
    pools.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    write_snapshot_atomic(output, |out| {
        for (pool, amount) in pools {
            writeln!(out, "{},{},{},", hex::encode(pool), amount, total)?;
        }
        Ok(())
    })
}

fn write_snapshot_atomic(
    output: &Path,
    write: impl FnOnce(&mut BufWriter<File>) -> Result<(), Error>,
) -> Result<(), Error> {
    let filename = output
        .file_name()
        .ok_or_else(|| Error::Snapshot("Output path has no filename".into()))?;
    let mut temporary_name = std::ffi::OsString::from(".");
    temporary_name.push(filename);
    temporary_name.push(format!(".{}.tmp", uuid::Uuid::now_v7()));
    let temporary = output.with_file_name(temporary_name);
    let file = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
    let result = (|| {
        let mut out = BufWriter::new(file);
        write(&mut out)?;
        out.flush()?;
        out.get_ref().sync_all()?;
        drop(out);
        std::fs::rename(&temporary, output)?;
        Ok(())
    })();
    if result.is_err() {
        std::fs::remove_file(&temporary)?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(missing_amount: bool) -> Vec<u8> {
        let mut encoder = minicbor::Encoder::new(Vec::new());
        encoder.array(7).unwrap().u8(0).unwrap().u8(0).unwrap().u8(0).unwrap();
        encoder
            .array(4)
            .unwrap()
            .u8(0)
            .unwrap()
            .u8(0)
            .unwrap()
            .array(4)
            .unwrap();
        for _ in 0..3 {
            encoder.array(3).unwrap().begin_map().unwrap();
            if !missing_amount {
                encoder
                    .array(2)
                    .unwrap()
                    .u8(0)
                    .unwrap()
                    .bytes(&[1; 28])
                    .unwrap()
                    .u64(42)
                    .unwrap();
            }
            encoder.end().unwrap().begin_map().unwrap();
            encoder
                .array(2)
                .unwrap()
                .u8(0)
                .unwrap()
                .bytes(&[1; 28])
                .unwrap()
                .bytes(&[2; 28])
                .unwrap();
            encoder.end().unwrap().map(0).unwrap();
        }
        encoder
            .u8(0)
            .unwrap()
            .u8(0)
            .unwrap()
            .u8(0)
            .unwrap()
            .u8(0)
            .unwrap()
            .u8(0)
            .unwrap();
        encoder.into_writer()
    }

    #[test]
    fn snapshot_parsing_and_atomic_output() {
        let directory = std::env::temp_dir().join(format!("cncli-snapshot-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&directory).unwrap();
        let output = directory.join("snapshot.csv");
        let previous = b"previous-good-output";
        let valid = fixture(false);
        let hrp = Hrp::parse("stake_test").unwrap();
        for bytes in [&[][..], &[0xff][..], &valid[..valid.len() - 1]] {
            std::fs::write(&output, previous).unwrap();
            assert!(write_stake_snapshot(&output, bytes, Snapshot::Mark, 0, hrp).is_err());
            assert_eq!(std::fs::read(&output).unwrap(), previous);
            assert!(write_pool_snapshot(&output, bytes, Snapshot::Mark, 0).is_err());
            assert_eq!(std::fs::read(&output).unwrap(), previous);
        }
        assert!(write_pool_snapshot(&output, &fixture(true), Snapshot::Mark, 0).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), previous);
        assert!(write_snapshot_atomic(&output, |out| {
            out.write_all(b"partial")?;
            Err(Error::Io(std::io::Error::other("injected writer failure")))
        })
        .is_err());
        assert_eq!(std::fs::read(&output).unwrap(), previous);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        for snapshot in [Snapshot::Mark, Snapshot::Set, Snapshot::Go] {
            write_pool_snapshot(&output, &valid, snapshot, 0).unwrap();
            assert_eq!(
                std::fs::read_to_string(&output).unwrap(),
                format!("{},42,42,\n", "02".repeat(28))
            );
            write_stake_snapshot(&output, &valid, snapshot, 0, hrp).unwrap();
            assert_eq!(
                std::fs::read_to_string(&output).unwrap(),
                "stake_test1uqqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgach69p,42,\n"
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn invalid_name_precedes_connection() {
        let socket = PathBuf::from("/nonexistent/cncli-snapshot-test.socket");
        assert!(matches!(
            dump(&socket, 0, "invalid", 0, "stake", "unused").await,
            Err(Error::Snapshot(_))
        ));
        assert!(matches!(
            pool_stake_dump(&socket, 0, "invalid", 0, "unused").await,
            Err(Error::Snapshot(_))
        ));
    }
}
