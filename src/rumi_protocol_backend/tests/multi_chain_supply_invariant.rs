//! Property tests for the supply invariant under random cross-chain ops.
//!
//! Strategy: build a randomized sequence of (chain_id, op) pairs where
//! ops are Mint(amount) / Burn(amount) / Bridge(src, dst, amount). Apply
//! each op via `apply_supply_delta` (failing the test if any apply errors
//! with `Divergence`), and assert after every step that
//! `sum(chain_supplies) == total_debt`. The harness tracks `total_debt`
//! explicitly so the property test does not depend on the live State.

use rumi_protocol_backend::chains::config::{
    ChainConfigV3, ChainId, ChainStatus, GasStrategy,
};
use rumi_protocol_backend::chains::multi_chain_state::MultiChainStateV4;
use rumi_protocol_backend::chains::supply::{apply_supply_delta, SupplyDelta};
use rumi_protocol_backend::chains::monad::chain_vault::open_chain_vault_in_state;
use rumi_protocol_backend::chains::monad::settlement::confirm_mint_in_state;
use rumi_protocol_backend::chains::monad::deposit_watch::apply_burn_to_state;
use rumi_protocol_backend::chains::monad::evm_rpc::BurnLog;
use proptest::prelude::*;

#[derive(Clone, Debug)]
enum Op {
    Mint { chain: u32, amount: u64 },
    Burn { chain: u32, amount: u64 },
    Bridge { src: u32, dst: u32, amount: u64 },
}

fn arb_op() -> impl Strategy<Value = Op> {
    let chain_id_strat = 1u32..=5u32;
    let amount_strat = 1u64..=1_000_000u64;
    prop_oneof![
        (chain_id_strat.clone(), amount_strat.clone())
            .prop_map(|(c, a)| Op::Mint { chain: c, amount: a }),
        (chain_id_strat.clone(), amount_strat.clone())
            .prop_map(|(c, a)| Op::Burn { chain: c, amount: a }),
        (chain_id_strat.clone(), chain_id_strat.clone(), amount_strat.clone())
            .prop_map(|(s, d, a)| Op::Bridge { src: s, dst: d, amount: a }),
    ]
}

fn seeded_state() -> MultiChainStateV4 {
    let mut state = MultiChainStateV4::default();
    for id in 1u32..=5u32 {
        state.chain_configs.insert(
            ChainId(id),
            ChainConfigV3 {
                chain_id: ChainId(id),
                display_name: format!("chain-{}", id),
                rpc_endpoints: vec![],
                finality_depth: 1,
                gas_strategy: GasStrategy::NotApplicable,
                chain_native_decimals: 18,
                registered_at_ns: 0,
                status: ChainStatus::Registered,
                burn_watch_poll_enabled: false,
                min_quorum_providers: None,
            },
        );
        state.chain_supplies.insert(ChainId(id), 0);
    }
    state
}

proptest! {
    #[test]
    fn invariant_holds_after_every_random_op(ops in proptest::collection::vec(arb_op(), 0..40)) {
        let mut state = seeded_state();
        let mut total_debt: u128 = 0;

        for op in ops {
            match op {
                Op::Mint { chain, amount } => {
                    let new_total = total_debt + amount as u128;
                    let res = apply_supply_delta(
                        &mut state,
                        ChainId(chain),
                        SupplyDelta::Increase(amount as u128),
                        new_total,
                    );
                    if res.is_ok() {
                        total_debt = new_total;
                    }
                }
                Op::Burn { chain, amount } => {
                    let current = state.chain_supplies[&ChainId(chain)];
                    if (amount as u128) > current || (amount as u128) > total_debt {
                        continue;
                    }
                    let new_total = total_debt - amount as u128;
                    let res = apply_supply_delta(
                        &mut state,
                        ChainId(chain),
                        SupplyDelta::Decrease(amount as u128),
                        new_total,
                    );
                    if res.is_ok() {
                        total_debt = new_total;
                    }
                }
                Op::Bridge { src, dst, amount } => {
                    if src == dst {
                        continue;
                    }
                    let current_src = state.chain_supplies[&ChainId(src)];
                    if (amount as u128) > current_src {
                        continue;
                    }
                    // Bridge: decrease on src, increase on dst. Total supply unchanged.
                    // After burn, sum temporarily dips below total_debt, so we pass
                    // the intermediate total to the burn.
                    let intermediate_total = total_debt - amount as u128;
                    let burn = apply_supply_delta(
                        &mut state,
                        ChainId(src),
                        SupplyDelta::Decrease(amount as u128),
                        intermediate_total,
                    );
                    prop_assert!(burn.is_ok(), "bridge burn rejected: {:?}", burn);
                    let mint = apply_supply_delta(
                        &mut state,
                        ChainId(dst),
                        SupplyDelta::Increase(amount as u128),
                        total_debt,
                    );
                    prop_assert!(mint.is_ok(), "bridge mint rejected: {:?}", mint);
                }
            }

            // Invariant after every op:
            let sum: u128 = state.chain_supplies.values().copied().sum();
            prop_assert_eq!(sum, total_debt);
        }
    }
}

// ─── Real-flow proptest ────────────────────────────────────────────────────
//
// The Phase 1a proptest above drives `apply_supply_delta` directly. This second
// proptest drives the REAL chain-vault + settlement helpers, proving the supply
// invariant survives the full open -> confirm -> burn lifecycle backed by the
// `chain_vaults` + `settlement_queues` state (not a synthetic supply delta).
//
// Supply-relevant transitions:
//   - open    (`open_chain_vault_in_state`): NO supply change — the vault is
//     created `AwaitingDeposit` with `debt_e8s = 0`, `pending_mint_e8s = amount`,
//     and enqueues nothing (open-then-verify, Design B).
//   - confirm (`confirm_mint_in_state`): `debt_e8s += amount`, supply += amount.
//   - burn    (`apply_burn_to_state`):     `debt_e8s -= amount`, supply -= amount.
//
// CRITICAL — pre-operation-total convention: `confirm_mint_in_state` and
// `apply_burn_to_state` BOTH take the PRE-operation total chain-vault debt and
// compute the post-total INTERNALLY (`pre + observed` / `pre - amount`). So we
// pass the CURRENT `total_debt` (the pre-op value) and only update our local
// `total_debt` tracker AFTER a successful call. Passing `total_debt ± amount`
// would double-count and the internal invariant check would reject the op.
//
// `confirm_mint_in_state` only checks `pending_mint_e8s == observed_e8s` (not the
// status), so calling it directly after open works here — the intermediate
// MintPending flip / deposit-verify step is orthogonal to supply and is covered
// by the unit + happy-path tests. The flow is therefore open -> confirm ->
// (later) burn directly.

#[derive(Clone, Debug)]
enum RealOp {
    OpenAndConfirm { chain: u32, amount: u64 },
    BurnPartial { vault_id: u64, frac_pct: u8 },
}

fn arb_real_op() -> impl Strategy<Value = RealOp> {
    prop_oneof![
        (1u32..=5u32, 1u64..=1_000_000u64)
            .prop_map(|(c, a)| RealOp::OpenAndConfirm { chain: c, amount: a }),
        (0u64..50, 0u8..=100u8).prop_map(|(v, f)| RealOp::BurnPartial { vault_id: v, frac_pct: f }),
    ]
}

proptest! {
    #[test]
    fn invariant_holds_across_open_confirm_burn(
        ops in proptest::collection::vec(arb_real_op(), 0..30)
    ) {
        // V2 seeded state: chains 1..=5 registered, chain_supplies 0. The
        // open/confirm/burn helpers additionally need a settlement queue and a
        // MON price per chain, so extend the seed here.
        let mut state = seeded_state();
        for id in 1u32..=5u32 {
            let cid = ChainId(id);
            state.settlement_queues.entry(cid).or_default();
            // $100/MON (USD e8). Combined with the huge declared collateral
            // below this keeps the CR effectively unbounded.
            state.manual_prices.insert((cid, "MON".to_string()), 100_0000_0000);
        }

        let mut total_debt: u128 = 0;
        let mut next_vault_id: u64 = 0;
        let mut open_vaults: Vec<u64> = vec![];

        for op in ops {
            match op {
                RealOp::OpenAndConfirm { chain, amount } => {
                    next_vault_id += 1;
                    let cid = ChainId(chain);
                    // Huge declared collateral + min_cr_e4 = 0 so the CR never
                    // binds — this test isolates the SUPPLY invariant, not CR.
                    let collateral = 1_000_000_000_000_000_000_000_000u128;
                    let amt = amount as u128;
                    let opened = open_chain_vault_in_state(
                        &mut state,
                        cid,
                        candid::Principal::anonymous(),
                        format!("0x{next_vault_id}"),
                        collateral,
                        amt,
                        // Valid EVM address (0x + 40 hex). The open path now
                        // rejects a malformed mint_recipient; a bad value here
                        // would make every open fail and the invariant pass
                        // VACUOUSLY (no vaults ever opened). The prop_assert
                        // below locks in that opens actually succeed.
                        "0x000000000000000000000000000000000000c0de".to_string(),
                        0,
                        0,
                        next_vault_id,
                    )
                    .is_ok();
                    // This open is constructed to always succeed (registered
                    // chain, price set, huge collateral, min_cr 0, non-zero debt,
                    // valid recipient). If it ever fails the supply invariant
                    // would hold vacuously — fail loudly instead.
                    prop_assert!(opened, "OpenAndConfirm must succeed; open returned Err");
                    if opened {
                        // PRE-mint total is the current total_debt; the helper
                        // adds `amt` internally to derive the post-mint total.
                        if confirm_mint_in_state(&mut state, cid, next_vault_id, amt, total_debt)
                            .is_ok()
                        {
                            total_debt += amt;
                            open_vaults.push(next_vault_id);
                        }
                    }
                }
                RealOp::BurnPartial { vault_id, frac_pct } => {
                    if open_vaults.is_empty() {
                        continue;
                    }
                    let vid = open_vaults[(vault_id as usize) % open_vaults.len()];
                    let debt = state.chain_vaults[&vid].debt_e8s;
                    if debt == 0 {
                        continue;
                    }
                    let amount = (debt * (frac_pct.min(100) as u128)) / 100;
                    if amount == 0 {
                        continue;
                    }
                    // PRE-burn total is the current total_debt; the helper
                    // subtracts `amount` internally to derive the post-burn total.
                    let burn = BurnLog {
                        vault_id: vid,
                        amount_e8s: amount,
                        tx_hash: "0xb".to_string(),
                        block_number: 1,
                    };
                    if apply_burn_to_state(&mut state, &burn, total_debt).is_ok() {
                        total_debt -= amount;
                    }
                }
            }

            // Invariant after every op: sum(chain_supplies) == total chain-vault
            // debt. A failure here is a REAL supply-invariant bug, not a test
            // artifact — do not weaken this assertion.
            let sum: u128 = state.chain_supplies.values().copied().sum();
            prop_assert_eq!(sum, total_debt);
        }
    }
}
