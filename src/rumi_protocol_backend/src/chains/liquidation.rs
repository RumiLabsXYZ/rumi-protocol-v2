//! Pure CR + partial-liquidation sizing math for the chains rail (spec §4.2,
//! §4.6, §4.7). All integer, all saturating, no I/O. Composed by
//! `chains::vault::begin_liquidation_in_state` and the detection tick.

/// `bonus_e4 = 10_000 + liquidation_penalty_bps` (spec §4.1.2). A repaid icUSD
/// releases `bonus_e4 / 10_000`x its value in collateral. NEVER hardcode 1.12.
pub fn bonus_e4_from_penalty_bps(liquidation_penalty_bps: u64) -> u64 {
    10_000u64.saturating_add(liquidation_penalty_bps)
}

/// USD value (e8s) of `collateral_native` at `price_e8`. Byte-identical to the
/// numerator math inside `chains::vault::collateral_ratio_e4` so the CR check and
/// the sizing agree.
pub fn collateral_value_e8s(collateral_native: u128, native_decimals: u8, price_e8: u64) -> u128 {
    let native_scale = 10u128.saturating_pow(native_decimals as u32);
    collateral_native.saturating_mul(price_e8 as u128) / native_scale
}

/// Interest-aware debt (spec §4.2): realized debt + reserved interest + freshly
/// accrued interest. The accrual window MUST be byte-identical to
/// `harvest_chain_interest_in_state` (`now_ns - last_interest_accrual_ns`), which
/// the CALLER computes and passes as `elapsed_ns`. Accrual is on `debt_e8s` only
/// (matching harvest), NOT on the pending term.
pub fn effective_debt_e8s(
    debt_e8s: u128,
    pending_interest_mint_e8s: u128,
    apr_bps: u64,
    elapsed_ns: u64,
) -> u128 {
    debt_e8s
        .saturating_add(pending_interest_mint_e8s)
        .saturating_add(crate::chains::interest::accrued_chain_interest_e8s(
            debt_e8s, apr_bps, elapsed_ns,
        ))
}

/// Partial-liquidation repay amount (e8s) that restores CR to approximately
/// `target_cr_e4` (spec §4.6). Port of ICP's `compute_partial_liquidation_cap`,
/// all saturating.
///
/// Rounding: the final division rounds repay DOWN (floor) — the BORROWER-
/// favorable direction (never seize more collateral than needed; the cardinal
/// rule for a liquidation). Because collateral is seized at the `bonus_e4` rate
/// (1.12x) while debt clears 1:1, `dCR/drepay > 0`, so rounding repay down lands
/// the restored CR a HAIR below target (< ~0.02%, dominated by integer
/// truncation; the real undershoot is < 1e-5 of the ratio). That is immaterial:
/// the vault is restored far above the liquidation threshold (133%) and drops out
/// of liquidation. Erring toward under-seizing is the safe, fair choice (it can
/// never over-liquidate). Proven by
/// `sized_repay_partial_restores_near_target_and_above_threshold`.
pub fn sized_repay_e8s(
    eff_debt_e8s: u128,
    collateral_value_e8s: u128,
    target_cr_e4: u64,
    bonus_e4: u64,
) -> u128 {
    let numerator = eff_debt_e8s.saturating_mul(target_cr_e4 as u128) / 10_000;
    if numerator <= collateral_value_e8s {
        return 0; // already at/above target
    }
    if target_cr_e4 <= bonus_e4 {
        return eff_debt_e8s; // denominator <= 0 -> full close
    }
    let deficit = numerator - collateral_value_e8s;
    let denom_e4 = (target_cr_e4 - bonus_e4) as u128; // >= 1 (target > bonus checked above)
    // Floor division (round down), capped at full debt: never over-seize.
    (deficit.saturating_mul(10_000) / denom_e4).min(eff_debt_e8s)
}

/// DEX-depth cap (spec §4.7): the max repay (e8s) coverable by selling at most
/// `max_swap_value_e8s` of collateral at the bonus. ADVISORY ONLY (finding #3):
/// it is a static USD ceiling, NOT a fraction of live reserves. The real
/// guarantee is the submit-time live-reserves min-out gate (Increment 3). An
/// operator MUST re-tune `max_swap_value_e8s` as pool depth moves.
pub fn value_cap_to_repay(max_swap_value_e8s: u128, bonus_e4: u64) -> u128 {
    if bonus_e4 == 0 {
        return 0;
    }
    max_swap_value_e8s.saturating_mul(10_000) / (bonus_e4 as u128)
}

/// Collateral (native base units) to seize for `repay_e8s`, grossed up by the
/// bonus so the swap output covers the debt (spec §4.6). The CALLER caps this at
/// the vault's `collateral_amount_native` (cap-binds -> residual debt falls to
/// SP/manual in a later increment).
pub fn collateral_in_native_for_repay(
    repay_e8s: u128,
    bonus_e4: u64,
    native_decimals: u8,
    price_e8: u64,
) -> u128 {
    if price_e8 == 0 {
        return 0;
    }
    let native_scale = 10u128.saturating_pow(native_decimals as u32);
    let grossed_value_e8s = repay_e8s.saturating_mul(bonus_e4 as u128) / 10_000;
    grossed_value_e8s.saturating_mul(native_scale) / (price_e8 as u128)
}

use crate::chains::config::ChainId;
use crate::chains::multi_chain_state::MultiChainState;

/// Why a chain price was rejected by the fail-closed staleness gate (spec §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceError {
    NoPrice,
    ZeroPrice,
    NoTimestamp, // pre-V5 price with no recorded set-time -> fail closed
    Stale,
}

/// Fail-closed fresh-price gate (spec §4.3, audit F-01). Liquidation is the most
/// dangerous consumer of a stale price (stale-high hides an underwater vault;
/// stale-low liquidates a healthy one), so it MUST verify freshness. A stale
/// price DEFERS chain liquidations only (caller emits ChainLiquidationDeferred +
/// skips); it does NOT latch the protocol to ReadOnly (asymmetric with the ICP
/// oracle breaker, deliberate). The off-chain monitor's uptime is thus a hard
/// production SLO.
pub fn fresh_chain_price_e8(
    state: &MultiChainState,
    chain: ChainId,
    symbol: &str,
    now_ns: u64,
    max_price_age_ns: u64,
) -> Result<u64, PriceError> {
    let (price_e8, set_at_ns) = state.get_manual_price(chain, symbol).ok_or(PriceError::NoPrice)?;
    if price_e8 == 0 {
        return Err(PriceError::ZeroPrice);
    }
    if set_at_ns == 0 {
        return Err(PriceError::NoTimestamp);
    }
    if now_ns.saturating_sub(set_at_ns) > max_price_age_ns {
        return Err(PriceError::Stale);
    }
    Ok(price_e8)
}

/// Bot->SP escalation predicate (spec §10, finding #10). A vault escalates when
/// its bot LiquidationSwap op is terminally failed OR the bot has held it past
/// `bot_timeout_ns`. The SP consumer (Increment 4) ANDs this with a cleared/
/// resolvable marker; here it is the pure timing core (tested now, wired in Inc 4).
pub fn should_escalate_to_sp(
    bot_pending_since_ns: u64,
    now_ns: u64,
    bot_timeout_ns: u64,
    op_terminally_failed: bool,
) -> bool {
    op_terminally_failed || now_ns.saturating_sub(bot_pending_since_ns) >= bot_timeout_ns
}

/// Clear routing state (`bot_pending_chain_vaults`, `sp_attempted_chain_vaults`)
/// for any vault on `chain` that recovered above its liquidation threshold or
/// resolved (Closed / zero-debt), and is no longer mid-liquidation. CR-derivable
/// so an upgrade mid-episode cannot strand a stale entry (findings #26, #36).
/// MUST be called UNCONDITIONALLY every detection tick (even quiet ticks). A
/// missing/stale price is treated as "do not prune on CR" (conservative — keep
/// the routing record); only resolved vaults are pruned in that case.
pub fn prune_recovered_chain_routing_state(
    state: &mut MultiChainState,
    chain: ChainId,
    price_symbol: &str,
    liquidation_threshold_e4: u64,
    now_ns: u64,
) {
    let max_age = state
        .chain_liquidation_configs
        .get(&chain)
        .map(|c| c.max_price_age_ns)
        .unwrap_or(0);
    let price_e8 = fresh_chain_price_e8(state, chain, price_symbol, now_ns, max_age);
    let native_decimals = state
        .chain_configs
        .get(&chain)
        .map(|c| c.chain_native_decimals)
        .unwrap_or(18);
    let apr_bps = crate::chains::collateral_config::chain_collateral_config(chain)
        .map(|c| c.interest_apr_bps)
        .unwrap_or(0);

    let mut recovered: Vec<u64> = Vec::new();
    for (&vid, v) in state.chain_vaults.iter() {
        if v.collateral_chain != chain || v.pending_liquidation.is_some() {
            continue;
        }
        let resolved = matches!(
            v.status,
            crate::chains::monad::chain_vault::ChainVaultStatus::Closed
        ) || v.debt_e8s == 0;
        let recovered_above = match price_e8 {
            Ok(p) => {
                let eff = effective_debt_e8s(
                    v.debt_e8s,
                    v.pending_interest_mint_e8s,
                    apr_bps,
                    now_ns.saturating_sub(v.last_interest_accrual_ns),
                );
                crate::chains::vault::collateral_ratio_e4(v.collateral_amount_native, native_decimals, p, eff)
                    >= liquidation_threshold_e4
            }
            Err(_) => false, // no fresh price -> do not prune on CR
        };
        if resolved || recovered_above {
            recovered.push(vid);
        }
    }
    for vid in recovered {
        state.bot_pending_chain_vaults.remove(&vid);
        state.sp_attempted_chain_vaults.remove(&vid);
    }
}

/// Synchronous per-chain liquidation detection (spec §4.5). Runs the routing-
/// state prune (unconditional, findings #26/#36), then scans Open vaults with
/// the §4.4 exclusions and routes each `CR < liquidation_threshold_e4` vault to
/// `begin_liquidation_in_state` (Bot tier), capped at `max_per_tick`. Returns the
/// number routed. The caller (the observer tick) has already applied the
/// ReadOnly/halt skips; this is a SINGLE synchronous mutation (no `.await`), so
/// the marker set in `begin_liquidation_in_state` is the dedup (finding #37) and
/// routing keys off the marker, never op presence (finding #5).
pub fn detect_and_route_chain_liquidations_in_state(
    state: &mut MultiChainState,
    chain: ChainId,
    price_symbol: &str,
    liquidation_threshold_e4: u64,
    now_ns: u64,
    max_per_tick: usize,
) -> usize {
    // Unconditional prune (findings #26/#36) — even on quiet ticks.
    prune_recovered_chain_routing_state(state, chain, price_symbol, liquidation_threshold_e4, now_ns);

    // Master gate: a config row that is enabled (spec §9).
    let enabled = state
        .chain_liquidation_configs
        .get(&chain)
        .map_or(false, |c| c.enabled);
    if !enabled {
        return 0;
    }

    let native_decimals = state
        .chain_configs
        .get(&chain)
        .map(|c| c.chain_native_decimals)
        .unwrap_or(18);
    let apr_bps = crate::chains::collateral_config::chain_collateral_config(chain)
        .map(|c| c.interest_apr_bps)
        .unwrap_or(0);
    let max_age = state
        .chain_liquidation_configs
        .get(&chain)
        .map(|c| c.max_price_age_ns)
        .unwrap_or(0);
    let price_e8 = match fresh_chain_price_e8(state, chain, price_symbol, now_ns, max_age) {
        Ok(p) => p,
        Err(_) => return 0, // caller emits ChainLiquidationDeferred; defer all this chain
    };

    // Read-only pass to pick candidates (no borrow held across the routing below).
    let mut candidates: Vec<u64> = Vec::new();
    for (&vid, v) in state.chain_vaults.iter() {
        if v.collateral_chain != chain
            || v.status != crate::chains::monad::chain_vault::ChainVaultStatus::Open
            || v.pending_mint_e8s != 0
            || v.pending_interest_mint_e8s != 0
            || v.pending_liquidation.is_some()
            // The bot already failed/gave up on this vault (escalated) — do NOT
            // re-route it to the bot (no retry loop, finding #10). It falls to
            // Tier-3 manual / the Tier-2 SP (Increment 4). The prune clears this
            // once the vault recovers above the liquidation threshold.
            || state.sp_attempted_chain_vaults.contains(&vid)
        {
            continue;
        }
        let eff = effective_debt_e8s(
            v.debt_e8s,
            v.pending_interest_mint_e8s,
            apr_bps,
            now_ns.saturating_sub(v.last_interest_accrual_ns),
        );
        let cr = crate::chains::vault::collateral_ratio_e4(v.collateral_amount_native, native_decimals, price_e8, eff);
        if cr < liquidation_threshold_e4 {
            candidates.push(vid);
            if candidates.len() >= max_per_tick {
                break;
            }
        }
    }

    let mut routed = 0;
    for vid in candidates {
        // begin_liquidation re-validates the full gate; ignore per-vault rejects.
        if crate::chains::vault::begin_liquidation_in_state(
            state,
            vid,
            crate::chains::evm::tecdsa::is_valid_evm_address,
            price_symbol,
            liquidation_threshold_e4,
            now_ns,
        )
        .is_ok()
        {
            routed += 1;
        }
    }
    routed
}

// ─── Increment 3: swap min-out + oracle cross-check (spec §4.8) ───

/// UniswapV2 constant-product output with fee, plus the slippage haircut (spec
/// §4.8). Returns `(expected_out, amount_out_min)` in the OUT token's native base
/// units. Returns `(0, 0)` on any zero/degenerate input (the caller fail-closes:
/// no swap without a satisfiable min-out).
pub fn compute_amount_out_min(
    amount_in: u128,
    reserve_in: u128,
    reserve_out: u128,
    fee_bps: u16,
    slippage_bps: u16,
) -> (u128, u128) {
    use ethnum::U256;
    if amount_in == 0 || reserve_in == 0 || reserve_out == 0 || fee_bps as u32 >= 10_000 {
        return (0, 0);
    }
    // MUST use 256-bit intermediates: `amount_in_with_fee * reserve_out` overflows
    // u128 at 18-decimal token magnitudes (~1e25 * 1e23 = 1e48 >> u128 max 3.4e38).
    // All ops checked -> fail-closed (0,0) on any overflow, never wrap/panic.
    let fee_mult = U256::from(10_000u32 - fee_bps as u32);
    let amount_in_with_fee = match U256::from(amount_in).checked_mul(fee_mult) {
        Some(v) => v,
        None => return (0, 0),
    };
    let numerator = match amount_in_with_fee.checked_mul(U256::from(reserve_out)) {
        Some(v) => v,
        None => return (0, 0),
    };
    let denominator = match U256::from(reserve_in)
        .checked_mul(U256::from(10_000u32))
        .and_then(|x| x.checked_add(amount_in_with_fee))
    {
        Some(d) if d != 0 => d,
        _ => return (0, 0),
    };
    let expected_u256 = numerator / denominator;
    // `expected_out < reserve_out <= u128::MAX` for a real swap, so it fits u128;
    // guard anyway (fail-closed) rather than truncate.
    let expected_out = match u128::try_from(expected_u256) {
        Ok(v) => v,
        Err(_) => return (0, 0),
    };
    let slip_mult = U256::from(10_000u32 - slippage_bps.min(10_000) as u32);
    let min_out = match U256::from(expected_out)
        .checked_mul(slip_mult)
        .map(|x| x / U256::from(10_000u32))
        .and_then(|x| u128::try_from(x).ok())
    {
        Some(v) => v,
        None => return (0, 0),
    };
    (expected_out, min_out)
}

/// USD value (e8s) of `native` units of a `decimals`-decimal stable token.
pub fn stable_native_to_e8s(native: u128, decimals: u8) -> u128 {
    let scale = 10u128.saturating_pow(decimals as u32);
    if scale == 0 {
        return 0;
    }
    native.saturating_mul(100_000_000) / scale
}

/// The DEX-depth oracle cross-check (spec §4.8): the pool's expected stable-out
/// must be at least `(1 - divergence)` of the oracle-implied USD value of the
/// seized collateral. Fail-closed (false) on a thin/manipulated pool.
pub fn oracle_corroborated(
    expected_out_native: u128,
    settle_decimals: u8,
    oracle_value_e8: u128,
    max_divergence_bps: u32,
) -> bool {
    let expected_out_e8 = stable_native_to_e8s(expected_out_native, settle_decimals);
    let floor =
        oracle_value_e8.saturating_mul(10_000u128.saturating_sub(max_divergence_bps as u128)) / 10_000;
    expected_out_e8 >= floor
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: u64 = 15_500; // 155%
    const BONUS: u64 = 11_200; // 10_000 + 1_200 penalty bps = 1.12x

    #[test]
    fn collateral_value_matches_collateral_ratio_math() {
        // 1400 CFX (1400e18 wei) @ $0.15 (15_000_000 e8) = $210 = 210e8.
        let v = collateral_value_e8s(1_400u128 * 10u128.pow(18), 18, 15_000_000);
        assert_eq!(v, 210u128 * 100_000_000);
    }

    #[test]
    fn effective_debt_adds_pending_interest_and_accrual() {
        // No apr/elapsed -> effective == debt + pending_interest only.
        assert_eq!(effective_debt_e8s(100, 7, 0, 0), 107);
        // With apr+elapsed the accrued term is added on top of debt (not pending).
        let e = effective_debt_e8s(100_000_000, 0, 200, 31_557_600_000_000_000); // ~1yr @ 2%
        assert!(e > 100_000_000, "accrual added");
    }

    #[test]
    fn sized_repay_zero_when_at_or_above_target() {
        // collateral_value already >= debt*target/10000 -> nothing to repay.
        assert_eq!(sized_repay_e8s(100, 1_000, TARGET, BONUS), 0);
    }

    #[test]
    fn sized_repay_full_when_denominator_nonpositive() {
        // target <= bonus -> denom <= 0 -> repay capped at full debt.
        assert_eq!(sized_repay_e8s(100, 0, 11_000, 11_200), 100);
    }

    #[test]
    fn sized_repay_partial_restores_near_target_and_above_threshold() {
        // Spec §4.6 property, honest about integer rounding: round-DOWN sizing
        // (borrower-favorable, never over-seizes) lands the restored CR a hair
        // UNDER target (< ~0.02%, dominated by the integer-truncation of the CR
        // readout itself) but ALWAYS comfortably above the liquidation threshold,
        // so the vault drops out of liquidation. Assert both across a fuzzed grid.
        const LIQ_THRESHOLD_E4: u64 = 13_300; // 133%
        const TOL_E4: u64 = 2; // covers truncation + the sub-unit round-down undershoot
        let mut checked = 0u64;
        for debt_units in 1u128..=60 {
            for cr_pct in 100u128..=154 {
                let eff_debt = debt_units * 100_000_000; // e8s
                let coll_val = eff_debt * cr_pct / 100; // collateral_value = debt * cr_pct/100
                let repay = sized_repay_e8s(eff_debt, coll_val, TARGET, BONUS);
                if repay == 0 || repay >= eff_debt {
                    continue; // only the strict-partial case has a restore target
                }
                let coll_sold = repay.saturating_mul(BONUS as u128) / 10_000;
                if coll_sold > coll_val {
                    continue; // cap-binds case (handled by the collateral clamp, not here)
                }
                let new_debt = eff_debt - repay;
                let new_coll = coll_val - coll_sold;
                let restored_cr_e4 = (new_coll.saturating_mul(10_000) / new_debt) as u64;
                // Restored to (essentially) target: at or above target minus a tiny
                // rounding tolerance.
                assert!(
                    restored_cr_e4 + TOL_E4 >= TARGET,
                    "restored {restored_cr_e4} not within {TOL_E4} of target {TARGET} (debt={eff_debt} cv={coll_val} repay={repay})"
                );
                // And the thing that actually matters: no longer liquidatable.
                assert!(
                    restored_cr_e4 > LIQ_THRESHOLD_E4,
                    "restored {restored_cr_e4} must clear the {LIQ_THRESHOLD_E4} liquidation threshold (debt={eff_debt} cv={coll_val} repay={repay})"
                );
                checked += 1;
            }
        }
        assert!(checked > 100, "property exercised on a meaningful grid ({checked})");
    }

    #[test]
    fn depth_cap_limits_repay_to_swappable_value() {
        // max_swap_value_e8s = $2000 (2000e8), bonus 1.12 -> max repay ~ 1785.7e8.
        let cap = value_cap_to_repay(2_000u128 * 100_000_000, BONUS);
        assert_eq!(cap, 2_000u128 * 100_000_000 * 10_000 / 11_200);
        // effective_repay = min(sized, cap)
        assert_eq!(cap.min(5_000u128 * 100_000_000), cap);
    }

    #[test]
    fn collateral_in_native_grosses_up_by_bonus() {
        // repay 100e8 @ $0.15, bonus 1.12, 18-dec: collateral worth $112 of CFX.
        let native = collateral_in_native_for_repay(100u128 * 100_000_000, BONUS, 18, 15_000_000);
        // $112 / $0.15 = 746.666.. CFX (round down)
        let expected = (112u128 * 100_000_000) // grossed value e8s
            .saturating_mul(10u128.pow(18))
            / 15_000_000;
        assert_eq!(native, expected);
    }

    #[test]
    fn bonus_from_penalty() {
        assert_eq!(bonus_e4_from_penalty_bps(1_200), 11_200);
    }

    // ─── Task 4: fail-closed price-staleness gate (spec §4.3) ───
    // (ChainId + MultiChainState come in via `use super::*`.)

    fn state_with_price(price: u64, set_at_ns: u64) -> MultiChainState {
        let mut s = MultiChainState::default();
        s.manual_prices.insert((ChainId(71), "CFX".into()), price);
        s.manual_price_set_at_ns.insert((ChainId(71), "CFX".into()), set_at_ns);
        s
    }

    #[test]
    fn fresh_price_ok_within_window() {
        let s = state_with_price(15_000_000, 1_000);
        assert_eq!(fresh_chain_price_e8(&s, ChainId(71), "CFX", 1_500, 1_000), Ok(15_000_000));
    }
    #[test]
    fn fresh_price_rejects_missing() {
        let s = MultiChainState::default();
        assert_eq!(fresh_chain_price_e8(&s, ChainId(71), "CFX", 1, 1_000), Err(PriceError::NoPrice));
    }
    #[test]
    fn fresh_price_rejects_zero() {
        let s = state_with_price(0, 1_000);
        assert_eq!(fresh_chain_price_e8(&s, ChainId(71), "CFX", 1_500, 1_000), Err(PriceError::ZeroPrice));
    }
    #[test]
    fn fresh_price_rejects_no_timestamp() {
        // pre-V5 price (set_at == 0) -> fail closed.
        let s = state_with_price(15_000_000, 0);
        assert_eq!(fresh_chain_price_e8(&s, ChainId(71), "CFX", 1_500, 1_000), Err(PriceError::NoTimestamp));
    }
    #[test]
    fn fresh_price_rejects_stale_and_accepts_boundary() {
        let s = state_with_price(15_000_000, 1_000);
        // exactly at the age ceiling -> OK; one ns past -> Stale.
        assert_eq!(fresh_chain_price_e8(&s, ChainId(71), "CFX", 2_000, 1_000), Ok(15_000_000));
        assert_eq!(fresh_chain_price_e8(&s, ChainId(71), "CFX", 2_001, 1_000), Err(PriceError::Stale));
    }

    // ─── Task 7: escalation predicate + routing-state prune (findings #10/#26/#36) ───
    use crate::chains::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};

    fn seed_cfg_unchecked(s: &mut MultiChainState) {
        use crate::chains::liquidation_config::{ChainLiquidationConfigV1, DexKind};
        s.chain_liquidation_configs.insert(
            ChainId(71),
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
            },
        );
    }

    fn insert_vault_liq(s: &mut MultiChainState, vault_id: u64, cfx_units: u128, debt_units: u128, status: ChainVaultStatus) {
        s.chain_vaults.insert(
            vault_id,
            ChainVaultV1 {
                vault_id,
                owner: candid::Principal::anonymous(),
                collateral_chain: ChainId(71),
                custody_address: "0xc".into(),
                collateral_amount_native: cfx_units * 1_000_000_000_000_000_000,
                debt_e8s: debt_units * 100_000_000,
                mint_recipient: "0xm".into(),
                pending_mint_e8s: 0,
                status,
                opened_at_ns: 0,
                owner_evm: None,
                last_interest_accrual_ns: 0,
                pending_interest_mint_e8s: 0,
                pending_liquidation: None,
            },
        );
    }

    #[test]
    fn escalation_predicate_timeout_and_terminal() {
        // Not timed out, op still live -> no escalation.
        assert!(!should_escalate_to_sp(1_000, 1_500, 1_000, false));
        // Timed out -> escalate.
        assert!(should_escalate_to_sp(1_000, 2_001, 1_000, false));
        // Op terminally failed -> escalate regardless of timeout.
        assert!(should_escalate_to_sp(1_000, 1_100, 1_000, true));
    }

    #[test]
    fn prune_clears_recovered_and_resolved_vaults() {
        let mut s = MultiChainState::default();
        seed_cfg_unchecked(&mut s);
        s.manual_prices.insert((ChainId(71), "CFX".into()), 15_000_000); // $0.15
        s.manual_price_set_at_ns.insert((ChainId(71), "CFX".into()), 1_000);
        // Vault 7 recovered above threshold (1400 CFX @ $0.15 = $210 vs 100 -> 210%).
        insert_vault_liq(&mut s, 7, 1_400, 100, ChainVaultStatus::Open);
        // Vault 8 closed (resolved).
        insert_vault_liq(&mut s, 8, 0, 0, ChainVaultStatus::Closed);
        s.bot_pending_chain_vaults.insert(7, 10);
        s.bot_pending_chain_vaults.insert(8, 10);
        s.sp_attempted_chain_vaults.insert(7);

        prune_recovered_chain_routing_state(&mut s, ChainId(71), "CFX", 13_300, 5_000);

        assert!(!s.bot_pending_chain_vaults.contains_key(&7), "recovered vault cleared");
        assert!(!s.bot_pending_chain_vaults.contains_key(&8), "resolved vault cleared");
        assert!(!s.sp_attempted_chain_vaults.contains(&7), "sp-attempted cleared on recovery");
    }

    #[test]
    fn detect_routes_liquidatable_and_caps_per_tick() {
        let mut s = MultiChainState::default();
        seed_cfg_unchecked(&mut s);
        s.manual_prices.insert((ChainId(71), "CFX".into()), 8_000_000); // $0.08 -> CR 112%
        s.manual_price_set_at_ns.insert((ChainId(71), "CFX".into()), 1_000);
        for vid in 1..=4u64 {
            insert_vault_liq(&mut s, vid, 1_400, 100, ChainVaultStatus::Open);
        }
        let routed = detect_and_route_chain_liquidations_in_state(&mut s, ChainId(71), "CFX", 13_300, 2_000, 2);
        assert_eq!(routed, 2, "capped at 2 per tick");
        let marked = s.chain_vaults.values().filter(|v| v.pending_liquidation.is_some()).count();
        assert_eq!(marked, 2, "exactly the routed vaults are marked");
        // Design B: debt untouched at trigger.
        assert!(s.chain_vaults.values().all(|v| v.debt_e8s == 100 * 100_000_000));
    }

    #[test]
    fn detect_skips_when_disabled_or_no_config() {
        let mut s = MultiChainState::default();
        s.manual_prices.insert((ChainId(71), "CFX".into()), 8_000_000);
        s.manual_price_set_at_ns.insert((ChainId(71), "CFX".into()), 1_000);
        insert_vault_liq(&mut s, 1, 1_400, 100, ChainVaultStatus::Open);
        // No config row -> no routing (master gate, spec §9).
        assert_eq!(detect_and_route_chain_liquidations_in_state(&mut s, ChainId(71), "CFX", 13_300, 2_000, 3), 0);
        assert!(s.chain_vaults.get(&1).unwrap().pending_liquidation.is_none());
        // Disabled config -> still no routing.
        seed_cfg_unchecked(&mut s);
        s.chain_liquidation_configs.get_mut(&ChainId(71)).unwrap().enabled = false;
        assert_eq!(detect_and_route_chain_liquidations_in_state(&mut s, ChainId(71), "CFX", 13_300, 2_000, 3), 0);
        assert!(s.chain_vaults.get(&1).unwrap().pending_liquidation.is_none());
    }

    #[test]
    fn detect_skips_bot_failed_sp_attempted_vaults() {
        // Finding #10: a vault the bot already failed (sp_attempted) is NOT
        // re-routed to the bot (no retry loop), even while still liquidatable.
        let mut s = MultiChainState::default();
        seed_cfg_unchecked(&mut s);
        s.manual_prices.insert((ChainId(71), "CFX".into()), 8_000_000);
        s.manual_price_set_at_ns.insert((ChainId(71), "CFX".into()), 1_000);
        insert_vault_liq(&mut s, 1, 1_400, 100, ChainVaultStatus::Open); // underwater
        s.sp_attempted_chain_vaults.insert(1);
        let routed = detect_and_route_chain_liquidations_in_state(&mut s, ChainId(71), "CFX", 13_300, 2_000, 10);
        assert_eq!(routed, 0, "bot-failed vault not re-routed");
        assert!(s.chain_vaults.get(&1).unwrap().pending_liquidation.is_none());
    }

    #[test]
    fn detect_skips_healthy_and_marked_vaults() {
        let mut s = MultiChainState::default();
        seed_cfg_unchecked(&mut s);
        s.manual_prices.insert((ChainId(71), "CFX".into()), 8_000_000);
        s.manual_price_set_at_ns.insert((ChainId(71), "CFX".into()), 1_000);
        insert_vault_liq(&mut s, 1, 1_400, 100, ChainVaultStatus::Open); // underwater
        insert_vault_liq(&mut s, 2, 5_000, 100, ChainVaultStatus::Open); // healthy (CR 400%)
        let routed = detect_and_route_chain_liquidations_in_state(&mut s, ChainId(71), "CFX", 13_300, 2_000, 10);
        assert_eq!(routed, 1, "only the underwater vault routes");
        assert!(s.chain_vaults.get(&1).unwrap().pending_liquidation.is_some());
        assert!(s.chain_vaults.get(&2).unwrap().pending_liquidation.is_none());
    }

    #[test]
    fn prune_keeps_still_liquidatable_marked_vault() {
        let mut s = MultiChainState::default();
        seed_cfg_unchecked(&mut s);
        s.manual_prices.insert((ChainId(71), "CFX".into()), 8_000_000); // $0.08 -> CR 112%
        s.manual_price_set_at_ns.insert((ChainId(71), "CFX".into()), 1_000);
        insert_vault_liq(&mut s, 7, 1_400, 100, ChainVaultStatus::Open); // still underwater
        s.bot_pending_chain_vaults.insert(7, 10);
        prune_recovered_chain_routing_state(&mut s, ChainId(71), "CFX", 13_300, 5_000);
        assert!(s.bot_pending_chain_vaults.contains_key(&7), "still-liquidatable vault NOT cleared");
    }

    // ─── Increment 3 / Task 2: pure swap min-out + oracle cross-check ───
    const E18: u128 = 1_000_000_000_000_000_000;
    const E8: u128 = 100_000_000;

    #[test]
    fn amount_out_min_v2_constant_product_with_fee_and_haircut() {
        let (expected, min_out) = compute_amount_out_min(
            2_000u128 * E18,
            1_000_000u128 * E18,
            90_900u128 * E18,
            25,
            250,
        );
        // ABSOLUTE value, not just relations: pool price ~0.0909 USDC/CFX, so
        // selling 2000 CFX yields ~181 USDC (18-dec) minus fee + small impact.
        // This catches the u128-overflow bug (saturating_mul gave a garbage ~3.4e10).
        assert!(
            expected > 175 * E18 && expected < 182 * E18,
            "expected_out {} not in the ~181e18 sane band (u256 math must not overflow)",
            expected
        );
        assert!(min_out > 0 && min_out < expected);
        // min_out is exactly the 2.5% haircut of expected.
        assert_eq!(min_out, expected.saturating_mul(9_750) / 10_000);
    }

    #[test]
    fn amount_out_min_zero_when_reserves_zero() {
        assert_eq!(compute_amount_out_min(1, 0, 100, 25, 250), (0, 0));
        assert_eq!(compute_amount_out_min(1, 100, 0, 25, 250), (0, 0));
        assert_eq!(compute_amount_out_min(0, 100, 100, 25, 250), (0, 0));
    }

    #[test]
    fn oracle_cross_check_rejects_thin_pool() {
        // expected_out 95 USDC (18-dec) vs oracle-implied 100 USD: exactly 5% below.
        let oracle_value_e8 = 100 * E8;
        let expected_out_native = 95u128 * E18;
        assert!(oracle_corroborated(expected_out_native, 18, oracle_value_e8, 500)); // at the edge: OK
        assert!(!oracle_corroborated(expected_out_native, 18, oracle_value_e8, 499)); // one bp tighter: reject
    }

    #[test]
    fn stable_native_to_e8s_scales_by_decimals() {
        assert_eq!(stable_native_to_e8s(50u128 * E18, 18), 50 * E8); // 18-dec USDC -> e8s
        assert_eq!(stable_native_to_e8s(50u128 * 1_000_000, 6), 50 * E8); // 6-dec -> e8s
    }
}
