use std::io::Write;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use futures::executor::block_on;
use net2::TcpStreamExt;
use pallas_miniprotocols::handshake;
use pallas_miniprotocols::handshake::Confirmation;
use pallas_multiplexer::bearers::Bearer;
use pallas_multiplexer::StdPlexer;
use serde::Serialize;

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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PingError {
    status: String,
    host: String,
    port: u16,
    error_message: String,
}

pub fn ping<W: Write>(out: &mut W, host: &str, port: u16, network_magic: u64, timeout_seconds: u64) {
    block_on(async {
        let start = Instant::now();
        let socket_addrs_result = format!("{host}:{port}").to_socket_addrs();
        match socket_addrs_result {
            Ok(mut socket_addrs) => {
                let socket_addr: &SocketAddr = &socket_addrs.next().unwrap();
                let dns_duration = start.elapsed();
                match Bearer::connect_tcp_timeout(socket_addr, Duration::from_secs(timeout_seconds)) {
                    Ok(bearer) => {
                        match &bearer {
                            Bearer::Tcp(tcp_stream) => {
                                tcp_stream.set_keepalive_ms(Some(30_000u32)).unwrap();
                                tcp_stream
                                    .set_read_timeout(Some(Duration::from_secs(timeout_seconds)))
                                    .unwrap();
                            }
                            Bearer::Unix(_) => {}
                        }
                        let connect_duration = start.elapsed() - dns_duration;

                        let mut plexer = StdPlexer::new(bearer);

                        //handshake is channel0
                        let channel0 = plexer.use_channel(0);

                        plexer.muxer.spawn();
                        plexer.demuxer.spawn();

                        let versions = handshake::n2n::VersionTable::v7_and_above(network_magic);
                        let mut client = handshake::N2NClient::new(channel0);
                        match client.handshake(versions) {
                            Ok(confirmation) => match confirmation {
                                Confirmation::Accepted(version_number, _) => {
                                    let total_duration = start.elapsed();
                                    let handshake_duration = total_duration - connect_duration - dns_duration;
                                    ping_json_success(
                                        out,
                                        dns_duration,
                                        connect_duration,
                                        handshake_duration,
                                        total_duration,
                                        version_number,
                                        host,
                                        port,
                                    );
                                }
                                Confirmation::Rejected(refuse_reason) => {
                                    ping_json_error(out, format!("{refuse_reason:?}"), host, port);
                                }
                            },
                            Err(error) => {
                                ping_json_error(out, format!("{error}"), host, port);
                            }
                        }
                    }
                    Err(error) => {
                        ping_json_error(out, error.to_string(), host, port);
                    }
                }
            }
            Err(error) => {
                ping_json_error(out, error.to_string(), host, port);
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn ping_json_success<W: Write>(
    out: &mut W,
    dns_duration: Duration,
    connect_duration: Duration,
    handshake_duration: Duration,
    total_duration: Duration,
    version_number: u64,
    host: &str,
    port: u16,
) {
    serde_json::ser::to_writer_pretty(
        out,
        &PingSuccess {
            status: "ok".to_string(),
            host: host.to_string(),
            port,
            network_protocol_version: version_number,
            dns_duration_ms: dns_duration.as_millis(),
            connect_duration_ms: connect_duration.as_millis(),
            handshake_duration_ms: handshake_duration.as_millis(),
            duration_ms: total_duration.as_millis(),
        },
    )
    .unwrap();
}

fn ping_json_error<W: Write>(out: &mut W, message: String, host: &str, port: u16) {
    serde_json::ser::to_writer_pretty(
        out,
        &PingError {
            status: "error".to_string(),
            host: host.to_string(),
            port,
            error_message: message,
        },
    )
    .unwrap();
}
