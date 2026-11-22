use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::nodeclient::sync::BlockHeader;
use chrono::{SecondsFormat, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::instrument::WithSubscriber;
use tracing::{error, info};

pub(crate) const POOLTOOL_BASE_URL: &str = "https://api.pooltool.io";
const NODE_VERSION_REFRESH_INTERVAL: Duration = Duration::from_secs(3600);

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

async fn read_node_version(cardano_node_path: &Path) -> Result<String, super::Error> {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(cardano_node_path)
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
    parse_node_version(&version)
}

pub(crate) struct NodeVersionWatcher {
    task: tokio::task::JoinHandle<()>,
}

impl NodeVersionWatcher {
    pub(crate) async fn spawn(
        cardano_node_path: PathBuf,
    ) -> Result<(Self, tokio::sync::watch::Receiver<String>), super::Error> {
        let initial_version = read_node_version(&cardano_node_path).await?;
        let (sender, receiver) = tokio::sync::watch::channel(initial_version);
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now() + NODE_VERSION_REFRESH_INTERVAL,
            NODE_VERSION_REFRESH_INTERVAL,
        );
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let task = tokio::spawn(
            async move {
                loop {
                    tokio::select! {
                        _ = sender.closed() => break,
                        _ = interval.tick() => {}
                    }
                    let refresh = tokio::select! {
                        _ = sender.closed() => break,
                        result = read_node_version(&cardano_node_path) => result,
                    };
                    match refresh {
                        Ok(version) => {
                            sender.send_if_modified(|current| {
                                if *current == version {
                                    false
                                } else {
                                    *current = version;
                                    true
                                }
                            });
                        }
                        Err(_) => error!("Cannot refresh cardano-node version; retaining verified version"),
                    }
                }
            }
            .with_current_subscriber(),
        );
        Ok((Self { task }, receiver))
    }

    #[cfg(test)]
    fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

impl Drop for NodeVersionWatcher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) struct PoolToolNotifier {
    pool_id: String,
    api_key: String,
    node_version: tokio::sync::watch::Receiver<String>,
    client: reqwest::Client,
    base_url: String,
}

impl PoolToolNotifier {
    pub(crate) fn new(
        _pool_name: String,
        pool_id: String,
        api_key: String,
        node_version: tokio::sync::watch::Receiver<String>,
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
            node_version,
            client,
            base_url,
        })
    }

    fn request_for(&self, header: &BlockHeader) -> Result<reqwest::RequestBuilder, super::Error> {
        let node_version = self.node_version.borrow().clone();
        let (endpoint, body) = if header.block_vrf_0.is_empty() {
            (
                "/v0/sendstats",
                serde_json::to_string(&PooltoolStats0 {
                    api_key: self.api_key.clone(),
                    pool_id: self.pool_id.clone(),
                    data: PooltoolData0 {
                        node_id: String::new(),
                        version: node_version,
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
                        version: node_version,
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
        loop {
            if tips.changed().await.is_err() {
                return Ok(());
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

fn parse_node_version(version: &str) -> Result<String, super::Error> {
    static HEADER_REGEX: std::sync::LazyLock<Result<Regex, regex::Error>> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?-u)^cardano-node (\d+(?:\.\d+){2,})(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?(?:\s|$)")
    });
    static GIT_REV_REGEX: std::sync::LazyLock<Result<Regex, regex::Error>> =
        std::sync::LazyLock::new(|| Regex::new(r"^git rev ([0-9a-fA-F]{5})[0-9a-fA-F]*$"));

    let header_regex = HEADER_REGEX
        .as_ref()
        .map_err(|_| super::Error::Reporter("Invalid node version matcher".into()))?;
    let git_rev_regex = GIT_REV_REGEX
        .as_ref()
        .map_err(|_| super::Error::Reporter("Invalid node revision matcher".into()))?;

    let mut lines = version.lines();
    let header = lines
        .next()
        .ok_or_else(|| super::Error::Reporter("Unrecognized cardano-node version output".into()))?;
    let release = header_regex
        .captures(header)
        .and_then(|captures| captures.get(1))
        .ok_or_else(|| super::Error::Reporter("Unrecognized cardano-node version output".into()))?
        .as_str();

    let revision = match lines.map(str::trim_end).find(|line| !line.is_empty()) {
        None | Some("git rev unknown") => "00000".to_owned(),
        Some(line) => git_rev_regex
            .captures(line)
            .and_then(|captures| captures.get(1))
            .map(|rev| rev.as_str().to_ascii_lowercase())
            .ok_or_else(|| super::Error::Reporter("Unrecognized cardano-node version output".into()))?,
    };

    Ok(format!("{release}:{revision}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    const SECRET: &str = "CNCLI_TEST_SECRET_DO_NOT_LOG";

    struct NodeFixture {
        dir: PathBuf,
        bin: PathBuf,
    }
    impl NodeFixture {
        fn new(mode: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("cncli-node-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("mode"), mode).unwrap();
            #[cfg(unix)]
            let bin = {
                use std::os::unix::fs::PermissionsExt;
                let path = dir.join("cardano-node");
                let script = concat!(
                    "#!/bin/sh\n",
                    "dir=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\n",
                    "printf '1\\n' >> \"$dir/spawns\"\n",
                    "mode=$(cat \"$dir/mode\")\n",
                    "case \"$mode\" in\n",
                    "  ok) printf 'cardano-node 10.1.0 linux\\ngit rev abcdef012345\\n' ;;\n",
                    "  upgrade) printf 'cardano-node 10.2.0 linux\\ngit rev 1234567890ab\\n' ;;\n",
                    "  multipart) printf 'cardano-node 11.2.0.164 - linux-x86_64 - ghc-9.6\\ngit rev 746f1b3c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a\\n' ;;\n",
                    "  two-part) printf 'cardano-node 11.2 - linux-x86_64\\ngit rev 746f1b3c9d8e\\n' ;;\n",
                    "  bad-rev) printf 'cardano-node 11.2.0.164 - linux-x86_64\\ngit rev 746f\\n' ;;\n",
                    "  bad) printf 'unrecognized\\n' ;;\n",
                    "  exit) exit 1 ;;\n",
                    "  timeout) exec sleep 10 ;;\n",
                    "  *) exit 2 ;;\n",
                    "esac\n",
                );
                std::fs::write(&path, script).unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
                path
            };
            #[cfg(windows)]
            let bin = {
                let path = dir.join("cardano-node.cmd");
                let script = concat!(
                    "@echo off\r\n",
                    "echo 1>>\"%~dp0spawns\"\r\n",
                    "set /p mode=<\"%~dp0mode\"\r\n",
                    "if \"%mode%\"==\"ok\" (\r\n",
                    "  echo cardano-node 10.1.0 windows\r\n",
                    "  echo git rev abcdef012345\r\n",
                    "  exit /b 0\r\n",
                    ")\r\n",
                    "if \"%mode%\"==\"upgrade\" (\r\n",
                    "  echo cardano-node 10.2.0 windows\r\n",
                    "  echo git rev 1234567890ab\r\n",
                    "  exit /b 0\r\n",
                    ")\r\n",
                    "if \"%mode%\"==\"multipart\" (\r\n",
                    "  echo cardano-node 11.2.0.164 - windows-x86_64 - ghc-9.6\r\n",
                    "  echo git rev 746f1b3c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a\r\n",
                    "  exit /b 0\r\n",
                    ")\r\n",
                    "if \"%mode%\"==\"two-part\" (\r\n",
                    "  echo cardano-node 11.2 - windows-x86_64\r\n",
                    "  echo git rev 746f1b3c9d8e\r\n",
                    "  exit /b 0\r\n",
                    ")\r\n",
                    "if \"%mode%\"==\"bad-rev\" (\r\n",
                    "  echo cardano-node 11.2.0.164 - windows-x86_64\r\n",
                    "  echo git rev 746f\r\n",
                    "  exit /b 0\r\n",
                    ")\r\n",
                    "if \"%mode%\"==\"bad\" (\r\n",
                    "  echo unrecognized\r\n",
                    "  exit /b 0\r\n",
                    ")\r\n",
                    "if \"%mode%\"==\"exit\" exit /b 1\r\n",
                    "if \"%mode%\"==\"timeout\" ping -n 11 127.0.0.1 >nul\r\n",
                );
                std::fs::write(&path, script).unwrap();
                path
            };
            Self { dir, bin }
        }

        fn set_mode(&self, mode: &str) {
            std::fs::write(self.dir.join("mode"), mode).unwrap();
        }

        fn spawns(&self) -> usize {
            std::fs::read_to_string(self.dir.join("spawns"))
                .map(|contents| contents.lines().count())
                .unwrap_or(0)
        }
    }
    impl Drop for NodeFixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).unwrap();
        }
    }
    fn notifier_with_pool(
        pool_id: &str,
        node_version: tokio::sync::watch::Receiver<String>,
        base_url: String,
    ) -> PoolToolNotifier {
        PoolToolNotifier::new(
            "test".into(),
            pool_id.into(),
            SECRET.into(),
            node_version,
            reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            base_url,
        )
        .unwrap()
    }
    fn notifier(node_version: tokio::sync::watch::Receiver<String>, base_url: String) -> PoolToolNotifier {
        notifier_with_pool("短", node_version, base_url)
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
            let (_watcher, node_version) = NodeVersionWatcher::spawn(node.bin.clone()).await.unwrap();
            let mut notifier = notifier(node_version, format!("http://{}", listener.local_addr().unwrap()));
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
            let (_watcher, node_version) = NodeVersionWatcher::spawn(node.bin.clone()).await.unwrap();
            let mut notifier = notifier(node_version, format!("http://{}", listener.local_addr().unwrap()));
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

    #[test]
    fn parse_node_version_accepts_multipart_and_rejects_malformed_outputs() {
        let output_format = Regex::new(r"(?-u)^\d+(?:\.\d+){2,}:[0-9a-f]{5}$").unwrap();
        for (input, expected) in [
            ("cardano-node 10.1.0 linux\ngit rev abcdef012345\n", "10.1.0:abcde"),
            (
                "cardano-node 11.1.3 - linux-x86_64 - ghc-9.6\ngit rev 1a2b3c4d5e6f7890abcdef1234567890abcdef12\n",
                "11.1.3:1a2b3",
            ),
            (
                "cardano-node 11.2.0.164 - linux-x86_64 - ghc-9.6\ngit rev 746f1b3c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a\n",
                "11.2.0.164:746f1",
            ),
            (
                "cardano-node 11.2.0.164.1 - linux-x86_64 - ghc-9.6\ngit rev 746f1b3c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a\n",
                "11.2.0.164.1:746f1",
            ),
            (
                "cardano-node 8.5.0.0 - linux-x86_64 - ghc-8.10\ngit rev a29ee68ba2c850cd50be39a3105ef191cfbc41d5\n",
                "8.5.0.0:a29ee",
            ),
            (
                "cardano-node 11.2.0-pre - linux-x86_64 - ghc-9.6\ngit rev 746f1b3c9d8e\n",
                "11.2.0:746f1",
            ),
            (
                "cardano-node 11.2.0-rc1 - linux-x86_64 - ghc-9.6\ngit rev 746f1b3c9d8e\n",
                "11.2.0:746f1",
            ),
            (
                "cardano-node 11.2.0+build - linux-x86_64 - ghc-9.6\ngit rev 746f1b3c9d8e\n",
                "11.2.0:746f1",
            ),
            ("cardano-node 11.2.0.164-rc.1+build.42\ngit rev 746f1\n", "11.2.0.164:746f1"),
            (
                "cardano-node 11.1.3 - linux-x86_64 - ghc-9.6\ngit rev ABCDEF0123456789ABCDEF0123456789ABCDEF01\n",
                "11.1.3:abcde",
            ),
            (
                "cardano-node 11.2.0.164 - linux-x86_64 - ghc-9.6\ngit rev 746F1B3C\n",
                "11.2.0.164:746f1",
            ),
            (
                "cardano-node 11.2.0.164 - linux-x86_64 - ghc-9.6\ngit rev 0000000000000000000000000000000000000000\n",
                "11.2.0.164:00000",
            ),
            (
                "cardano-node 11.2.0.164 - linux-x86_64 - ghc-9.6\ngit rev unknown\n",
                "11.2.0.164:00000",
            ),
            (
                "cardano-node 11.2.0.164 - linux-x86_64 - ghc-9.6\n",
                "11.2.0.164:00000",
            ),
            ("cardano-node 11.2.0.164", "11.2.0.164:00000"),
            (
                "cardano-node 11.2.0.164 - windows-x86_64 - ghc-9.6\r\ngit rev 746F1B3C9D8E\r\n",
                "11.2.0.164:746f1",
            ),
            ("cardano-node 10.1.0 windows\r\ngit rev unknown\r\n", "10.1.0:00000"),
            ("cardano-node 11.2.0.164.1\r\n", "11.2.0.164.1:00000"),
        ] {
            let parsed = parse_node_version(input).unwrap_or_else(|err| panic!("rejected {input:?}: {err}"));
            assert_eq!(parsed, expected, "unexpected version for {input:?}");
            assert!(output_format.is_match(&parsed), "invalid format {parsed:?}");
        }

        for invalid in [
            "",
            "unrecognized\n",
            "cardano-cli 11.2.0.164 - linux-x86_64 - ghc-9.6\ngit rev 746f1b3c\n",
            "my-cardano-node 10.1.0 - linux\ngit rev abcdef012345\n",
            "cardano-node 11 - linux-x86_64\ngit rev 746f1b3c\n",
            "cardano-node 11.2 - linux-x86_64\ngit rev 746f1b3c\n",
            "cardano-node v11.2.0 - linux-x86_64\ngit rev 746f1b3c\n",
            "cardano-node 11.2.0. - linux-x86_64\ngit rev 746f1b3c\n",
            "cardano-node 11.2.0.164. - linux-x86_64\ngit rev 746f1b3c\n",
            "cardano-node 11.2.0_rc1 - linux-x86_64\ngit rev 746f1b3c\n",
            "cardano-node 11.2.0- - linux-x86_64\ngit rev 746f1b3c\n",
            "cardano-node 11.2.0+ - linux-x86_64\ngit rev 746f1b3c\n",
            "cardano-node 11.2.0.164 - linux-x86_64\ngit rev\n",
            "cardano-node 11.2.0.164 - linux-x86_64\ngit rev \n",
            "cardano-node 11.2.0.164 - linux-x86_64\ngit rev 746f\n",
            "cardano-node 11.2.0.164 - linux-x86_64\ngit rev ggggg\n",
            "cardano-node 11.2.0.164 - linux-x86_64\ngit rev 746f1xyz\n",
            "cardano-node 11.2.0.164 - linux-x86_64\ngit rev unknown extra\n",
            "cardano-node 11.2.0.164 - linux-x86_64\nunrecognized revision line\n",
        ] {
            assert!(
                matches!(parse_node_version(invalid), Err(super::super::Error::Reporter(_))),
                "unexpectedly accepted {invalid:?}"
            );
        }
    }

    #[tokio::test]
    async fn worker_sends_multipart_node_version() {
        for v1 in [false, true] {
            let node = NodeFixture::new("multipart");
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let (_watcher, node_version) = NodeVersionWatcher::spawn(node.bin.clone()).await.unwrap();
            let mut notifier = notifier(node_version, format!("http://{}", listener.local_addr().unwrap()));
            let (tx, rx) = tokio::sync::watch::channel(None);
            let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel();
            let server = std::thread::spawn(move || {
                let (stream, _, payload) = receive(&listener);
                respond(stream, 200);
                seen_tx.send(payload).unwrap();
            });
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_writer(std::io::sink)
                .finish();
            let worker = tokio::spawn(async move { notifier.run(rx).await }.with_subscriber(subscriber));
            tx.send_replace(Some(header(1, v1)));
            let payload = tokio::time::timeout(Duration::from_secs(10), seen_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(payload["data"]["version"], "11.2.0.164:746f1");
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
        for mode in ["bad", "two-part", "bad-rev", "exit", "timeout"] {
            let node = NodeFixture::new(mode);
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let (tx, rx) = tokio::sync::watch::channel(None);
            tx.send_replace(Some(header(1, false)));
            let startup = async {
                let (_watcher, node_version) = NodeVersionWatcher::spawn(node.bin.clone()).await?;
                let mut notifier = notifier(node_version, base_url);
                notifier.run(rx).await
            };
            assert!(tokio::time::timeout(Duration::from_secs(8), startup)
                .await
                .unwrap()
                .is_err());
            assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn shared_watcher_updates_multiple_notifiers_once_per_hour_and_stops_when_closed() {
        tokio::time::resume();
        let node = NodeFixture::new("ok");
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (watcher, node_version) = NodeVersionWatcher::spawn(node.bin.clone()).await.unwrap();
        assert_eq!(node.spawns(), 1);

        let mut observer = node_version.clone();
        let mut notifier_a = notifier_with_pool("pool-a", node_version.clone(), base_url.clone());
        let mut notifier_b = notifier_with_pool("pool-b", node_version, base_url);
        let (tx_a, rx_a) = tokio::sync::watch::channel(None);
        let (tx_b, rx_b) = tokio::sync::watch::channel(None);
        let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel();
        let server = std::thread::spawn(move || {
            for _ in 0..4 {
                let (stream, _, payload) = receive(&listener);
                respond(stream, 200);
                seen_tx.send(payload).unwrap();
            }
        });
        let worker_a = tokio::spawn(async move { notifier_a.run(rx_a).await });
        let worker_b = tokio::spawn(async move { notifier_b.run(rx_b).await });

        tx_a.send_replace(Some(header(1, false)));
        tx_b.send_replace(Some(header(1, false)));
        for _ in 0..2 {
            let payload = tokio::time::timeout(Duration::from_secs(10), seen_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(payload["data"]["version"], "10.1.0:abcde");
        }
        assert_eq!(node.spawns(), 1);

        node.set_mode("upgrade");
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(3600)).await;
        tokio::time::resume();
        tokio::time::timeout(Duration::from_secs(5), observer.changed())
            .await
            .unwrap()
            .unwrap();

        tx_a.send_replace(Some(header(2, false)));
        tx_b.send_replace(Some(header(2, false)));
        for _ in 0..2 {
            let payload = tokio::time::timeout(Duration::from_secs(10), seen_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(payload["data"]["version"], "10.2.0:12345");
        }
        assert_eq!(node.spawns(), 2);

        // MissedTickBehavior::Delay must coalesce overdue ticks into one refresh instead of bursting.
        node.set_mode("ok");
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(7500)).await;
        tokio::time::resume();
        tokio::time::timeout(Duration::from_secs(5), observer.changed())
            .await
            .unwrap()
            .unwrap();
        drop(observer);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(node.spawns(), 3);

        drop(tx_a);
        drop(tx_b);
        worker_a.await.unwrap().unwrap();
        worker_b.await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !watcher.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(3600)).await;
        tokio::time::resume();
        tokio::task::yield_now().await;
        assert_eq!(node.spawns(), 3);
        server.join().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn failed_or_timed_out_hourly_refresh_logs_retains_version_and_never_stalls_tips() {
        tokio::time::resume();
        let node = NodeFixture::new("ok");
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());

        let logs = Arc::new(Mutex::new(Vec::new()));
        let writer = LogWriter(logs.clone());
        let subscriber = tracing::Dispatch::new(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_ansi(false)
                .without_time()
                .with_writer(move || writer.clone())
                .finish(),
        );

        let (watcher, node_version) = async { NodeVersionWatcher::spawn(node.bin.clone()).await.unwrap() }
            .with_subscriber(subscriber.clone())
            .await;
        assert_eq!(node.spawns(), 1);

        let mut notifier = notifier(node_version, base_url);
        let (tx, rx) = tokio::sync::watch::channel(None);
        let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel();
        let server = std::thread::spawn(move || {
            for _ in 0..4 {
                let (stream, _, payload) = receive(&listener);
                respond(stream, 200);
                seen_tx.send(payload).unwrap();
            }
        });
        let worker = tokio::spawn(async move { notifier.run(rx).await }.with_subscriber(subscriber));

        let error_log_count = || {
            String::from_utf8(logs.lock().unwrap().clone())
                .unwrap()
                .matches("Cannot refresh cardano-node version; retaining verified version")
                .count()
        };

        for (step, mode) in [(1, "bad"), (2, "exit")] {
            node.set_mode(mode);
            let expected_spawns = step + 1;
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(3600)).await;
            tokio::time::resume();
            tokio::time::timeout(Duration::from_secs(5), async {
                while node.spawns() < expected_spawns || error_log_count() < step {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();

            let sent_at = Utc::now();
            let started = Instant::now();
            tx.send_replace(Some(header(step as u64, false)));
            let payload = tokio::time::timeout(Duration::from_secs(5), seen_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(started.elapsed() < Duration::from_secs(1));
            assert_eq!(payload["data"]["version"], "10.1.0:abcde");
            let stamped = chrono::DateTime::parse_from_rfc3339(payload["data"]["at"].as_str().unwrap())
                .unwrap()
                .with_timezone(&Utc);
            assert!((stamped - sent_at).num_milliseconds().abs() < 1000);
        }

        // Trigger an hourly refresh that hangs for 10s, then send a tip mid-refresh.
        node.set_mode("timeout");
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(3600)).await;
        tokio::time::resume();
        tokio::time::timeout(Duration::from_secs(5), async {
            while node.spawns() < 4 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();

        let sent_at = Utc::now();
        let tip_started = Instant::now();
        tx.send_replace(Some(header(3, false)));
        let payload = tokio::time::timeout(Duration::from_secs(1), seen_rx.recv())
            .await
            .expect("tip delivery stalled behind background refresh subprocess")
            .unwrap();
        assert!(tip_started.elapsed() < Duration::from_secs(1));
        assert_eq!(payload["data"]["version"], "10.1.0:abcde");
        let stamped = chrono::DateTime::parse_from_rfc3339(payload["data"]["at"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc);
        assert!((stamped - sent_at).num_milliseconds().abs() < 1000);

        // Advance paused clock by 5s to trip the background subprocess timeout without waiting 5s.
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::time::resume();
        tokio::time::timeout(Duration::from_secs(5), async {
            while error_log_count() < 3 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();

        tx.send_replace(Some(header(4, false)));
        let payload = tokio::time::timeout(Duration::from_secs(5), seen_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(payload["data"]["version"], "10.1.0:abcde");

        drop(tx);
        worker.await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !watcher.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        server.join().unwrap();

        assert_eq!(error_log_count(), 3);
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
