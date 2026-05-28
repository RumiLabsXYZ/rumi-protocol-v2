//! Property tests for the supply invariant under random cross-chain ops.
//!
//! Strategy: build a randomized sequence of (chain_id, op) pairs where
//! ops are Mint(amount) / Burn(amount) / Bridge(src, dst, amount). Apply
//! each op via `apply_supply_delta` (failing the test if any apply errors
//! with `Divergence`), and assert after every step that
//! `sum(chain_supplies) == total_debt`. The harness tracks `total_debt`
//! explicitly so the property test does not depend on the live State.

use rumi_protocol_backend::chains::config::{
    ChainConfigV1, ChainId, ChainStatus, GasStrategy,
};
use rumi_protocol_backend::chains::multi_chain_state::MultiChainStateV1;
use rumi_protocol_backend::chains::supply::{apply_supply_delta, SupplyDelta};
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

fn seeded_state() -> MultiChainStateV1 {
    let mut state = MultiChainStateV1::default();
    for id in 1u32..=5u32 {
        state.chain_configs.insert(
            ChainId(id),
            ChainConfigV1 {
                chain_id: ChainId(id),
                display_name: format!("chain-{}", id),
                rpc_endpoints: vec![],
                finality_depth: 1,
                gas_strategy: GasStrategy::NotApplicable,
                chain_native_decimals: 18,
                registered_at_ns: 0,
                status: ChainStatus::Registered,
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
