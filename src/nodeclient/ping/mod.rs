use std::io::{self, Write};
use std::time::{Duration, Instant};

use pallas_network::miniprotocols::handshake::Confirmation;
use pallas_network::miniprotocols::{handshake, PROTOCOL_N2N_HANDSHAKE};
use pallas_network::multiplexer::{Bearer, Plexer};
use serde::Serialize;
use tokio::time::{timeout_at, Instant as TokioInstant};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PingSuccess {
    status: String,
    host: String,
    port: u16,
    network_protocol_version: u64,
    dns_duration_ms: u128,
    connect_duration_ms: u128,
    handshake_duration_ms: u128,
    duration_ms: u128,
}

pub async fn ping<W: Write>(
    out: &mut W,
    host: &str,
    port: u16,
    network_magic: u64,
    timeout_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let start = Instant::now();
    let deadline = TokioInstant::now()
        .checked_add(Duration::from_secs(timeout_seconds))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Ping timeout is too large"))?;
    let (bearer, dns_duration, connect_duration) = timeout_at(deadline, async {
        let addresses = tokio::net::lookup_host((host, port)).await?;
        let dns_duration = start.elapsed();
        let mut last_error = io::Error::new(io::ErrorKind::NotFound, "DNS returned no addresses");
        for address in addresses {
            match Bearer::connect_tcp(address).await {
                Ok(bearer) => return Ok((bearer, dns_duration, start.elapsed() - dns_duration)),
                Err(error) => last_error = error,
            }
        }
        Err(last_error)
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Ping deadline exceeded"))??;

    let mut plexer = Plexer::new(bearer);
    let hs_channel = plexer.subscribe_client(PROTOCOL_N2N_HANDSHAKE);
    let running_plexer = plexer.spawn();
    let mut client = handshake::Client::new(hs_channel);
    let outcome = timeout_at(
        deadline,
        client.handshake(handshake::n2n::VersionTable::v7_and_above(network_magic)),
    )
    .await;
    let total_duration = start.elapsed();
    // Keep cleanup outside the timed future so cancellation cannot detach the plexer.
    running_plexer.abort().await;
    let confirmation = outcome.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Ping deadline exceeded"))??;
    let version_number = match confirmation {
        Confirmation::Accepted(version_number, _) => version_number,
        Confirmation::Rejected(reason) => {
            return Err(io::Error::other(format!("Handshake rejected: {reason:?}")).into());
        }
        Confirmation::QueryReply(_) => {
            return Err(io::Error::other("Unexpected handshake QueryReply").into());
        }
    };

    crate::write_json(
        out,
        &PingSuccess {
            status: "ok".to_string(),
            host: host.to_string(),
            port,
            network_protocol_version: version_number,
            dns_duration_ms: dns_duration.as_millis(),
            connect_duration_ms: connect_duration.as_millis(),
            handshake_duration_ms: (total_duration - connect_duration - dns_duration).as_millis(),
            duration_ms: total_duration.as_millis(),
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
    use tokio::task::JoinHandle;

    async fn handshake_peer(network_magic: u64) -> (u16, oneshot::Sender<()>, JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (done, wait) = oneshot::channel();
        let task = tokio::spawn(async move {
            let (bearer, _) = Bearer::accept_tcp(&listener).await.unwrap();
            let mut plexer = Plexer::new(bearer);
            let channel = plexer.subscribe_server(PROTOCOL_N2N_HANDSHAKE);
            let running_plexer = plexer.spawn();
            let mut server = handshake::N2NServer::new(channel);
            server
                .handshake(handshake::n2n::VersionTable::v7_and_above(network_magic))
                .await
                .unwrap();
            let _ = wait.await;
            running_plexer.abort().await;
        });
        (port, done, task)
    }

    #[tokio::test]
    async fn accepted_handshake_writes_success() {
        let (port, done, peer) = handshake_peer(1).await;
        let mut output = Vec::new();
        ping(&mut output, "127.0.0.1", port, 1, 2).await.unwrap();
        done.send(()).unwrap();
        peer.await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["host"], "127.0.0.1");
        assert_eq!(body["port"], port);
        assert!(body["networkProtocolVersion"].as_u64().unwrap() >= 7);
        for field in [
            "dnsDurationMs",
            "connectDurationMs",
            "handshakeDurationMs",
            "durationMs",
        ] {
            assert!(body[field].is_number());
        }
        assert_eq!(output.last(), Some(&b'\n'));
    }

    #[tokio::test]
    async fn rejected_handshake_returns_error_without_output() {
        let (port, done, peer) = handshake_peer(2).await;
        let mut output = Vec::new();
        assert!(ping(&mut output, "127.0.0.1", port, 1, 2).await.is_err());
        done.send(()).unwrap();
        peer.await.unwrap();
        assert!(output.is_empty());
    }

    #[tokio::test]
    async fn silent_handshake_obeys_total_deadline() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (done, wait) = oneshot::channel();
        let peer = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            let _ = wait.await;
        });
        let mut output = Vec::new();
        let start = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(3), ping(&mut output, "127.0.0.1", port, 1, 1))
            .await
            .expect("silent handshake exceeded bounded harness wait");
        let error = result.unwrap_err();
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(output.is_empty());
        done.send(()).unwrap();
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn refused_connection_returns_error_without_output() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let mut output = Vec::new();
        assert!(ping(&mut output, "127.0.0.1", port, 1, 2).await.is_err());
        assert!(output.is_empty());
    }

    struct FailingWriter {
        fail_flush: bool,
    }

    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_flush {
                Ok(bytes.len())
            } else {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed test pipe"))
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("test flush failure"))
        }
    }

    #[tokio::test]
    async fn writer_failures_preserve_output_error_identity() {
        for fail_flush in [false, true] {
            let (port, done, peer) = handshake_peer(1).await;
            let error = ping(&mut FailingWriter { fail_flush }, "127.0.0.1", port, 1, 2)
                .await
                .unwrap_err();
            assert!(error.downcast_ref::<crate::OutputError>().is_some());
            done.send(()).unwrap();
            peer.await.unwrap();
        }
    }
}
