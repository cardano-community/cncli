use std::collections::BTreeMap;

use log::debug;
use serde_cbor::{de, ser, Value};

use crate::nodeclient::protocols::{Agency, Protocol};

#[derive(Debug)]
pub enum State {
    Propose,
    Confirm,
    Done,
}

pub struct HandshakeProtocol {
    pub(crate) state: State,
    pub(crate) network_magic: u32,
    pub(crate) result: Option<Result<String, String>>,
}

impl Default for HandshakeProtocol {
    fn default() -> Self {
        HandshakeProtocol {
            state: State::Propose,
            network_magic: 764824073,
            result: None,
        }
    }
}

impl HandshakeProtocol {
    // Serialize cbor for MsgProposeVersions
    //
    // Create the byte representation of MsgProposeVersions for sending to the server
    fn msg_propose_versions(&self, network_magic: u32) -> Vec<u8> {
        let mut payload_map: BTreeMap<Value, Value> = BTreeMap::new();
        // protocol version 3 mapped to the network_magic value
        payload_map.insert(Value::Integer(0x03), Value::Integer(network_magic as i128));

        let message = Value::Array(vec![
            Value::Integer(0), // message_id
            Value::Map(payload_map)
        ]);

        ser::to_vec_packed(&message).unwrap()
    }

    // Search through the cbor values until we find a Text value.
    fn find_error_message(&self, cbor_value: &Value) -> Result<String, ()> {
        match cbor_value {
            Value::Text(cbor_text) => {
                return Ok(cbor_text.to_owned());
            }
            Value::Array(cbor_array) => {
                for value in cbor_array {
                    let result = self.find_error_message(value);
                    if result.is_ok() {
                        return result;
                    }
                }
            }
            _ => {}
        }
        return Err(());
    }
}

impl Protocol for HandshakeProtocol {
    fn protocol_id(&self) -> u16 {
        return 0x0000u16;
    }

    fn get_agency(&self) -> Agency {
        return match self.state {
            State::Propose => { Agency::Client }
            State::Confirm => { Agency::Server }
            State::Done => { Agency::None }
        };
    }

    fn get_state(&self) -> String {
        format!("{:?}", self.state)
    }

    fn send_data(&mut self) -> Option<Vec<u8>> {
        return match self.state {
            State::Propose => {
                debug!("HandshakeProtocol::State::Propose");
                let payload = self.msg_propose_versions(self.network_magic);
                self.state = State::Confirm;
                Some(payload)
            }
            State::Confirm => {
                debug!("HandshakeProtocol::State::Confirm");
                None
            }
            State::Done => {
                debug!("HandshakeProtocol::State::Done");
                None
            }
        };
    }

    fn receive_data(&mut self, data: Vec<u8>) {
        if data.len() != 8 {
            // some payload error
            let cbor_value: Value = de::from_slice(&data[..]).unwrap();
            match self.find_error_message(&cbor_value) {
                Ok(error_message) => {
                    self.result = Some(Err(error_message));
                }
                Err(_) => {
                    self.result = Some(Err(format!("Unable to parse payload error! {}", hex::encode(data))));
                }
            }
        } else {
            self.result = Some(Ok(hex::encode(data)));
        }
        debug!("HandshakeProtocol::State::Done");
        self.state = State::Done
    }
}