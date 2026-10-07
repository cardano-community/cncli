use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::nodeclient::sync::BlockHeader;
use chrono::{SecondsFormat, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{error, info};

pub(crate) const POOLTOOL_BASE_URL: &str = "https://api.pooltool.io";

pub(crate) fn get_pooltool_config(config: &Path) -> Result<PooltoolConfig, Box<dyn std::error::Error + Send + Sync>> {
    let config: PooltoolConfig = serde_json::from_reader(BufReader::new(File::open(config)?))?;
    if config.api_key.trim().is_empty()
        || config.pools.is_empty()
        || config.pools.iter().any(|pool| pool.pool_id.trim().is_empty())
    {
        return Err(super::Error::Reporter(
            "Invalid PoolTool configuration: require API key and nonempty pools with pool IDs".into(),
        )
        .into());
    }
    Ok(config)
}

#[derive(Debug, Deserialize)]
pub(crate) struct PooltoolConfig {
    pub(crate) api_key: String,
    pub(crate) pools: Vec<Pool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Pool {
    pub(crate) name: String,
    pub(crate) pool_id: String,
    pub(crate) host: String,
    pub(crate) port: u16,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PooltoolStats0 {
    api_key: String,
    pool_id: String,
    data: PooltoolData0,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PooltoolData0 {
    node_id: String,
    version: String,
    at: String,
    block_no: u64,
    slot_no: u64,
    block_hash: String,
    parent_hash: String,
    leader_vrf: String,
    leader_vrf_proof: String,
    node_v_key: String,
    protocol_major_version: u64,
    protocol_minor_version: u64,
    platform: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PooltoolStats1 {
    api_key: String,
    pool_id: String,
    data: PooltoolData1,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PooltoolData1 {
    node_id: String,
    version: String,
    at: String,
    block_no: u64,
    slot_no: u64,
    block_hash: String,
    parent_hash: String,
    leader_vrf: String,
    block_vrf: String,
    block_vrf_proof: String,
    node_v_key: String,
    protocol_major_version: u64,
    protocol_minor_version: u64,
    platform: String,
}

pub(crate) struct PoolToolNotifier {
    pool_id: String,
    api_key: String,
    cardano_node_path: PathBuf,
    last_node_version_time: Instant,
    node_version: String,
    client: reqwest::Client,
    base_url: String,
}

impl PoolToolNotifier {
    pub(crate) fn new(
        _pool_name: String,
        pool_id: String,
        api_key: String,
        cardano_node_path: PathBuf,
        client: reqwest::Client,
        base_url: String,
    ) -> Result<Self, super::Error> {
        if pool_id.trim().is_empty() || api_key.trim().is_empty() {
            return Err(super::Error::Reporter(
                "PoolTool requires nonblank pool ID and API key".into(),
            ));
        }
        Ok(Self {
            pool_id,
            api_key,
            cardano_node_path,
            client,
            base_url,
            last_node_version_time: Instant::now(),
            node_version: String::new(),
        })
    }

    async fn read_node_version(&self) -> Result<String, super::Error> {
        let output = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::process::Command::new(&self.cardano_node_path)
                .arg("--version")
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| super::Error::Reporter("cardano-node version deadline exceeded".into()))?
        .map_err(|_| super::Error::Reporter("Cannot execute cardano-node version command".into()))?;
        if !output.status.success() {
            return Err(super::Error::Reporter("cardano-node version command failed".into()));
        }
        let version = String::from_utf8(output.stdout)
            .map_err(|_| super::Error::Reporter("Invalid cardano-node version output".into()))?;
        static VERSION_REGEX: std::sync::LazyLock<Result<Regex, regex::Error>> =
            std::sync::LazyLock::new(|| Regex::new(r"cardano-node (\d+\.\d+\.\d+) .*\r?\ngit rev ([a-f\d]{5}).*"));
        let regex = VERSION_REGEX
            .as_ref()
            .map_err(|_| super::Error::Reporter("Invalid node version matcher".into()))?;
        let captures = regex
            .captures(&version)
            .ok_or_else(|| super::Error::Reporter("Unrecognized cardano-node version output".into()))?;
        let release = captures
            .get(1)
            .ok_or_else(|| super::Error::Reporter("Missing node release".into()))?;
        let revision = captures
            .get(2)
            .ok_or_else(|| super::Error::Reporter("Missing node revision".into()))?;
        Ok(format!("{}:{}", release.as_str(), revision.as_str()))
    }

    fn request_for(&self, header: &BlockHeader) -> Result<reqwest::RequestBuilder, super::Error> {
        let (endpoint, body) = if header.block_vrf_0.is_empty() {
            (
                "/v0/sendstats",
                serde_json::to_string(&PooltoolStats0 {
                    api_key: self.api_key.clone(),
                    pool_id: self.pool_id.clone(),
                    data: PooltoolData0 {
                        node_id: String::new(),
                        version: self.node_version.clone(),
                        at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                        block_no: header.block_number,
                        slot_no: header.slot_number,
                        block_hash: hex::encode(&header.hash),
                        parent_hash: hex::encode(&header.prev_hash),
                        leader_vrf: hex::encode(&header.leader_vrf_0),
                        leader_vrf_proof: hex::encode(&header.leader_vrf_1),
                        node_v_key: hex::encode(&header.node_vkey),
                        protocol_major_version: header.protocol_major_version,
                        protocol_minor_version: header.protocol_minor_version,
                        platform: "cncli".into(),
                    },
                }),
            )
        } else {
            (
                "/v1/sendstats",
                serde_json::to_string(&PooltoolStats1 {
                    api_key: self.api_key.clone(),
                    pool_id: self.pool_id.clone(),
                    data: PooltoolData1 {
                        node_id: String::new(),
                        version: self.node_version.clone(),
                        at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                        block_no: header.block_number,
                        slot_no: header.slot_number,
                        block_hash: hex::encode(&header.hash),
                        parent_hash: hex::encode(&header.prev_hash),
                        leader_vrf: hex::encode(&header.leader_vrf_0),
                        block_vrf: hex::encode(&header.block_vrf_0),
                        block_vrf_proof: hex::encode(&header.block_vrf_1),
                        node_v_key: hex::encode(&header.node_vkey),
                        protocol_major_version: header.protocol_major_version,
                        protocol_minor_version: header.protocol_minor_version,
                        platform: "cncli".into(),
                    },
                }),
            )
        };
        let body = body.map_err(|_| super::Error::Reporter("Cannot serialize PoolTool request".into()))?;
        Ok(self.client.post(format!("{}{endpoint}", self.base_url)).body(body))
    }

    pub(crate) async fn run(
        &mut self,
        mut tips: tokio::sync::watch::Receiver<Option<BlockHeader>>,
    ) -> Result<(), super::Error> {
        self.node_version = self.read_node_version().await?;
        self.last_node_version_time = Instant::now();
        loop {
            if tips.changed().await.is_err() {
                return Ok(());
            }
            if self.last_node_version_time.elapsed() >= Duration::from_secs(3600) {
                self.last_node_version_time = Instant::now();
                match self.read_node_version().await {
                    Ok(version) => self.node_version = version,
                    Err(_) => error!("Cannot refresh cardano-node version; retaining verified version"),
                }
            }
            // Serialize while borrowing, then release the watch guard before HTTP awaits.
            let request = {
                let latest = tips.borrow_and_update();
                latest
                    .as_ref()
                    .map(|header| self.request_for(header).map(|request| (header.block_number, request)))
                    .transpose()?
            };
            let Some((block_number, request)) = request else {
                continue;
            };
            let started = Instant::now();
            let response = request.send().await;
            let http_status = response.as_ref().map(|r| r.status().as_u16()).unwrap_or(0);
            let result = response.and_then(|r| r.error_for_status());
            let duration_ms = started.elapsed().as_millis() as u64;
            if result.is_ok() {
                info!(operation = "pooltool.sendstats", pool_id = %self.pool_id,
                    block_number, http_status, duration_ms, outcome = "ok",
                    "PoolTool request completed");
            } else {
                error!(operation = "pooltool.sendstats", pool_id = %self.pool_id,
                    block_number, http_status, duration_ms, outcome = "error",
                    "PoolTool request failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use tracing::instrument::WithSubscriber;

    const SECRET: &str = "CNCLI_TEST_SECRET_DO_NOT_LOG";

    struct NodeFixture(PathBuf);
    impl NodeFixture {
        fn new(mode: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("cncli-node-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir(&dir).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let body = match mode {
                    "ok" => "printf 'cardano-node 10.1.0 linux\\ngit rev abcdef012345\\n'",
                    "bad" => "printf 'unrecognized\\n'",
                    "exit" => "exit 1",
                    "timeout" => "exec sleep 10",
                    _ => unreachable!(),
                };
                let path = dir.join("cardano-node");
                std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
                Self(path)
            }
            #[cfg(windows)]
            {
                let body = match mode {
                    "ok" => "echo cardano-node 10.1.0 windows\r\necho git rev abcdef012345",
                    "bad" => "echo unrecognized",
                    "exit" => "exit /b 1",
                    "timeout" => "ping -n 11 127.0.0.1 >nul",
                    _ => unreachable!(),
                };
                let path = dir.join("cardano-node.cmd");
                std::fs::write(&path, format!("@echo off\r\n{body}\r\n")).unwrap();
                Self(path)
            }
        }
    }
    impl Drop for NodeFixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
        }
    }
    fn notifier(node: &NodeFixture, base_url: String) -> PoolToolNotifier {
        PoolToolNotifier::new(
            "test".into(),
            "短".into(),
            SECRET.into(),
            node.0.clone(),
            reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            base_url,
        )
        .unwrap()
    }
    fn header(number: u64, v1: bool) -> BlockHeader {
        BlockHeader {
            block_number: number,
            slot_number: 100,
            hash: vec![1; 32],
            prev_hash: vec![2; 32],
            node_vkey: vec![3; 32],
            node_vrf_vkey: vec![],
            block_vrf_0: if v1 { vec![4; 32] } else { vec![] },
            block_vrf_1: vec![5; 32],
            eta_vrf_0: vec![],
            eta_vrf_1: vec![],
            leader_vrf_0: vec![6; 32],
            leader_vrf_1: vec![7; 32],
            block_size: 0,
            block_body_hash: vec![],
            pool_opcert: vec![],
            unknown_0: 0,
            unknown_1: 0,
            unknown_2: vec![],
            protocol_major_version: 8,
            protocol_minor_version: 0,
        }
    }
    fn receive(listener: &TcpListener) -> (TcpStream, String, serde_json::Value) {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut received = Vec::new();
        let mut chunk = [0; 1024];
        let end = loop {
            let count = stream.read(&mut chunk).unwrap();
            assert_ne!(count, 0);
            received.extend_from_slice(&chunk[..count]);
            if let Some(end) = received.windows(4).position(|v| v == b"\r\n\r\n") {
                break end + 4;
            }
        };
        let headers = String::from_utf8(received[..end].to_vec()).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .unwrap();
        while received.len() < end + length {
            let count = stream.read(&mut chunk).unwrap();
            assert_ne!(count, 0);
            received.extend_from_slice(&chunk[..count]);
        }
        let payload = serde_json::from_slice(&received[end..end + length]).unwrap();
        (stream, headers, payload)
    }
    fn respond(mut stream: TcpStream, status: u16) {
        write!(
            stream,
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{SECRET}",
            SECRET.len()
        )
        .unwrap();
    }
    #[derive(Clone)]
    struct LogWriter(Arc<Mutex<Vec<u8>>>);
    impl Write for LogWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("log capture lock poisoned"))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn worker_payloads_http_recovery_and_safe_logs() {
        for v1 in [false, true] {
            let node = NodeFixture::new("ok");
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let mut notifier = notifier(&node, format!("http://{}", listener.local_addr().unwrap()));
            let (tx, rx) = tokio::sync::watch::channel(None);
            let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel();
            let server = std::thread::spawn(move || {
                for status in [429, 500, 200] {
                    let (stream, headers, payload) = receive(&listener);
                    respond(stream, status);
                    seen_tx.send((headers, payload)).unwrap();
                }
            });
            let logs = Arc::new(Mutex::new(Vec::new()));
            let writer = LogWriter(logs.clone());
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_ansi(false)
                .without_time()
                .with_writer(move || writer.clone())
                .finish();
            let worker = tokio::spawn(async move { notifier.run(rx).await }.with_subscriber(subscriber));
            for number in 1..=3 {
                tx.send_replace(Some(header(number, v1)));
                let (headers, payload) = tokio::time::timeout(Duration::from_secs(10), seen_rx.recv())
                    .await
                    .unwrap()
                    .unwrap();
                assert!(headers.starts_with(if v1 {
                    "POST /v1/sendstats "
                } else {
                    "POST /v0/sendstats "
                }));
                assert_eq!(payload["apiKey"], SECRET);
                assert_eq!(payload["poolId"], "短");
                let data = &payload["data"];
                assert_eq!(data["version"], "10.1.0:abcde");
                assert_eq!(data["blockNo"], number);
                assert_eq!(data["slotNo"], 100);
                assert_eq!(data["nodeId"], "");
                assert_eq!(data["blockHash"], hex::encode([1; 32]));
                assert_eq!(data["parentHash"], hex::encode([2; 32]));
                assert_eq!(data["nodeVKey"], hex::encode([3; 32]));
                assert_eq!(data["leaderVrf"], hex::encode([6; 32]));
                assert_eq!(data["protocolMajorVersion"], 8);
                assert_eq!(data["protocolMinorVersion"], 0);
                assert_eq!(data["platform"], "cncli");
                assert!(chrono::DateTime::parse_from_rfc3339(data["at"].as_str().unwrap()).is_ok());
                if v1 {
                    assert_eq!(data["blockVrf"], hex::encode([4; 32]));
                    assert_eq!(data["blockVrfProof"], hex::encode([5; 32]));
                    assert!(data.get("leaderVrfProof").is_none());
                } else {
                    assert_eq!(data["leaderVrfProof"], hex::encode([7; 32]));
                    assert!(data.get("blockVrf").is_none());
                }
            }
            drop(tx);
            tokio::time::timeout(Duration::from_secs(10), worker)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            server.join().unwrap();
            let logs = String::from_utf8(
                logs.lock()
                    .map_or_else(|poison| poison.into_inner().clone(), |logs| logs.clone()),
            )
            .unwrap();
            for forbidden in [SECRET, "apiKey", "leaderVrf", "blockHash"] {
                assert!(!logs.contains(forbidden));
            }
            for field in [
                "operation=\"pooltool.sendstats\"",
                "pool_id=短",
                "block_number=",
                "http_status=429",
                "http_status=500",
                "http_status=200",
                "duration_ms=",
                "outcome=\"ok\"",
                "outcome=\"error\"",
            ] {
                assert!(logs.contains(field), "missing {field}: {logs}");
            }
        }
    }

    #[tokio::test]
    async fn stalled_request_coalesces_and_none_invalidates_pending() {
        for invalidate in [false, true] {
            let node = NodeFixture::new("ok");
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let mut notifier = notifier(&node, format!("http://{}", listener.local_addr().unwrap()));
            let (tx, rx) = tokio::sync::watch::channel(None);
            let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let server = std::thread::spawn(move || {
                let (stream, _, payload) = receive(&listener);
                seen_tx.send(payload["data"]["blockNo"].as_u64().unwrap()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                respond(stream, 200);
                let (stream, _, payload) = receive(&listener);
                seen_tx.send(payload["data"]["blockNo"].as_u64().unwrap()).unwrap();
                respond(stream, 200);
            });
            let worker = tokio::spawn(async move { notifier.run(rx).await });
            tx.send_replace(Some(header(1, false)));
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(10), seen_rx.recv())
                    .await
                    .unwrap(),
                Some(1)
            );
            for number in 2..=10_001 {
                tx.send_replace(Some(header(number, false)));
            }
            if invalidate {
                tx.send_replace(None);
            }
            release_tx.send(()).unwrap();
            if invalidate {
                assert!(tokio::time::timeout(Duration::from_millis(200), seen_rx.recv())
                    .await
                    .is_err());
                tx.send_replace(Some(header(10_002, false)));
            }
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(10), seen_rx.recv())
                    .await
                    .unwrap(),
                Some(if invalidate { 10_002 } else { 10_001 })
            );
            drop(tx);
            tokio::time::timeout(Duration::from_secs(10), worker)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            server.join().unwrap();
        }
    }

    #[tokio::test]
    async fn invalid_initial_node_version_never_sends_http() {
        for mode in ["bad", "exit", "timeout"] {
            let node = NodeFixture::new(mode);
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let mut notifier = notifier(&node, format!("http://{}", listener.local_addr().unwrap()));
            let (tx, rx) = tokio::sync::watch::channel(None);
            tx.send_replace(Some(header(1, false)));
            assert!(tokio::time::timeout(Duration::from_secs(8), notifier.run(rx))
                .await
                .unwrap()
                .is_err());
            assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        }
    }

    #[test]
    fn config_rejects_missing_blank_or_empty_values_without_credentials_in_errors() {
        let dir = std::env::temp_dir().join(format!("cncli-config-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("config.json");
        assert!(get_pooltool_config(&path).is_err());
        for value in [
            serde_json::json!({"api_key": SECRET, "pools": []}),
            serde_json::json!({"api_key": " ", "pools": [{"name":"x","pool_id":"id","host":"localhost","port":1}]}),
            serde_json::json!({"api_key": SECRET, "pools": [{"name":"x","pool_id":" ","host":"localhost","port":1}]}),
        ] {
            std::fs::write(&path, value.to_string()).unwrap();
            assert!(!get_pooltool_config(&path).unwrap_err().to_string().contains(SECRET));
        }
        std::fs::write(
            &path,
            serde_json::json!({
                "api_key": SECRET, "pools": [{"name":"x","pool_id":"id","host":"localhost","port":1}]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(get_pooltool_config(&path).unwrap().pools.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
