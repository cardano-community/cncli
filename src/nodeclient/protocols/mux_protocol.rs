use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::nodeclient::protocols::handshake_protocol;

// Ping a remote cardano-node
//
// Ping connects to a remote cardano-node and runs the handshake protocol
pub fn ping(host: &String, port: u16, network_magic: u32) {
    // let connect_url = format!("{}:{}", host, port);
    // println!("mux_protocol, pinging: {}", connect_url);
    let start_time = Instant::now();
    let socket_addr: SocketAddr = (host.as_str(), port).to_socket_addrs().unwrap().nth(0).unwrap();
    match TcpStream::connect_timeout(&socket_addr, Duration::from_secs(1)) {
        Ok(stream) => {
            match handshake_protocol::ping(&stream, timestamp(&start_time), network_magic) {
                Ok(_payload) => {
                    stream.shutdown(Shutdown::Both).expect("shutdown call failed");
                    println!("{{\n\
                      \x20\"status\": \"ok\",\n\
                      \x20\"host\": \"{}\",\n\
                      \x20\"port\": {},\n\
                      \x20\"durationMs\": {}\n\
                    }}", host, port, start_time.elapsed().as_millis())
                }
                Err(message) => {
                    stream.shutdown(Shutdown::Both).expect("shutdown call failed");
                    println!("{{\n\
                      \x20\"status\": \"error\",\n\
                      \x20\"host\": \"{}\",\n\
                      \x20\"port\": {},\n\
                      \x20\"errorMessage\": \"{}\"\n\
                    }}", host, port, message)
                }
            }
        }
        Err(e) => {
            println!("{{\n\
                      \x20\"status\": \"error\",\n\
                      \x20\"host\": \"{}\",\n\
                      \x20\"port\": {},\n\
                      \x20\"errorMessage\": \"Failed to connect: {}\"\n\
                    }}", host, port, e)
        }
    }
}

// return microseconds from the monotonic clock dropping all bits above 32.
fn timestamp(start: &Instant) -> u32 {
    start.elapsed().as_micros() as u32
}