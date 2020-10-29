use log::error;
use serde_cbor::Value;

pub fn parse_msg_roll_backward(cbor_array: Vec<Value>) -> u64 {
    let mut slot: u64 = 0;
    match &cbor_array[1] {
        Value::Array(block) => {
            match block[0] {
                Value::Integer(parsed_slot) => { slot = parsed_slot as u64 }
                _ => { error!("invalid cbor"); }
            }
        }
        _ => { error!("invalid cbor"); }
    }

    slot
}
