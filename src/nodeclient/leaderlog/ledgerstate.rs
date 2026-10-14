use super::rational::ExactRatio;
use std::io::Error;

#[derive(Debug)]
pub(crate) struct LedgerInfo {
    pub(crate) sigma: (u64, u64),
    pub(super) decentralization: ExactRatio,
    pub(crate) extra_entropy: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn calculate_ledger_state_sigma_d_and_extra_entropy(
    pool_stake: &u64,
    active_stake: &u64,
    d: &ExactRatio,
    extra_entropy: &Option<String>,
) -> Result<LedgerInfo, Error> {
    Ok(LedgerInfo {
        sigma: (*pool_stake, *active_stake),
        decentralization: *d,
        extra_entropy: extra_entropy.clone(),
    })
}
