use pallas_network::miniprotocols::chainsync::HeaderContent;
use pallas_traverse::MultiEraHeader;

pub fn header_hash(header: &HeaderContent) -> Result<Vec<u8>, pallas_traverse::Error> {
    let subtag = header.byron_prefix.map(|(tag, _)| tag);
    let multi = MultiEraHeader::decode(header.variant, subtag, &header.cbor)?;
    Ok(multi.hash().to_vec())
}

pub fn extract_slot(header: &HeaderContent) -> Result<u64, pallas_traverse::Error> {
    let subtag = header.byron_prefix.map(|(tag, _)| tag);
    let multi = MultiEraHeader::decode(header.variant, subtag, &header.cbor)?;
    Ok(multi.slot())
}
