use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::nodeclient::protocols::handshake_protocol;

// Ping a remote cardano-node
//
// Ping connects to a remote cardano-node and runs the handshake protocol
pub fn ping(host: &String, port: u16, network_magic: u32) {
    let start_time = Instant::now();

    match (host.as_str(), port).to_socket_addrs() {
        Ok(mut into_iter) => {
            if into_iter.len() > 0 {
                let socket_addr: SocketAddr = into_iter.nth(0).unwrap();
                match TcpStream::connect_timeout(&socket_addr, Duration::from_secs(1)) {
                    Ok(stream) => {
                        match handshake_protocol::ping(&stream, timestamp(&start_time), network_magic) {
                            Ok(_payload) => {
                                stream.shutdown(Shutdown::Both).expect("shutdown call failed");
                                print_json_success(start_time.elapsed(), host, port);
                            }
                            Err(message) => {
                                stream.shutdown(Shutdown::Both).expect("shutdown call failed");
                                print_json_error(message, host, port);
                            }
                        }
                    }
                    Err(e) => {
                        print_json_error(format!("Failed to connect: {}", e), host, port);
                    }
                }
            } else {
                print_json_error("No IP addresses found!".to_owned(), host, port);
            }
        }
        Err(error) => {
            print_json_error(error.to_string(), host, port);
        }
    }
}

fn print_json_success(duration: Duration, host: &String, port: u16) {
    println!("{{\n\
        \x20\"status\": \"ok\",\n\
        \x20\"host\": \"{}\",\n\
        \x20\"port\": {},\n\
        \x20\"durationMs\": {}\n\
    }}", host, port, duration.as_millis())
}

fn print_json_error(message: String, host: &String, port: u16) {
    println!("{{\n\
        \x20\"status\": \"error\",\n\
        \x20\"host\": \"{}\",\n\
        \x20\"port\": {},\n\
        \x20\"errorMessage\": \"{}\"\n\
    }}", host, port, message);
}

// return microseconds from the monotonic clock dropping all bits above 32.
fn timestamp(start: &Instant) -> u32 {
    start.elapsed().as_micros() as u32
}