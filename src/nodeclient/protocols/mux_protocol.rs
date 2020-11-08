use std::io::{Error, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::exit;
use std::thread::sleep;
use std::time::{Duration, Instant};

use byteorder::{ByteOrder, NetworkEndian, WriteBytesExt};
use log::{debug, error, info, warn};
use net2::TcpStreamExt;

use crate::nodeclient::protocols::{Agency, MiniProtocol, Protocol};
use crate::nodeclient::protocols::chainsync_protocol::{ChainSyncProtocol, Mode};
use crate::nodeclient::protocols::handshake_protocol::HandshakeProtocol;
use crate::nodeclient::protocols::transaction_protocol::TxSubmissionProtocol;

#[derive(PartialEq)]
pub enum Cmd {
    Ping,
    Sync,
    SendTip,
}

// Sync a cardano-node database
//
// Connect to cardano-node and run protocols depending on command type
pub fn start(cmd: Cmd, db: &std::path::PathBuf, host: &String, port: u16, network_magic: u32, pooltool_api_key: &String, node_version: &String, pool_name: &String, pool_id: &String) {
    let start_time = Instant::now();

    // continually retry connection
    loop {
        if cmd != Cmd::Ping {
            info!("Connecting to {}:{} ...", host, port);
        }

        let mut protocols: Vec<MiniProtocol> = vec![
            MiniProtocol::Handshake(
                HandshakeProtocol {
                    network_magic,
                    ..Default::default()
                }
            )
        ];

        match (host.as_str(), port).to_socket_addrs() {
            Ok(mut into_iter) => {
                if into_iter.len() > 0 {
                    let socket_addr: SocketAddr = into_iter.nth(0).unwrap();
                    match TcpStream::connect_timeout(&socket_addr, Duration::from_secs(1)) {
                        Ok(mut stream) => {
                            stream.set_nodelay(true).unwrap();
                            stream.set_keepalive_ms(Some(10_000u32)).unwrap();
                            let mut last_data_timestamp = Instant::now();
                            loop {
                                // Try sending some data
                                match mux_send_data(&start_time, &mut protocols, &mut stream) {
                                    Ok(did_send_data) => {
                                        if did_send_data {
                                            last_data_timestamp = Instant::now();
                                        }
                                    }
                                    Err(e) => {
                                        handle_error(&cmd, format!("mux_send_data error: {}", e), host, port);
                                        break;
                                    }
                                }

                                // only read from the server if no protocols have client agency and
                                // at least one has Server agency
                                let should_read_from_server =
                                    !protocols.iter().any(|protocol| protocol.get_agency() == Agency::Client)
                                        && protocols.iter().any(|protocol| protocol.get_agency() == Agency::Server);

                                if should_read_from_server {
                                    // try receiving some data
                                    match mux_receive_data(&mut protocols, &mut stream) {
                                        Ok(did_receive_data) => {
                                            if did_receive_data {
                                                last_data_timestamp = Instant::now();
                                            }
                                        }
                                        Err(e) => {
                                            handle_error(&cmd, format!("mux_receive_data error: {}", e), host, port);
                                            break;
                                        }
                                    }
                                }

                                // Add and Remove protocols depending on status
                                mux_add_remove_protocols(&cmd, db, network_magic, &mut protocols, host, port, pooltool_api_key, node_version, pool_name, pool_id);

                                if protocols.is_empty() {
                                    match cmd {
                                        Cmd::Ping => {
                                            // for ping, we need to print the final output
                                            ping_json_success(start_time.elapsed(), host, port);
                                        }
                                        _ => { warn!("No more active protocols, exiting..."); }
                                    }
                                    return;
                                }

                                if last_data_timestamp.elapsed() > Duration::from_secs(60) {
                                    for protocol in protocols.iter() {
                                        error!("state: {}", protocol.get_state());
                                    }
                                    error!("No communication for over 1 minute from server! restarting connection...");
                                    break;
                                }
                            }

                            // shutdown stream and ignore errors
                            match stream.shutdown(Shutdown::Both) {
                                Ok(_) => {}
                                Err(error) => {
                                    handle_error(&cmd, format!("{}", error), host, port);
                                }
                            }
                        }
                        Err(e) => {
                            handle_error(&cmd, format!("Failed to connect: {}", e), host, port);
                        }
                    }
                } else {
                    handle_error(&cmd, format!("No IP addresses found!"), host, port);
                }
            }
            Err(error) => {
                handle_error(&cmd, format!("{}", error), host, port);
            }
        }

        // Wait a bit before trying again
        sleep(Duration::from_secs(5))
    }
}

fn mux_send_data(start_time: &Instant, protocols: &mut Vec<MiniProtocol>, stream: &mut TcpStream) -> Result<bool, Error> {
    let mut did_send_data = false;
    for protocol in protocols.iter_mut() {
        match protocol.send_data() {
            Some(send_payload) => {
                let mut message: Vec<u8> = Vec::new();
                message.write_u32::<NetworkEndian>(timestamp(&start_time)).unwrap();
                message.write_u16::<NetworkEndian>(protocol.protocol_id()).unwrap();
                message.write_u16::<NetworkEndian>(send_payload.len() as u16).unwrap();
                message.write(&send_payload[..]).unwrap();
                // debug!("sending: {}", hex::encode(&message));
                stream.write(&message)?;
                did_send_data = true;
                break;
            }
            None => {}
        }
    }

    Ok(did_send_data)
}

fn mux_receive_data(protocols: &mut Vec<MiniProtocol>, stream: &mut TcpStream) -> Result<bool, Error> {
    let mut did_receive_data = false;
    let mut message_header = [0u8; 8]; // read 8 bytes to start with
    let size = stream.peek(&mut message_header)?;
    if size == 8 {
        stream.read_exact(&mut message_header)?;
        let _server_timestamp = NetworkEndian::read_u32(&mut message_header[0..4]);
        // println!("server_timestamp: {:x}", server_timestamp);
        let protocol_id = NetworkEndian::read_u16(&mut message_header[4..6]);
        // println!("protocol_id: {:x}", protocol_id);
        let payload_length = NetworkEndian::read_u16(&mut message_header[6..]) as usize;
        // println!("payload_length: {:x}", payload_length);
        let mut payload = vec![0u8; payload_length];

        let timeout_check = Instant::now();
        loop {
            let size = stream.peek(&mut payload)?;
            if size != payload_length {
                if timeout_check.elapsed() > Duration::from_secs(60) {
                    panic!("Waiting for payload_length: {}, but available is: {}", payload_length, size);
                }
                continue;
            }
            stream.read_exact(&mut payload)?;
            // Find the protocol to handle the message
            for protocol in protocols.iter_mut() {
                if protocol_id == (protocol.protocol_id() | 0x8000u16) {
                    // println!("receive_data: {}", hex::encode(&payload));
                    protocol.receive_data(payload);
                    did_receive_data = true;
                    break;
                }
            }
            // We processed the data, break out of loop
            break;
        }
    }

    Ok(did_receive_data)
}

fn mux_add_remove_protocols(cmd: &Cmd, db: &PathBuf, network_magic: u32, protocols: &mut Vec<MiniProtocol>, host: &String, port: u16, pooltool_api_key: &String, node_version: &String, pool_name: &String, pool_id: &String) {
    let mut protocols_to_add: Vec<MiniProtocol> = Vec::new();
    // Remove any protocols that have a result (are done)
    protocols.retain(|protocol| {
        match protocol {
            MiniProtocol::Handshake(handshake_protocol) => {
                match handshake_protocol.result.as_ref() {
                    Some(protocol_result) => {
                        match protocol_result {
                            Ok(message) => {
                                debug!("HandshakeProtocol Result: {}", message);

                                match cmd {
                                    Cmd::Ping => {}
                                    Cmd::Sync => {
                                        // handshake succeeded. Add other protocols to continue sync
                                        protocols_to_add.push(
                                            MiniProtocol::TxSubmission(TxSubmissionProtocol::default())
                                        );
                                        let mut chain_sync_protocol = ChainSyncProtocol {
                                            mode: Mode::Sync,
                                            network_magic,
                                            ..Default::default()
                                        };
                                        chain_sync_protocol.init_database(db).expect("Error opening database!");

                                        protocols_to_add.push(
                                            MiniProtocol::ChainSync(chain_sync_protocol)
                                        );
                                    }
                                    Cmd::SendTip => {
                                        // handshake succeeded. Add other protocols to continue sync
                                        protocols_to_add.push(
                                            MiniProtocol::TxSubmission(TxSubmissionProtocol::default())
                                        );
                                        protocols_to_add.push(
                                            MiniProtocol::ChainSync(ChainSyncProtocol {
                                                mode: Mode::SendTip,
                                                network_magic,
                                                pooltool_api_key: pooltool_api_key.clone(),
                                                node_version: node_version.clone(),
                                                pool_name: pool_name.clone(),
                                                pool_id: pool_id.clone(),
                                                ..Default::default()
                                            })
                                        );
                                    }
                                }
                            }
                            Err(error) => {
                                handle_error(cmd, format!("HandshakeProtocol Error: {}", error), host, port);
                            }
                        }
                        false
                    }
                    None => { true }
                }
            }
            MiniProtocol::TxSubmission(tx_submission_protocol) => {
                match tx_submission_protocol.result.as_ref() {
                    Some(protocol_result) => {
                        match protocol_result {
                            Ok(message) => {
                                debug!("TxSubmissionProtocol Result: {}", message);
                            }
                            Err(error) => {
                                error!("TxSubmissionProtocol Error: {}", error);
                            }
                        }
                        false
                    }
                    None => { true }
                }
            }
            MiniProtocol::ChainSync(chainsync_protocol) => {
                match chainsync_protocol.result.as_ref() {
                    Some(protocol_result) => {
                        match protocol_result {
                            Ok(message) => {
                                debug!("ChainSyncProtocol Result: {}", message);
                            }
                            Err(error) => {
                                error!("ChainSyncProtocol Error: {}", error);
                            }
                        }
                        false
                    }
                    None => { true }
                }
            }
        }
    });

    protocols.append(&mut protocols_to_add);
}

fn ping_json_success(duration: Duration, host: &String, port: u16) {
    println!("{{\n\
        \x20\"status\": \"ok\",\n\
        \x20\"host\": \"{}\",\n\
        \x20\"port\": {},\n\
        \x20\"durationMs\": {}\n\
    }}", host, port, duration.as_millis())
}

fn handle_error(cmd: &Cmd, message: String, host: &String, port: u16) {
    match cmd {
        Cmd::Ping => { ping_json_error(message, host, port); }
        _ => { error!("{}", message); }
    }
}

fn ping_json_error(message: String, host: &String, port: u16) {
    println!("{{\n\
        \x20\"status\": \"error\",\n\
        \x20\"host\": \"{}\",\n\
        \x20\"port\": {},\n\
        \x20\"errorMessage\": \"{}\"\n\
    }}", host, port, message);
    // blow out of the process
    exit(0);
}

// return microseconds from the monotonic clock dropping all bits above 32.
fn timestamp(start: &Instant) -> u32 {
    start.elapsed().as_micros() as u32
}