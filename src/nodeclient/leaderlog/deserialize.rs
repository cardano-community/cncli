use rug::Float;
use serde::{Deserialize, Deserializer};
use serde_cbor::{de, Value};
use serde_json::Number;

pub(crate) fn fixed_number<'de, D: Deserializer<'de>>(d: D) -> Result<Float, D::Error> {
    let n: Number = Deserialize::deserialize(d)?;
    Ok(Float::with_val(120, Float::parse(&*n.to_string()).unwrap()))
}

pub(crate) fn cbor_hex<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let cbor: String = Deserialize::deserialize(d)?;
    let cbor_vec = hex::decode(cbor).unwrap();
    let value: Value = de::from_slice(&*cbor_vec).unwrap();
    match value {
        Value::Bytes(key) => {
            Ok(key)
        }
        _ => {
            panic!("Invalid cbor hex!")
        }
    }
}
