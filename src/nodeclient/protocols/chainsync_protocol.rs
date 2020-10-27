use serde_cbor::{de, ser, Value};

use crate::nodeclient::protocols::{Agency, Protocol};
use crate::nodeclient::protocols::chainsync_protocol::msg_roll_forward::parse_msg_roll_forward;

mod msg_roll_forward;

pub enum State {
    Idle,
    Intersect,
    CanAwait,
    MustReply,
    Done,
}

pub struct ChainSyncProtocol {
    pub(crate) state: State,
    pub(crate) result: Option<Result<String, String>>,
    pub(crate) is_intersect_found: bool,
}

impl ChainSyncProtocol {
    fn msg_find_intersect(&self, chain_blocks: Vec<(u64, Vec<u8>)>) -> Vec<u8> {
        let msg: Value = Value::Array(
            vec![
                Value::Integer(4), // message_id
                Value::Array(chain_blocks.iter().map(|(slot, &hash)| Value::Array(vec![Value::Integer(*slot as i128), Value::Bytes(hash)])).collect())
            ]
        );

        ser::to_vec_packed(&msg).unwrap()
    }

    fn msg_request_next(&self) -> Vec<u8> {
        // we just send an array containing the message_id for this one.
        ser::to_vec_packed(&Value::Array(vec![Value::Integer(0)])).unwrap()
    }
}

impl Protocol for ChainSyncProtocol {
    fn protocol_id(&self) -> u16 {
        return 0x0002u16;
    }

    fn get_agency(&self) -> Agency {
        return match self.state {
            State::Idle => { Agency::Client }
            State::Intersect => { Agency::Server }
            State::CanAwait => { Agency::Server }
            State::MustReply => { Agency::Server }
            State::Done => { Agency::None }
        };
    }

    fn send_data(&mut self) -> Option<Vec<u8>> {
        return match self.state {
            State::Idle => {
                if !self.is_intersect_found {
                    // request an intersect with the server so we know where to start syncing blocks
                    let chain_blocks = vec![
                        // Last byron block of mainnet
                        (4492799, hex::decode("f8084c61b6a238acec985b59310b6ecec49c0ab8352249afd7268da5cff2a457").unwrap()),
                        // Last byron block of testnet
                        (1598399, hex::decode("7e16781b40ebf8b6da18f7b5e8ade855d6738095ef2f1c58c77e88b6e45997a4").unwrap()),
                    ];
                    let payload = self.msg_find_intersect(chain_blocks);
                    self.state = State::Intersect;
                    Some(payload)
                } else {
                    // request the next block from the server.
                    let payload = self.msg_request_next();
                    self.state = State::CanAwait;
                    Some(payload)
                }
            }
            State::Intersect => {
                println!("ChainSyncProtocol::State::Intersect");
                None
            }
            State::CanAwait => {
                println!("ChainSyncProtocol::State::CanAwait");
                None
            }
            State::MustReply => {
                println!("ChainSyncProtocol::State::MustReply");
                None
            }
            State::Done => {
                println!("ChainSyncProtocol::State::Done");
                None
            }
        };
    }

    fn receive_data(&mut self, data: Vec<u8>) {
        //msgRequestNext         = [0]
        //msgAwaitReply          = [1]
        //msgRollForward         = [2, wrappedHeader, tip]
        //msgRollBackward        = [3, point, tip]
        //msgFindIntersect       = [4, points]
        //msgIntersectFound      = [5, point, tip]
        //msgIntersectNotFound   = [6, tip]
        //chainSyncMsgDone       = [7]

        let cbor_value: Value = de::from_slice(&data[..]).unwrap();
        match cbor_value {
            Value::Array(cbor_array) => {
                match cbor_array[0] {
                    Value::Integer(message_id) => {
                        match message_id {
                            1 => {
                                // Server wants us to wait a bit until it gets a new block
                                self.state = State::MustReply;
                            }
                            2 => {
                                // MsgRollForward
                                let (msg_roll_forward, tip) = parse_msg_roll_forward(cbor_array);

                                if msg_roll_forward.slot_number % 1000 == 0 {
                                    println!("ChainSync: slot {} of {}.", msg_roll_forward.slot_number, tip.slot_number);
                                }
                                // self.state = State::Idle;

                                // testing only
                                self.state = State::Done;
                                self.result = Some(Ok(String::from("Done")))
                            }
                            3 => {
                                // MsgRollBackward
                                println!("ChainSync: rollback to point: {:?}, tip: {:?}", cbor_array[1], cbor_array[2]);
                                self.state = State::Idle;
                            }
                            5 => {
                                println!("MsgIntersectFound: {:?}", cbor_value);
                                self.is_intersect_found = true;
                                self.state = State::Idle;
                            }
                            6 => {
                                println!("MsgIntersectNotFound: {:?}", cbor_value);
                                self.is_intersect_found = true; // should start syncing at first byron block. will probably crash later, but oh well.
                                self.state = State::Idle;
                            }
                            7 => {
                                println!("MsgDone: {:?}", cbor_value);
                                self.state = State::Done;
                                self.result = Some(Ok(String::from("Done")))
                            }
                            _ => {
                                println!("Got unexpected message_id: {}", message_id);
                            }
                        }
                    }
                    _ => {
                        println!("Unexpected cbor!")
                    }
                }
            }
            _ => {
                println!("Unexpected cbor!")
            }
        }
    }
}
