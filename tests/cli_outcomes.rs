use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use pallas_network::facades::{KeepAliveHandle, KeepAliveLoop, PeerServer};
use pallas_network::miniprotocols::chainsync::{ClientRequest, HeaderContent, N2NServer, Tip};
use pallas_network::miniprotocols::{Point, MAINNET_MAGIC};
use pallas_network::multiplexer::RunningPlexer;
use pallas_traverse::MultiEraHeader;
use serde_json::Value;
use tokio::net::TcpListener;

const LIMIT: Duration = Duration::from_secs(20);

struct FixtureDir(PathBuf);
impl FixtureDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("cncli-outcomes-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// Every child is reaped even when a protocol assertion or deadline fails.
struct Process(Option<Child>);
impl Process {
    fn spawn(command: &mut Command) -> Self {
        Self(Some(command.spawn().unwrap()))
    }
    async fn output(mut self) -> Output {
        let deadline = Instant::now() + LIMIT;
        loop {
            if self.0.as_mut().unwrap().try_wait().unwrap().is_some() {
                return self.0.take().unwrap().wait_with_output().unwrap();
            }
            assert!(Instant::now() < deadline, "CLI exceeded harness deadline");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    fn stop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            child.wait().unwrap();
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}

fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cncli"));
    command
        .env("RUST_LOG", "error")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}
async fn run(args: &[&str]) -> Output {
    Process::spawn(cli().args(args)).output().await
}
fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid single JSON response: {error}; stdout={}; stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
fn error(output: &Output) -> Value {
    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = json(output);
    assert_eq!(body["status"], "error");
    assert!(body["errorMessage"].is_string());
    body
}
async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(LIMIT, future)
        .await
        .expect("local peer exceeded harness deadline")
}

#[tokio::test]
async fn configuration_read_and_json_failures_are_command_errors() {
    let dir = FixtureDir::new();
    let config = dir.0.join("pooltool.json");
    for contents in [None, Some("{"), Some("{}"), Some("{\"api_key\":\"test\",\"pools\":[]}")] {
        if let Some(contents) = contents {
            fs::write(&config, contents).unwrap();
        }
        error(
            &run(&[
                "sendtip",
                "--config",
                config.to_str().unwrap(),
                "--cardano-node",
                "unused-node",
            ])
            .await,
        );
    }
    error(
        &run(&[
            "sendtip",
            "--config",
            dir.0.to_str().unwrap(),
            "--cardano-node",
            "unused-node",
        ])
        .await,
    );
}

fn ping(port: u16) -> Process {
    Process::spawn(cli().args([
        "ping",
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--timeout-seconds",
        "1",
    ]))
}

#[tokio::test]
async fn refused_and_silent_ping_obey_whole_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let body = error(&ping(port).output().await);
    assert_eq!(body["host"], "127.0.0.1");
    assert_eq!(body["port"], port);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let started = Instant::now();
    let child = ping(listener.local_addr().unwrap().port());
    let (_silent_connection, _) = bounded(listener.accept()).await.unwrap();
    let body = error(&child.output().await);
    assert_eq!(body["host"], "127.0.0.1");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "timeout did not cover handshake"
    );
}

#[tokio::test]
async fn accepted_and_rejected_ping_use_real_handshake() {
    for accepted in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let child = ping(port);
        let peer = bounded(PeerServer::accept(
            &listener,
            if accepted { MAINNET_MAGIC } else { MAINNET_MAGIC + 1 },
        ))
        .await;
        let output = child.output().await;
        if accepted {
            let peer = peer.unwrap();
            assert!(output.status.success());
            let body = json(&output);
            assert_eq!(body["status"], "ok");
            assert_eq!(body["host"], "127.0.0.1");
            assert_eq!(body["port"], port);
            peer.abort().await;
        } else {
            assert!(peer.is_err());
            error(&output);
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn closed_stdout_before_launch_is_exit_one_not_panic() {
    use std::os::fd::{FromRawFd, OwnedFd};
    unsafe extern "C" {
        fn pipe(fds: *mut std::ffi::c_int) -> std::ffi::c_int;
    }
    let closed_writer = || {
        let mut fds = [-1; 2];
        // Closing the reader before spawn makes writes fail without a race.
        assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);
        let reader = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let writer = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        drop(reader);
        Stdio::from(writer)
    };
    for args in [
        vec!["challenge", "--domain", "example.test"],
        vec![
            "sendtip",
            "--config",
            "/dev/null/missing-config",
            "--cardano-node",
            "/dev/null/missing-node",
        ],
    ] {
        for close_stderr in [false, true] {
            let mut command = cli();
            command.args(&args).stdout(closed_writer());
            if close_stderr {
                command.stderr(closed_writer());
            }
            let output = Process::spawn(&mut command).output().await;
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
            assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
        }
    }
}

fn sync(db: &Path, port: u16, redb: bool, no_service: bool) -> Process {
    let mut command = cli();
    command.args([
        "sync",
        "--db",
        db.to_str().unwrap(),
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--shelley-genesis-hash",
        &"00".repeat(32),
    ]);
    if redb {
        command.arg("--use-redb");
    }
    if no_service {
        command.arg("--no-service");
    }
    Process::spawn(&mut command)
}

#[tokio::test]
async fn no_service_returns_first_connection_failure() {
    let dir = FixtureDir::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    for redb in [false, true] {
        let started = Instant::now();
        error(
            &sync(
                &dir.0.join(if redb { "failed.redb" } else { "failed.db" }),
                port,
                redb,
                true,
            )
            .output()
            .await,
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "no-service retried a failed connection"
        );
    }
}

#[derive(Clone)]
struct Header {
    content: HeaderContent,
    point: Point,
    hash: String,
    number: u64,
    slot: u64,
}
fn header(number: u64, slot: u64, previous: &[u8]) -> Header {
    let mut e = minicbor::Encoder::new(Vec::new());
    e.array(2)
        .unwrap()
        .array(15)
        .unwrap()
        .u64(number)
        .unwrap()
        .u64(slot)
        .unwrap()
        .bytes(previous)
        .unwrap();
    e.bytes(&[11; 32]).unwrap().bytes(&[12; 32]).unwrap();
    for marker in [number as u8, (number + 10) as u8] {
        e.array(2)
            .unwrap()
            .bytes(&[marker; 64])
            .unwrap()
            .bytes(&[marker; 80])
            .unwrap();
    }
    e.u64(1)
        .unwrap()
        .bytes(&[13; 32])
        .unwrap()
        .bytes(&[14; 32])
        .unwrap()
        .u64(0)
        .unwrap()
        .u64(0)
        .unwrap()
        .bytes(&[15; 64])
        .unwrap()
        .u64(2)
        .unwrap()
        .u64(0)
        .unwrap()
        .bytes(&[16; 448])
        .unwrap();
    let cbor = e.into_writer();
    let hash = MultiEraHeader::decode(1, None, &cbor).unwrap().hash();
    Header {
        content: HeaderContent {
            variant: 1,
            byron_prefix: None,
            cbor,
        },
        point: Point::Specific(slot, hash.to_vec()),
        hash: hex::encode(hash),
        number,
        slot,
    }
}
fn chain() -> [Header; 3] {
    let a = header(1, 10, &[0; 32]);
    let b = header(2, 20, &hex::decode(&a.hash).unwrap());
    let c = header(3, 100, &hex::decode(&b.hash).unwrap());
    [a, b, c]
}

struct Peer {
    chain: N2NServer,
    plexer: RunningPlexer,
    keepalive: KeepAliveHandle,
}
impl Peer {
    async fn accept(listener: &TcpListener) -> Self {
        let peer = bounded(PeerServer::accept(listener, MAINNET_MAGIC)).await.unwrap();
        Self {
            chain: peer.chainsync,
            plexer: peer.plexer,
            keepalive: KeepAliveLoop::server(peer.keepalive).spawn(),
        }
    }
    async fn intersect(&mut self, point: Point, tip: Tip) -> Vec<Point> {
        let points = match bounded(self.chain.recv_while_idle()).await.unwrap() {
            Some(ClientRequest::Intersect(points)) => points,
            other => panic!("expected intersection, got {other:?}"),
        };
        assert!(points.contains(&point), "peer selected an unproposed intersection");
        bounded(self.chain.send_intersect_found(point, tip)).await.unwrap();
        points
    }
    async fn next(&mut self) {
        assert!(matches!(
            bounded(self.chain.recv_while_idle()).await.unwrap(),
            Some(ClientRequest::RequestNext)
        ));
    }
    async fn forward(&mut self, header: &Header, tip: &Tip) {
        self.next().await;
        bounded(self.chain.send_roll_forward(header.content.clone(), tip.clone()))
            .await
            .unwrap();
    }
    async fn close(self) {
        self.keepalive.abort();
        let _ = self.keepalive.await;
        self.plexer.abort().await;
    }
}

async fn validate(db: &Path, header: &Header, status: &str) {
    let output = run(&["validate", "--db", db.to_str().unwrap(), "--hash", &header.hash]).await;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let body = json(&output);
    assert_eq!(body["status"], status);
    assert_eq!(body["hash"], header.hash);
    assert_eq!(body["block_number"], header.number.to_string());
    assert_eq!(body["slot_number"], header.slot.to_string());
}

async fn seed(db: &Path, redb: bool, headers: &[Header; 3]) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let child = sync(db, listener.local_addr().unwrap().port(), redb, true);
    let mut peer = Peer::accept(&listener).await;
    let tip = Tip(headers[2].point.clone(), 3);
    peer.intersect(Point::Origin, tip.clone()).await;
    for header in headers {
        peer.forward(header, &tip).await;
    }
    assert!(
        bounded(peer.chain.recv_while_idle()).await.unwrap().is_none(),
        "newly reached tip must send Done"
    );
    let output = child.output().await;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stdout.is_empty());
    peer.close().await;
    for header in headers {
        validate(db, header, "ok").await;
    }
}

#[tokio::test]
async fn newly_reached_tip_is_committed_and_already_at_tip_sends_done() {
    let dir = FixtureDir::new();
    let headers = chain();
    for redb in [false, true] {
        let db = dir.0.join(if redb { "tip.redb" } else { "tip.db" });
        seed(&db, redb, &headers).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let child = sync(&db, listener.local_addr().unwrap().port(), redb, true);
        let mut peer = Peer::accept(&listener).await;
        let points = peer
            .intersect(headers[2].point.clone(), Tip(headers[2].point.clone(), 3))
            .await;
        assert_eq!(points.first(), Some(&headers[2].point));
        assert!(
            bounded(peer.chain.recv_while_idle()).await.unwrap().is_none(),
            "already-at-tip must send Done without RequestNext"
        );
        assert!(child.output().await.status.success());
        peer.close().await;
        validate(&db, &headers[2], "ok").await;
    }
}

#[tokio::test]
async fn rollback_is_durable_before_any_replacement_header() {
    let dir = FixtureDir::new();
    let headers = chain();
    for redb in [false, true] {
        let db = dir.0.join(if redb { "rollback.redb" } else { "rollback.db" });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut child = sync(&db, listener.local_addr().unwrap().port(), redb, false);
        let mut peer = Peer::accept(&listener).await;
        let tip = Tip(headers[2].point.clone(), 3);
        peer.intersect(Point::Origin, tip.clone()).await;
        for header in &headers {
            peer.forward(header, &tip).await;
        }
        peer.next().await;
        bounded(peer.chain.send_roll_backward(headers[1].point.clone(), tip))
            .await
            .unwrap();
        // This request proves the rollback callback returned before the peer
        // sends Await; no replacement header can accidentally perform the fix.
        peer.next().await;
        bounded(peer.chain.send_await_reply()).await.unwrap();
        child.stop();
        peer.close().await;
        validate(&db, &headers[0], "ok").await;
        validate(&db, &headers[1], "ok").await;
        validate(&db, &headers[2], "orphaned").await;

        let child = sync(&db, listener.local_addr().unwrap().port(), redb, true);
        let mut peer = Peer::accept(&listener).await;
        let points = peer
            .intersect(headers[1].point.clone(), Tip(headers[1].point.clone(), 2))
            .await;
        assert_eq!(points.first(), Some(&headers[1].point));
        assert!(
            !points.contains(&headers[2].point),
            "orphan proposed as canonical intersection"
        );
        assert!(bounded(peer.chain.recv_while_idle()).await.unwrap().is_none());
        assert!(child.output().await.status.success());
        peer.close().await;
    }
}

#[tokio::test]
async fn reconnect_intersection_orphans_stale_suffix_before_forward_data() {
    let dir = FixtureDir::new();
    let headers = chain();
    for redb in [false, true] {
        let db = dir.0.join(if redb { "reconnect.redb" } else { "reconnect.db" });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut child = sync(&db, listener.local_addr().unwrap().port(), redb, false);
        let mut peer = Peer::accept(&listener).await;
        let tip = Tip(headers[2].point.clone(), 3);
        peer.intersect(Point::Origin, tip.clone()).await;
        for header in &headers {
            peer.forward(header, &tip).await;
        }
        peer.next().await;
        bounded(peer.chain.send_await_reply()).await.unwrap();
        peer.close().await;
        // Same executable reconnects after transport loss, reusing its store.
        let mut peer = Peer::accept(&listener).await;
        let points = peer.intersect(headers[1].point.clone(), tip).await;
        assert_eq!(points.first(), Some(&headers[2].point));
        peer.next().await;
        bounded(peer.chain.send_await_reply()).await.unwrap();
        child.stop();
        peer.close().await;
        validate(&db, &headers[0], "ok").await;
        validate(&db, &headers[1], "ok").await;
        validate(&db, &headers[2], "orphaned").await;
    }
}
