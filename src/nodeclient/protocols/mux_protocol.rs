use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Instant;

use byteorder::{ByteOrder, NetworkEndian};

// Ping a remote cardano-node
//
// Ping connects to a remote cardano-node and runs the handshake protocol
pub fn ping(connect_url: &String) {
    println!("mux_protocol, pinging: {}", connect_url);
    let start_time = Instant::now();
    match TcpStream::connect(connect_url) {
        Ok(mut stream) => {
            let mut handshake = [0u8; 17];
            NetworkEndian::write_u32(&mut handshake[0..4], timestamp(&start_time)); // timestamp
            NetworkEndian::write_u16(&mut handshake[4..6], 0u16); // handshake protocol id
            NetworkEndian::write_u16(&mut handshake[6..8], 9u16); // length of payload
            handshake[8] = 0x82; // begin cbor array of size 2
            handshake[9] = 0x00; // message id for propose versions
            handshake[10] = 0xa1; // begin map (version to network magic)
            handshake[11] = 0x03; // version 3 (map key)
            handshake[12] = 0x1a; // unsigned (map value)
            NetworkEndian::write_u32(&mut handshake[13..], 764824073); // mainnet network magic
            println!("sending: {:?}", hex::encode(handshake));

            // send the message. Expect it to succeed.
            stream.write(&handshake).unwrap();

            let mut response = [0u8; 8]; // read 8 bytes to start with
            match stream.read_exact(&mut response) {
                Ok(_) => {
                    let server_timestamp = NetworkEndian::read_u32(&mut response[0..4]);
                    println!("server_timestamp: {:x}", server_timestamp);
                    let protocol_id = NetworkEndian::read_u16(&mut response[4..6]);
                    println!("protocol_id: {:x}", protocol_id);
                    let payload_length = NetworkEndian::read_u16(&mut response[6..]) as usize;
                    println!("payload_length: {:x}", payload_length);
                    let mut response = vec![0u8; payload_length];
                    match stream.read_exact(&mut response) {
                        Ok(_) => {
                            println!("payload: {}", hex::encode(response))
                        }
                        Err(e) => {
                            println!("Unable to read response payload! {}", e);
                        }
                    }
                }
                Err(e) => {
                    println!("Unable to read response header! {}", e);
                }
            }

            stream.shutdown(Shutdown::Both).expect("shutdown call failed");

            println!("Ping took {} microseconds", start_time.elapsed().as_micros())
            // println!("connected!")
        }
        Err(e) => {
            println!("Failed to connect: {}", e);
        }
    }
}

// return microseconds from the monotonic clock dropping all bits above 32.
fn timestamp(start: &Instant) -> u32 {
    start.elapsed().as_micros() as u32
}