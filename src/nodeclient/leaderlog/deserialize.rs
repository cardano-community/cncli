use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_cbor::{de, Value};

pub(crate) fn cbor_hex<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let cbor: String = Deserialize::deserialize(d)?;
    let cbor_vec = hex::decode(cbor).map_err(D::Error::custom)?;
    let value: Value = de::from_slice(&cbor_vec).map_err(D::Error::custom)?;
    match value {
        Value::Bytes(key) => Ok(key),
        _ => Err(D::Error::custom("Expected CBOR byte string for VRF key")),
    }
}
