use blake2b_simd::Params;
use log::error;
use serde_cbor::{de, Value};

pub struct MsgRollForward {
    pub block_number: i64,
    pub slot_number: i64,
    pub hash: Vec<u8>,
    pub prev_hash: Vec<u8>,
    pub node_vkey: Vec<u8>,
    pub node_vrf_vkey: Vec<u8>,
    pub eta_vrf_0: Vec<u8>,
    pub eta_vrf_1: Vec<u8>,
    pub leader_vrf_0: Vec<u8>,
    pub leader_vrf_1: Vec<u8>,
    pub block_size: i64,
    pub block_body_hash: Vec<u8>,
    pub pool_opcert: Vec<u8>,
    pub unknown_0: i64,
    pub unknown_1: i64,
    pub unknown_2: Vec<u8>,
    pub protocol_major_version: i64,
    pub protocol_minor_version: i64,
}

#[derive(Debug)]
pub struct Tip {
    pub block_number: i64,
    pub slot_number: i64,
    pub hash: Vec<u8>,
}

trait UnwrapValue {
    fn integer(&self) -> i128;
    fn bytes(&self) -> Vec<u8>;
}

impl UnwrapValue for Value {
    fn integer(&self) -> i128 {
        match self {
            Value::Integer(integer_value) => { *integer_value }
            _ => { panic!("not an integer!") }
        }
    }

    fn bytes(&self) -> Vec<u8> {
        match self {
            Value::Bytes(bytes_vec) => { bytes_vec.clone() }
            _ => { panic!("not a byte array!") }
        }
    }
}

pub fn parse_msg_roll_forward(cbor_array: Vec<Value>) -> (MsgRollForward, Tip) {
    let mut msg_roll_forward = MsgRollForward {
        block_number: 0,
        slot_number: 0,
        hash: vec![],
        prev_hash: vec![],
        node_vkey: vec![],
        node_vrf_vkey: vec![],
        eta_vrf_0: vec![],
        eta_vrf_1: vec![],
        leader_vrf_0: vec![],
        leader_vrf_1: vec![],
        block_size: 0,
        block_body_hash: vec![],
        pool_opcert: vec![],
        unknown_0: 0,
        unknown_1: 0,
        unknown_2: vec![],
        protocol_major_version: 0,
        protocol_minor_version: 0,
    };
    let mut tip = Tip {
        block_number: 0,
        slot_number: 0,
        hash: vec![],
    };

    match &cbor_array[1] {
        Value::Array(header_array) => {
            match &header_array[1] {
                Value::Bytes(wrapped_block_header_bytes) => {
                    // calculate the block hash
                    let hash = Params::new().hash_length(32).to_state().update(&*wrapped_block_header_bytes).finalize();
                    msg_roll_forward.hash = hash.as_bytes().to_owned();

                    let block_header: Value = de::from_slice(&wrapped_block_header_bytes[..]).unwrap();
                    match block_header {
                        Value::Array(block_header_array) => {
                            match &block_header_array[0] {
                                Value::Array(block_header_array_inner) => {
                                    msg_roll_forward.block_number = block_header_array_inner[0].integer() as i64;
                                    msg_roll_forward.slot_number = block_header_array_inner[1].integer() as i64;
                                    msg_roll_forward.prev_hash.append(&mut block_header_array_inner[2].bytes());
                                    msg_roll_forward.node_vkey.append(&mut block_header_array_inner[3].bytes());
                                    msg_roll_forward.node_vrf_vkey.append(&mut block_header_array_inner[4].bytes());
                                    match &block_header_array_inner[5] {
                                        Value::Array(nonce_array) => {
                                            msg_roll_forward.eta_vrf_0.append(&mut nonce_array[0].bytes());
                                            msg_roll_forward.eta_vrf_1.append(&mut nonce_array[1].bytes());
                                        }
                                        _ => { error!("invalid cbor!") }
                                    }
                                    match &block_header_array_inner[6] {
                                        Value::Array(leader_array) => {
                                            msg_roll_forward.leader_vrf_0.append(&mut leader_array[0].bytes());
                                            msg_roll_forward.leader_vrf_1.append(&mut leader_array[1].bytes());
                                        }
                                        _ => { error!("invalid cbor!") }
                                    }
                                    msg_roll_forward.block_size = block_header_array_inner[7].integer() as i64;
                                    msg_roll_forward.block_body_hash.append(&mut block_header_array_inner[8].bytes());
                                    msg_roll_forward.pool_opcert.append(&mut block_header_array_inner[9].bytes());
                                    msg_roll_forward.unknown_0 = block_header_array_inner[10].integer() as i64;
                                    msg_roll_forward.unknown_1 = block_header_array_inner[11].integer() as i64;
                                    msg_roll_forward.unknown_2.append(&mut block_header_array_inner[12].bytes());
                                    msg_roll_forward.protocol_major_version = block_header_array_inner[13].integer() as i64;
                                    msg_roll_forward.protocol_minor_version = block_header_array_inner[14].integer() as i64;
                                }
                                _ => { error!("invalid cbor!") }
                            }
                        }
                        _ => { error!("invalid cbor!") }
                    }
                }
                _ => { error!("invalid cbor!") }
            }
        }
        _ => { error!("invalid cbor!") }
    }

    match &cbor_array[2] {
        Value::Array(tip_array) => {
            match &tip_array[0] {
                Value::Array(tip_info_array) => {
                    tip.slot_number = tip_info_array[0].integer() as i64;
                    tip.hash.append(&mut tip_info_array[1].bytes());
                }
                _ => { error!("invalid cbor!") }
            }
            tip.block_number = tip_array[1].integer() as i64;
        }
        _ => { error!("invalid cbor!") }
    }

    (msg_roll_forward, tip)
}