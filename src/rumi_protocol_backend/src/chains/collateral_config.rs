//! Per-chain collateral risk parameters, mirroring the ICP protocol's
//! `CollateralConfig` VALUES. Compile-time (the chain-rail analogue of
//! `chains::evm::EvmChainConfig`); runtime admin-settability (ICP's persisted,
//! tunable config) is a deliberate deferral to the mainnet-hardening phase.
//!
//! Scale: `_e4` fields are ratios x 10^-4 (13_300 == 1.3300 == 133%); `_bps`
//! fields are fractions x 10^-4 (30 == 0.0030 == 0.30%). Values reflect ICP's
//! live dashboard (reconfirm against `get_protocol_config()` before mainnet).
//!
//! Currently the open/borrow/withdraw rail consumes only `min_cr_e4`. The other
//! fields are forward-looking: `interest_apr_bps` is consumed by interest
//! accrual (later task), and the liquidation/fee fields by the deferred
//! liquidation + fee work. They are carried here so the params live in one place.

use crate::chains::config::ChainId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainCollateralConfig {
    pub min_cr_e4: u64,
    pub borrow_threshold_e4: u64,
    pub liquidation_penalty_bps: u64,
    pub borrowing_fee_bps: u64,
    pub interest_apr_bps: u64,
    pub min_vault_debt_e8s: u128,
    pub recovery_target_cr_e4: u64,
    pub debt_ceiling_e8s: Option<u128>,
}

/// ICP-mirrored defaults (the live dashboard values).
const ICP_MIRROR: ChainCollateralConfig = ChainCollateralConfig {
    min_cr_e4: 13_300,
    borrow_threshold_e4: 15_000,
    liquidation_penalty_bps: 1_200,
    borrowing_fee_bps: 30,
    interest_apr_bps: 200,
    min_vault_debt_e8s: 10_000_000,
    recovery_target_cr_e4: 15_500,
    debt_ceiling_e8s: None,
};

/// Compile-time per-chain collateral config. `None` for unknown chains.
pub fn chain_collateral_config(chain: ChainId) -> Option<ChainCollateralConfig> {
    match chain.0 {
        // Conflux eSpace testnet: full ICP mirror.
        71 => Some(ICP_MIRROR),
        // Monad testnet: preserve its historical 130% open threshold
        // (behavior-preserving); other params ICP-mirrored but inert for Monad.
        10143 => Some(ChainCollateralConfig {
            min_cr_e4: 13_000,
            ..ICP_MIRROR
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chains::config::ChainId;

    #[test]
    fn conflux_mirrors_icp() {
        let c = chain_collateral_config(ChainId(71)).expect("conflux known");
        assert_eq!(c.min_cr_e4, 13_300);
        assert_eq!(c.borrow_threshold_e4, 15_000);
        assert_eq!(c.liquidation_penalty_bps, 1_200);
        assert_eq!(c.borrowing_fee_bps, 30);
        assert_eq!(c.interest_apr_bps, 200);
        assert_eq!(c.min_vault_debt_e8s, 10_000_000);
        assert_eq!(c.recovery_target_cr_e4, 15_500);
        assert_eq!(c.debt_ceiling_e8s, None);
    }

    #[test]
    fn monad_min_cr_preserved_at_13000() {
        // Monad keeps its historical 130% open threshold (behavior-preserving);
        // the other params are ICP-mirrored but inert until fees/liquidation land.
        let c = chain_collateral_config(ChainId(10143)).expect("monad known");
        assert_eq!(c.min_cr_e4, 13_000);
    }

    #[test]
    fn unknown_chain_is_none() {
        assert!(chain_collateral_config(ChainId(999)).is_none());
    }
}
