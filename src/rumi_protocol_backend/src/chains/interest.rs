//! Chain-vault interest accrual math + harvest (Phase 1b Task 12, Option B).
//!
//! Mirrors the ICP-native `State::accrue_single_vault` factor formula
//! (`new_debt = ceil(debt * (1 + rate * elapsed / NANOS_PER_YEAR))`, round UP =
//! protocol favor, overflow => defer) but for foreign-chain vaults whose debt is
//! denominated in e8s `u128`. Unlike ICP, accrued interest is NOT folded into
//! `debt_e8s` here (that would break the chain supply invariant); it is only
//! realized when the on-chain interest mint confirms (see `evm/settlement.rs`).

use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal::Decimal;

use crate::numeric::NANOS_PER_YEAR;

/// Interest (e8s) accrued on `debt_e8s` at `apr_bps` (fraction x 10^-4; 200 =
/// 2.00% APR) over `elapsed_ns` nanoseconds. Rounds UP (protocol favor). Returns
/// 0 on zero debt / zero elapsed / zero rate, and DEFERS (returns 0, never
/// panics) on any Decimal overflow — mirroring ICP's overflow-defer.
pub fn accrued_chain_interest_e8s(debt_e8s: u128, apr_bps: u64, elapsed_ns: u64) -> u128 {
    if debt_e8s == 0 || apr_bps == 0 || elapsed_ns == 0 {
        return 0;
    }
    // Defer (not panic) if debt cannot be represented as a Decimal.
    let debt = match Decimal::from_u128(debt_e8s) {
        Some(d) => d,
        None => return 0,
    };
    let rate = Decimal::from(apr_bps) / Decimal::from(10_000u64);
    let factor = Decimal::ONE + rate * Decimal::from(elapsed_ns) / Decimal::from(NANOS_PER_YEAR);
    // new_debt = ceil(debt * factor); the accrued interest is the delta. Defer
    // on a Decimal->u128 overflow (unreachable at real debt scales).
    let new_debt = match (debt * factor).ceil().to_u128() {
        Some(n) => n,
        None => return 0,
    };
    new_debt.saturating_sub(debt_e8s)
}

use crate::chains::config::ChainId;
use crate::chains::multi_chain_state::MultiChainState;
use crate::chains::settlement_queue::{SettlementOp, SettlementOpKind};
use crate::chains::vault::ChainVaultStatus;

/// Enqueue an `InterestMint` for every `Open`, debt-bearing vault on `chain`
/// whose accrued interest clears `threshold_e8s` and has no interest mint
/// already in flight. Sets `pending_interest_mint_e8s` (Design-B reserve); does
/// NOT touch `debt_e8s`, supply, or `last_interest_accrual_ns` — the confirm
/// path realizes those once the on-chain mint lands. `alloc_mint_id` yields
/// FRESH globally-unique ids (from `chain_vault_id_counter`) so the on-chain
/// `IcUSD.mint` never collides with a real vault id or a prior interest mint.
/// `treasury_recipient` is the per-chain interest-treasury address (resolved by
/// the async caller). Returns the op_ids enqueued.
pub fn harvest_chain_interest_in_state(
    state: &mut MultiChainState,
    chain: ChainId,
    apr_bps: u64,
    threshold_e8s: u128,
    treasury_recipient: &str,
    now_ns: u64,
    mut alloc_mint_id: impl FnMut() -> u64,
) -> Vec<u64> {
    if state.chain_bad_debt_circuit_tripped(chain) {
        ic_canister_log::log!(
            crate::logs::INFO,
            "[harvest chain={:?}] bad-debt circuit tripped; skipping interest mint enqueue",
            chain
        );
        return Vec::new();
    }

    // Phase 1: pick eligible vaults + locked-in amounts (immutable scan, so the
    // mint-id allocator + enqueue can borrow `state` mutably in phase 2).
    let eligible: Vec<(u64, u128)> = state
        .chain_vaults
        .values()
        .filter(|v| {
            v.collateral_chain == chain
                && v.status == ChainVaultStatus::Open
                && v.debt_e8s > 0
                && v.pending_interest_mint_e8s == 0
        })
        .filter_map(|v| {
            let accrued = accrued_chain_interest_e8s(
                v.debt_e8s,
                apr_bps,
                now_ns.saturating_sub(v.last_interest_accrual_ns),
            );
            (accrued > 0 && accrued >= threshold_e8s).then_some((v.vault_id, accrued))
        })
        .collect();

    // Phase 2: enqueue an InterestMint + reserve the pending amount for each.
    let mut enqueued = Vec::new();
    for (vault_id, amount) in eligible {
        let mint_id = alloc_mint_id();
        let op = SettlementOp::new(
            SettlementOpKind::InterestMint {
                vault_id,
                mint_id,
                amount_e8s: amount,
                accrual_through_ns: now_ns,
                recipient: treasury_recipient.to_string(),
            },
            format!("interest-{}-{}", chain.0, mint_id),
            now_ns,
        );
        match state
            .settlement_queues
            .entry(chain)
            .or_default()
            .enqueue(op)
        {
            Ok(op_id) => {
                if let Some(v) = state.chain_vaults.get_mut(&vault_id) {
                    v.pending_interest_mint_e8s = amount;
                }
                enqueued.push(op_id);
            }
            // Duplicate idempotency key (a fresh mint_id makes this unreachable);
            // skip and retry at the next harvest. No reservation made. Log it —
            // if it ever fires, something is wrong (mirrors accrue_single_vault's
            // overflow-defer log).
            Err(e) => {
                ic_canister_log::log!(
                    crate::logs::INFO,
                    "[harvest chain={:?}] InterestMint enqueue failed for vault {} (unexpected with a fresh mint_id): {:?}; no reservation made",
                    chain,
                    vault_id,
                    e
                );
            }
        }
    }
    enqueued
}

#[cfg(test)]
mod tests {
    use super::accrued_chain_interest_e8s;
    const E8: u128 = 100_000_000;
    const NANOS_PER_YEAR: u64 = 365 * 24 * 60 * 60 * 1_000_000_000;

    #[test]
    fn two_percent_full_year_on_100_icusd_is_2_icusd() {
        assert_eq!(
            accrued_chain_interest_e8s(100 * E8, 200, NANOS_PER_YEAR),
            2 * E8
        );
    }

    #[test]
    fn half_year_is_half_the_interest() {
        assert_eq!(
            accrued_chain_interest_e8s(100 * E8, 200, NANOS_PER_YEAR / 2),
            E8
        );
    }

    #[test]
    fn rounds_up_protocol_favor() {
        // A 1ns window on 100 icUSD yields a sub-e8s interest that must ceil to 1.
        assert_eq!(accrued_chain_interest_e8s(100 * E8, 200, 1), 1);
    }

    #[test]
    fn zero_debt_zero_interest() {
        assert_eq!(accrued_chain_interest_e8s(0, 200, NANOS_PER_YEAR), 0);
    }

    #[test]
    fn zero_elapsed_zero_interest() {
        assert_eq!(accrued_chain_interest_e8s(100 * E8, 200, 0), 0);
    }

    #[test]
    fn zero_bps_zero_interest() {
        assert_eq!(accrued_chain_interest_e8s(100 * E8, 0, NANOS_PER_YEAR), 0);
    }

    #[test]
    fn overflow_defers_to_zero_not_panic() {
        // u128::MAX exceeds Decimal's range -> defer (0), never panic.
        assert_eq!(
            accrued_chain_interest_e8s(u128::MAX, 200, NANOS_PER_YEAR),
            0
        );
    }

    // ─── harvest_chain_interest_in_state ────────────────────────────────────
    use super::harvest_chain_interest_in_state;
    use crate::chains::config::ChainId;
    use crate::chains::multi_chain_state::MultiChainState;
    use crate::chains::settlement_queue::SettlementOpKind;
    use crate::chains::vault::{ChainVaultStatus, ChainVaultV1};
    use candid::Principal;

    const CHAIN: ChainId = ChainId(71);
    const TREASURY: &str = "0x00000000000000000000000000000000000c0ffe";

    fn open_vault(
        s: &mut MultiChainState,
        vault_id: u64,
        debt_e8s: u128,
        last_accrual_ns: u64,
        pending_interest: u128,
        status: ChainVaultStatus,
    ) {
        s.chain_vaults.insert(
            vault_id,
            ChainVaultV1 {
                vault_id,
                owner: Principal::anonymous(),
                collateral_chain: CHAIN,
                custody_address: "0xc".into(),
                collateral_amount_native: 1_400 * 1_000_000_000_000_000_000,
                debt_e8s,
                mint_recipient: "0xr".into(),
                pending_mint_e8s: 0,
                status,
                opened_at_ns: 0,
                owner_evm: None,
                last_interest_accrual_ns: last_accrual_ns,
                pending_interest_mint_e8s: pending_interest,
                pending_liquidation: None,
            },
        );
    }

    #[test]
    fn harvest_enqueues_one_interest_mint_per_eligible_vault() {
        let mut s = MultiChainState::default();
        open_vault(
            &mut s,
            1,
            100 * E8,
            /*last accrual a year ago*/ 0,
            0,
            ChainVaultStatus::Open,
        );
        let now = NANOS_PER_YEAR;
        let mut next = 1000u64;
        let ops =
            harvest_chain_interest_in_state(&mut s, CHAIN, 200, 1_000_000, TREASURY, now, || {
                next += 1;
                next
            });
        assert_eq!(ops.len(), 1, "one eligible vault -> one op");
        let v = &s.chain_vaults[&1];
        assert_eq!(
            v.pending_interest_mint_e8s,
            2 * E8,
            "reserved 2% of 100 for a year"
        );
        assert_eq!(v.debt_e8s, 100 * E8, "debt untouched until confirm");
        assert_eq!(
            v.last_interest_accrual_ns, 0,
            "accrual window not advanced at harvest"
        );
        let q = &s.settlement_queues[&CHAIN];
        let op = q.pending.values().next().expect("op enqueued");
        match &op.kind {
            SettlementOpKind::InterestMint {
                vault_id,
                mint_id,
                amount_e8s,
                accrual_through_ns,
                recipient,
            } => {
                assert_eq!(*vault_id, 1);
                assert_eq!(*mint_id, 1001, "id from the allocator");
                assert_eq!(*amount_e8s, 2 * E8);
                assert_eq!(*accrual_through_ns, now);
                assert_eq!(recipient, TREASURY);
            }
            other => panic!("expected InterestMint, got {other:?}"),
        }
    }

    #[test]
    fn harvest_skips_interest_mint_when_bad_debt_circuit_tripped() {
        let mut s = MultiChainState::default();
        s.chain_bad_debt_circuit_tripped_at_ns.insert(CHAIN, 42);
        open_vault(
            &mut s,
            1,
            100 * E8,
            /*last accrual a year ago*/ 0,
            0,
            ChainVaultStatus::Open,
        );
        let now = NANOS_PER_YEAR;
        let mut next = 1000u64;

        let ops =
            harvest_chain_interest_in_state(&mut s, CHAIN, 200, 1_000_000, TREASURY, now, || {
                next += 1;
                next
            });

        assert!(ops.is_empty());
        assert_eq!(s.chain_vaults[&1].pending_interest_mint_e8s, 0);
        assert!(s.settlement_queues.get(&CHAIN).is_none());
    }

    #[test]
    fn harvest_skips_below_threshold() {
        let mut s = MultiChainState::default();
        // 1ns elapsed on 100 icUSD -> 1 e8s accrued, below a 1e6 threshold.
        open_vault(&mut s, 1, 100 * E8, 0, 0, ChainVaultStatus::Open);
        let ops =
            harvest_chain_interest_in_state(&mut s, CHAIN, 200, 1_000_000, TREASURY, 1, || 99);
        assert!(ops.is_empty(), "below-threshold accrual is not realized");
        assert_eq!(s.chain_vaults[&1].pending_interest_mint_e8s, 0);
    }

    #[test]
    fn harvest_skips_in_flight_non_open_and_zero_debt() {
        let mut s = MultiChainState::default();
        open_vault(
            &mut s,
            1,
            100 * E8,
            0,
            /*in flight*/ 50_000_000,
            ChainVaultStatus::Open,
        );
        open_vault(&mut s, 2, 100 * E8, 0, 0, ChainVaultStatus::MintPending); // not Open
        open_vault(&mut s, 3, 0, 0, 0, ChainVaultStatus::Open); // zero debt
        let ops =
            harvest_chain_interest_in_state(&mut s, CHAIN, 200, 1, TREASURY, NANOS_PER_YEAR, || 99);
        assert!(
            ops.is_empty(),
            "in-flight / non-Open / zero-debt vaults are all skipped"
        );
    }

    #[test]
    fn harvest_allocates_disjoint_mint_ids_for_multiple_vaults() {
        let mut s = MultiChainState::default();
        open_vault(&mut s, 1, 100 * E8, 0, 0, ChainVaultStatus::Open);
        open_vault(&mut s, 2, 100 * E8, 0, 0, ChainVaultStatus::Open);
        let mut next = 5000u64;
        let ops = harvest_chain_interest_in_state(
            &mut s,
            CHAIN,
            200,
            1,
            TREASURY,
            NANOS_PER_YEAR,
            || {
                next += 1;
                next
            },
        );
        assert_eq!(ops.len(), 2);
        let mint_ids: Vec<u64> = s.settlement_queues[&CHAIN]
            .pending
            .values()
            .filter_map(|o| match &o.kind {
                SettlementOpKind::InterestMint { mint_id, .. } => Some(*mint_id),
                _ => None,
            })
            .collect();
        assert_eq!(mint_ids.len(), 2);
        assert_ne!(mint_ids[0], mint_ids[1], "mint ids are disjoint");
    }
}
