use serde::{Deserialize, Deserializer};
use serde_cbor::{de, Value};

pub(crate) fn cbor_hex<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let cbor: String = Deserialize::deserialize(d)?;
    let cbor_vec = hex::decode(cbor).unwrap();
    let value: Value = de::from_slice(&*cbor_vec).unwrap();
    match value {
        Value::Bytes(key) => Ok(key),
        _ => {
            panic!("Invalid cbor hex!")
        }
    }
}
