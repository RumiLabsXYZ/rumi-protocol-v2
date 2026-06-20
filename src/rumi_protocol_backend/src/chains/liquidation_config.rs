//! Per-chain, operator-settable liquidation config (spec 8, Tier B).
//!
//! Chain-agnostic by construction: the liquidation ENGINE is generic; the only
//! per-chain knobs are the DEX wiring (which UniswapV2-family pool to sell the
//! seized collateral into) + the risk knobs. A new EVM chain plugs in as a config
//! ROW (`set_chain_liquidation_config`), not as code. The same DEX family (any
//! UniswapV2 fork, e.g. Swappi on Conflux eSpace) reuses `DexKind::UniswapV2`
//! verbatim.
//!
//! Versioned-snapshot pattern (see spec Section 3): the active shape is
//! `ChainLiquidationConfigV1`. Adding a field = bump to V2 (carry every prior
//! field verbatim, decorate the new one with `#[serde(default)]`). The enclosing
//! `MultiChainStateV6.chain_liquidation_configs` map is `#[serde(default)]`, so a
//! pre-config snapshot decodes with an empty map (state-wipe safe). Inert until
//! Increment 2+ reads it; Increment 1 only ships the getter/setter scaffolding.

use candid::{CandidType, Deserialize};
use serde::Serialize;

/// The DEX family used to liquidate a chain's collateral. Only the UniswapV2
/// constant-product family ships first (Swappi/Conflux); the engine's swap
/// encoder + quote reader are keyed off this so a new family is a new arm, not a
/// rewrite.
#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DexKind {
    /// UniswapV2 constant-product AMM (and forks: Swappi, etc.).
    UniswapV2,
}

/// Per-chain liquidation config (operator-set, dev-gated). Holds the DEX wiring
/// + the two risk knobs the bot path needs. Addresses are chain-native hex
/// strings (validated by the engine's address validator at swap-build time, not
/// here). `enabled` is the per-chain kill switch: even with the whole chains rail
/// dev-gated, an operator must explicitly flip this on before any liquidation
/// swap can run for the chain.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ChainLiquidationConfigV1 {
    /// Which DEX family to route the collateral->stable swap through.
    pub dex: DexKind,
    /// The DEX router contract (the swap entrypoint, e.g. Swappi router). Distinct
    /// from `factory` — the spec calls out that the liquidity facts only give the
    /// factory, and the router address is a required separate input.
    pub router: String,
    /// The DEX factory contract (used for the set-time pair sanity check in a
    /// later increment).
    pub factory: String,
    /// The collateral/stable pair (pool) contract the swap reads reserves from.
    pub pair: String,
    /// The collateral token sold (wrapped native gas asset, e.g. WCFX).
    pub collateral_token: String,
    /// The settle-stable token received and held as reserve (e.g. USDC).
    pub settle_stable_token: String,
    /// Max acceptable slippage on the swap, in basis points (<= 10_000).
    pub slippage_cap_bps: u16,
    /// The collateral ratio (e4: 15_500 == 155.00%) the partial liquidation sizes
    /// the vault back up to.
    pub restore_target_cr_e4: u64,
    /// Per-chain kill switch. `false` => no liquidation swap runs for this chain
    /// even if the rest of the rail is enabled.
    pub enabled: bool,
}

/// Reasons `set_chain_liquidation_config` rejects a config (no state mutation on
/// any error).
#[derive(Debug, PartialEq, Eq)]
pub enum LiquidationConfigError {
    /// `slippage_cap_bps` exceeds 100% (10_000 bps).
    SlippageCapTooHigh { slippage_cap_bps: u16 },
    /// `restore_target_cr_e4` is not above 100% (a target at/below par cannot
    /// restore a vault).
    RestoreTargetTooLow { restore_target_cr_e4: u64 },
    /// An `enabled` config left a required address empty (the swap could never
    /// build). Disabled configs may carry placeholder/empty addresses.
    MissingAddress(&'static str),
}

impl ChainLiquidationConfigV1 {
    /// Validate operator-supplied invariants before persisting. Disabled configs
    /// are allowed to be partially filled (an operator stages the wiring, then
    /// flips `enabled`); an enabled config must have every address present.
    pub fn validate(&self) -> Result<(), LiquidationConfigError> {
        if self.slippage_cap_bps > 10_000 {
            return Err(LiquidationConfigError::SlippageCapTooHigh {
                slippage_cap_bps: self.slippage_cap_bps,
            });
        }
        if self.restore_target_cr_e4 <= 10_000 {
            return Err(LiquidationConfigError::RestoreTargetTooLow {
                restore_target_cr_e4: self.restore_target_cr_e4,
            });
        }
        if self.enabled {
            if self.router.is_empty() {
                return Err(LiquidationConfigError::MissingAddress("router"));
            }
            if self.factory.is_empty() {
                return Err(LiquidationConfigError::MissingAddress("factory"));
            }
            if self.pair.is_empty() {
                return Err(LiquidationConfigError::MissingAddress("pair"));
            }
            if self.collateral_token.is_empty() {
                return Err(LiquidationConfigError::MissingAddress("collateral_token"));
            }
            if self.settle_stable_token.is_empty() {
                return Err(LiquidationConfigError::MissingAddress("settle_stable_token"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_cfg() -> ChainLiquidationConfigV1 {
        ChainLiquidationConfigV1 {
            dex: DexKind::UniswapV2,
            router: "0xrouter".into(),
            factory: "0xfactory".into(),
            pair: "0xpair".into(),
            collateral_token: "0xwcfx".into(),
            settle_stable_token: "0xusdc".into(),
            slippage_cap_bps: 250,
            restore_target_cr_e4: 15_500,
            enabled: true,
        }
    }

    #[test]
    fn valid_enabled_config_passes() {
        assert_eq!(enabled_cfg().validate(), Ok(()));
    }

    #[test]
    fn slippage_above_100pct_rejected() {
        let mut c = enabled_cfg();
        c.slippage_cap_bps = 10_001;
        assert_eq!(
            c.validate(),
            Err(LiquidationConfigError::SlippageCapTooHigh { slippage_cap_bps: 10_001 })
        );
    }

    #[test]
    fn restore_target_at_or_below_par_rejected() {
        let mut c = enabled_cfg();
        c.restore_target_cr_e4 = 10_000;
        assert_eq!(
            c.validate(),
            Err(LiquidationConfigError::RestoreTargetTooLow { restore_target_cr_e4: 10_000 })
        );
    }

    #[test]
    fn enabled_config_with_empty_address_rejected() {
        let mut c = enabled_cfg();
        c.pair = String::new();
        assert_eq!(c.validate(), Err(LiquidationConfigError::MissingAddress("pair")));
    }

    #[test]
    fn disabled_config_may_have_empty_addresses() {
        // An operator can stage a config (addresses TBD) while it is disabled.
        let c = ChainLiquidationConfigV1 {
            dex: DexKind::UniswapV2,
            router: String::new(),
            factory: String::new(),
            pair: String::new(),
            collateral_token: String::new(),
            settle_stable_token: String::new(),
            slippage_cap_bps: 250,
            restore_target_cr_e4: 15_500,
            enabled: false,
        };
        assert_eq!(c.validate(), Ok(()));
    }
}
