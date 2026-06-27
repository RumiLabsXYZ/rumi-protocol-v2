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
/// + the two risk knobs the bot path needs. DEX/token addresses are EVM
/// 0x-prefixed 20-byte hex strings validated at setter time. `enabled` is the
/// per-chain kill switch: even with the whole chains rail dev-gated, an operator
/// must explicitly flip this on before any liquidation swap can run for the chain.
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
    /// Depth-bound cap: the max USD value (e8s) of collateral sold in a single
    /// liquidation swap (spec §4.7). ADVISORY at sizing time (finding #3): the
    /// submit-time live-reserves min-out (Increment 3) is the real safety. Start
    /// ~$2k for Conflux; re-tune as pool depth moves.
    #[serde(default)]
    pub max_swap_value_e8s: u128,
    /// Staleness ceiling (ns) for the manual collateral price (spec §4.3). A
    /// price older than this fails the fresh-price gate and DEFERS liquidation
    /// for this chain (fail-closed). Recommended ~2-3x the off-chain monitor's
    /// push interval (start ~30 min).
    #[serde(default)]
    pub max_price_age_ns: u64,
    /// Pool-vs-oracle divergence ceiling (bps) for the submit-time cross-check
    /// (spec §4.8): if the pool prices collateral more than this below oracle, the
    /// pool is thin/manipulated -> do NOT swap, escalate. Also half of the #16
    /// penalty-cushion invariant (`penalty > slippage + divergence`).
    #[serde(default)]
    pub max_dex_oracle_divergence_bps: u32,
    /// The DEX swap fee in bps (Swappi UniswapV2 = 25 = 0.25%); used by the
    /// constant-product min-out (spec §4.8). CONFIRM against the live router.
    #[serde(default)]
    pub fee_bps: u16,
    /// Decimals of `settle_stable_token` (18 on eSpace; a future chain may be 6).
    /// Used at the realized-USD <-> e8s boundary. NOT a constant (spec §8).
    #[serde(default)]
    pub settle_stable_decimals: u8,
    /// On-chain swap deadline horizon (secs, ~180). The tx deadline = now + this.
    #[serde(default)]
    pub deadline_secs: u64,
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
    /// A non-empty address is not an EVM `0x` + 40 hex digit address.
    InvalidAddress { field: &'static str, address: String },
    /// An `enabled` config left the depth cap at 0 (spec §4.7): no swap could be
    /// sized. Disabled configs may carry 0.
    ZeroDepthCap,
    /// An `enabled` config left the price-staleness ceiling at 0 (spec §4.3):
    /// every fresh-price check would fail closed. Disabled configs may carry 0.
    ZeroPriceAge,
    /// An `enabled` config has `settle_stable_decimals` of 0 or > 36 (spec §8): the
    /// realized-USD <-> e8s conversion would be wrong. Disabled configs may carry 0.
    BadSettleDecimals,
    /// An `enabled` config left the swap `deadline_secs` at 0 (spec §4.8): a swap
    /// with a zero deadline reverts immediately. Disabled configs may carry 0.
    ZeroDeadline,
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
        validate_evm_address_field("router", &self.router, self.enabled)?;
        validate_evm_address_field("factory", &self.factory, self.enabled)?;
        validate_evm_address_field("pair", &self.pair, self.enabled)?;
        validate_evm_address_field("collateral_token", &self.collateral_token, self.enabled)?;
        validate_evm_address_field("settle_stable_token", &self.settle_stable_token, self.enabled)?;
        if self.enabled {
            if self.max_swap_value_e8s == 0 {
                return Err(LiquidationConfigError::ZeroDepthCap);
            }
            if self.max_price_age_ns == 0 {
                return Err(LiquidationConfigError::ZeroPriceAge);
            }
            if self.settle_stable_decimals == 0 || self.settle_stable_decimals > 36 {
                return Err(LiquidationConfigError::BadSettleDecimals);
            }
            if self.deadline_secs == 0 {
                return Err(LiquidationConfigError::ZeroDeadline);
            }
        }
        Ok(())
    }
}

fn validate_evm_address_field(
    field: &'static str,
    address: &str,
    required: bool,
) -> Result<(), LiquidationConfigError> {
    if address.is_empty() {
        return if required {
            Err(LiquidationConfigError::MissingAddress(field))
        } else {
            Ok(())
        };
    }
    canonical_evm_address(address).map(|_| ()).map_err(|_| LiquidationConfigError::InvalidAddress {
        field,
        address: address.to_string(),
    })
}

/// Normalize an EVM address for equality checks after a value has passed the same
/// setter-time validation as liquidation config addresses.
pub fn canonical_evm_address(address: &str) -> Result<String, LiquidationConfigError> {
    let hex = address
        .strip_prefix("0x")
        .or_else(|| address.strip_prefix("0X"))
        .ok_or_else(|| LiquidationConfigError::InvalidAddress {
            field: "address",
            address: address.to_string(),
        })?;
    if hex.len() != 40 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(LiquidationConfigError::InvalidAddress {
            field: "address",
            address: address.to_string(),
        });
    }
    if hex.bytes().all(|b| b == b'0') {
        return Err(LiquidationConfigError::InvalidAddress {
            field: "address",
            address: address.to_string(),
        });
    }
    Ok(format!("0x{}", hex.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_cfg() -> ChainLiquidationConfigV1 {
        ChainLiquidationConfigV1 {
            dex: DexKind::UniswapV2,
            router: "0x1111111111111111111111111111111111111111".into(),
            factory: "0x2222222222222222222222222222222222222222".into(),
            pair: "0x3333333333333333333333333333333333333333".into(),
            collateral_token: "0x4444444444444444444444444444444444444444".into(),
            settle_stable_token: "0x5555555555555555555555555555555555555555".into(),
            slippage_cap_bps: 250,
            restore_target_cr_e4: 15_500,
            enabled: true,
            max_swap_value_e8s: 2_000 * 100_000_000,
            max_price_age_ns: 1_800_000_000_000,
            max_dex_oracle_divergence_bps: 500,
            fee_bps: 25,
            settle_stable_decimals: 18,
            deadline_secs: 180,
        }
    }

    #[test]
    fn config_carries_inc3_swap_fields() {
        let c = enabled_cfg();
        assert!(c.max_dex_oracle_divergence_bps > 0);
        assert!(c.fee_bps > 0);
        assert_eq!(c.settle_stable_decimals, 18);
        assert!(c.deadline_secs > 0);
    }

    #[test]
    fn enabled_config_rejects_zero_settle_decimals() {
        let mut c = enabled_cfg();
        c.settle_stable_decimals = 0;
        assert_eq!(c.validate(), Err(LiquidationConfigError::BadSettleDecimals));
    }

    #[test]
    fn enabled_config_rejects_zero_deadline() {
        let mut c = enabled_cfg();
        c.deadline_secs = 0;
        assert_eq!(c.validate(), Err(LiquidationConfigError::ZeroDeadline));
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
    fn enabled_config_with_malformed_address_rejected() {
        let mut c = enabled_cfg();
        c.router = "router-no-0x".into();
        assert_eq!(
            c.validate(),
            Err(LiquidationConfigError::InvalidAddress {
                field: "router",
                address: "router-no-0x".into()
            })
        );
    }

    #[test]
    fn disabled_config_rejects_malformed_non_empty_address() {
        let mut c = enabled_cfg();
        c.enabled = false;
        c.max_swap_value_e8s = 0;
        c.max_price_age_ns = 0;
        c.settle_stable_decimals = 0;
        c.deadline_secs = 0;
        c.factory = "0xnothex".into();
        assert_eq!(
            c.validate(),
            Err(LiquidationConfigError::InvalidAddress {
                field: "factory",
                address: "0xnothex".into()
            })
        );
    }

    #[test]
    fn canonical_evm_address_lowercases_valid_input() {
        assert_eq!(
            canonical_evm_address("0XABCDEFabcdefABCDEFabcdefABCDEFabcdefABCD").unwrap(),
            "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd"
        );
        assert!(canonical_evm_address("0x1234").is_err());
        assert!(canonical_evm_address("0x0000000000000000000000000000000000000000").is_err());
    }

    #[test]
    fn enabled_config_rejects_zero_addresses_for_all_evm_fields() {
        let zero = "0x0000000000000000000000000000000000000000".to_string();
        for field in ["router", "factory", "pair", "collateral_token", "settle_stable_token"] {
            let mut c = enabled_cfg();
            match field {
                "router" => c.router = zero.clone(),
                "factory" => c.factory = zero.clone(),
                "pair" => c.pair = zero.clone(),
                "collateral_token" => c.collateral_token = zero.clone(),
                "settle_stable_token" => c.settle_stable_token = zero.clone(),
                _ => unreachable!(),
            }
            assert_eq!(
                c.validate(),
                Err(LiquidationConfigError::InvalidAddress {
                    field,
                    address: zero.clone()
                }),
                "{field} zero address must reject"
            );
        }
    }

    #[test]
    fn config_carries_depth_cap_and_staleness_ceiling() {
        let cfg = enabled_cfg();
        assert!(cfg.max_swap_value_e8s > 0);
        assert!(cfg.max_price_age_ns > 0);
    }

    #[test]
    fn enabled_config_rejects_zero_depth_cap() {
        let mut cfg = enabled_cfg();
        cfg.max_swap_value_e8s = 0;
        assert_eq!(cfg.validate(), Err(LiquidationConfigError::ZeroDepthCap));
    }

    #[test]
    fn enabled_config_rejects_zero_price_age() {
        let mut cfg = enabled_cfg();
        cfg.max_price_age_ns = 0;
        assert_eq!(cfg.validate(), Err(LiquidationConfigError::ZeroPriceAge));
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
            max_swap_value_e8s: 0,
            max_price_age_ns: 0,
            max_dex_oracle_divergence_bps: 0,
            fee_bps: 0,
            settle_stable_decimals: 0,
            deadline_secs: 0,
        };
        assert_eq!(c.validate(), Ok(()));
    }
}
