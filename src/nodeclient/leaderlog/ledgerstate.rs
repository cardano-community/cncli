use std::io::Error;

#[derive(Debug)]
pub(crate) struct LedgerInfo {
    pub(crate) sigma: (u64, u64),
    pub(crate) decentralization: f64,
    pub(crate) extra_entropy: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn calculate_ledger_state_sigma_d_and_extra_entropy(
    pool_stake: &u64,
    active_stake: &u64,
    d: &f64,
    extra_entropy: &Option<String>,
) -> Result<LedgerInfo, Error> {
    // We're assuming d=0 at this point if we're using this new cardano-cli stake-snapshot API
    Ok(LedgerInfo {
        sigma: (*pool_stake, *active_stake),
        decentralization: *d,
        extra_entropy: extra_entropy.clone(),
    })
}
