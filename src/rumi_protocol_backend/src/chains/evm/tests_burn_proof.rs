use crate::chains::config::{ChainConfigV3, ChainId, ChainStatus, GasStrategy};
use crate::chains::monad::burn_proof::{apply_receipt_burns_to_state, ApplyBurnsError};
use crate::chains::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
use crate::chains::monad::evm_rpc::{TxReceiptWithLogs, BURN_EVENT_TOPIC0};
use crate::chains::multi_chain_state::MultiChainState;
use candid::Principal;

fn word(v: u128) -> String {
    format!("0x{:064x}", v)
}

fn state_with_open_vault(debt: u128) -> MultiChainState {
    let mut s = MultiChainState::default();
    s.chain_supplies.insert(ChainId(10143), debt);
    s.chain_vaults.insert(
        1,
        ChainVaultV1 {
            vault_id: 1,
            owner: Principal::anonymous(),
            collateral_chain: ChainId(10143),
            custody_address: "0xc".into(),
            collateral_amount_native: 0,
            debt_e8s: debt,
            mint_recipient: "0xr".into(),
            pending_mint_e8s: 0,
            status: ChainVaultStatus::Open,
            opened_at_ns: 0,
            owner_evm: None,
            last_interest_accrual_ns: 0,
            pending_interest_mint_e8s: 0,
            pending_liquidation: None,
        },
    );
    s
}

fn chain_config(status: ChainStatus) -> ChainConfigV3 {
    ChainConfigV3 {
        chain_id: ChainId(10143),
        display_name: "MonadTestnet".into(),
        rpc_endpoints: vec!["https://rpc.example".into()],
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 2,
            max_fee_gwei_ceiling: 500,
        },
        chain_native_decimals: 18,
        registered_at_ns: 0,
        status,
        burn_watch_poll_enabled: false,
        min_quorum_providers: Some(1),
    }
}

#[test]
fn applies_burn_log_from_correct_contract_and_dedups() {
    let mut s = state_with_open_vault(100);
    let contract = "0xcafe";
    let receipt = TxReceiptWithLogs {
        tx_hash: None,
        success: true,
        block_number: 10,
        logs: vec![(
            contract.to_string(),
            vec![BURN_EVENT_TOPIC0.to_string(), word(1), word(0xdead)],
            word(40),
            3,
        )],
    };
    let applied = apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt)
        .expect("apply");
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].vault_id, 1);
    assert_eq!(applied[0].amount_e8s, 40);
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
    // Re-apply same receipt → deduped, no change.
    let again = apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt)
        .expect("apply again");
    assert_eq!(again.len(), 0);
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
}

#[test]
fn disabled_chain_still_applies_verified_repayment_burn() {
    let chain = ChainId(10143);
    let mut s = state_with_open_vault(100);
    s.chain_configs
        .insert(chain, chain_config(ChainStatus::Disabled));
    let receipt = halted_receipt("0xcafe");

    let applied = apply_receipt_burns_to_state(&mut s, chain, "0xcafe", "0xtx", &receipt)
        .expect("a verified repayment proof remains available while disabled");

    assert_eq!(applied.len(), 1);
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
    assert_eq!(s.chain_supplies[&chain], 60);
}

#[test]
fn proof_replay_is_rejected_after_observer_covers_and_prunes_its_block() {
    use super::deposit_watch::advance_cursor_and_prune;

    let mut s = state_with_open_vault(100);
    let chain = ChainId(10143);
    let contract = "0xcafe";
    let receipt = TxReceiptWithLogs {
        tx_hash: None,
        success: true,
        block_number: 10,
        logs: vec![(
            contract.to_string(),
            vec![BURN_EVENT_TOPIC0.to_string(), word(1), word(0xdead)],
            word(40),
            3,
        )],
    };
    let first = apply_receipt_burns_to_state(&mut s, chain, contract, "0xtx", &receipt)
        .expect("first direct proof applies");
    assert_eq!(first.len(), 1);
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
    assert!(s.has_evm_burn_replay_id(chain, 10, "0xtx", 3));

    advance_cursor_and_prune(&mut s, chain, 20);
    assert_eq!(s.evm_burn_proof_floor_by_chain.get(&chain), Some(&20));
    assert!(!s.has_evm_burn_replay_id(chain, 10, "0xtx", 3));
    // Removing/reseeding the observer cursor cannot reopen the covered history.
    s.last_observed_block.remove(&chain);
    let replay = apply_receipt_burns_to_state(&mut s, chain, contract, "0xtx", &receipt);
    assert_eq!(
        replay,
        Err(ApplyBurnsError::StaleProof {
            block: 10,
            floor: 20
        })
    );
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
    assert_eq!(s.chain_supplies[&chain], 60);
}

#[test]
fn observer_coverage_floor_blocks_late_burn_proof_after_pruning() {
    use super::deposit_watch::apply_burn_log_window_and_advance;

    let mut s = state_with_open_vault(100);
    let chain = ChainId(10143);
    let contract = "0xcafe";
    let topics = vec![BURN_EVENT_TOPIC0.to_string(), word(1), word(0xdead)];
    apply_burn_log_window_and_advance(
        &mut s,
        chain,
        &[(topics.clone(), word(40), "0xtx".into(), 10, 3)],
        10,
    )
    .expect("observer applies the finalized burn");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
    assert_eq!(s.evm_burn_proof_floor_by_chain.get(&chain), Some(&10));
    assert!(!s.processed_burn_keys.contains_key(&10));

    // Once the observer prunes the block-scoped key, the monotonic floor
    // rejects every pull-proof ingress for that covered block.
    let receipt = TxReceiptWithLogs {
        tx_hash: None,
        success: true,
        block_number: 10,
        logs: vec![(contract.into(), topics, word(40), 3)],
    };
    assert_eq!(
        apply_receipt_burns_to_state(&mut s, chain, contract, "0xtx", &receipt),
        Err(ApplyBurnsError::StaleProof {
            block: 10,
            floor: 10
        })
    );
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
    assert_eq!(s.chain_supplies[&chain], 60);
}

#[test]
fn settlement_consumption_blocks_burn_proof() {
    let mut s = state_with_open_vault(100);
    let chain = ChainId(10143);
    s.settled_settlement_burn_logs.insert("10143:0xtx:3".into());
    let receipt = TxReceiptWithLogs {
        tx_hash: None,
        success: true,
        block_number: 10,
        logs: vec![(
            "0xcafe".into(),
            vec![BURN_EVENT_TOPIC0.into(), word(1), word(0xdead)],
            word(40),
            3,
        )],
    };
    let applied = apply_receipt_burns_to_state(&mut s, chain, "0xcafe", "0xtx", &receipt)
        .expect("settlement-consumed burn is skipped");
    assert!(applied.is_empty());
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100);
    assert_eq!(s.chain_supplies[&chain], 100);
}

#[test]
fn burn_proof_consumption_blocks_pending_settlement_proof() {
    use crate::chains::evm::settlement_proof::VerifiedBurnSettlementProof;

    let mut s = state_with_open_vault(100);
    let chain = ChainId(10143);
    let receipt = TxReceiptWithLogs {
        tx_hash: None,
        success: true,
        block_number: 10,
        logs: vec![(
            "0xcafe".into(),
            vec![BURN_EVENT_TOPIC0.into(), word(1), word(0xdead)],
            word(40),
            3,
        )],
    };
    apply_receipt_burns_to_state(&mut s, chain, "0xcafe", "0xtx", &receipt)
        .expect("receipt proof consumes burn first");
    let supply_after_proof = s.chain_supplies[&chain];
    let debt_after_proof = s.chain_vaults[&1].debt_e8s;
    let settlement = VerifiedBurnSettlementProof {
        proof_id: "pending:0xtx:3".into(),
        tx_hash: "0xtx".into(),
        log_index: 3,
        block_number: 10,
        vault_id: 1,
        burner: "0xoperator".into(),
        amount_e8s: 40,
    };

    assert!(matches!(
        crate::chains::supply::settle_pending_chain_burn_with_verified_proof(
            &mut s, chain, settlement, 1_700
        ),
        Err(crate::chains::supply::ProofBackedSettlementError::DuplicateBurnLog { .. })
    ));
    assert_eq!(s.chain_supplies[&chain], supply_after_proof);
    assert_eq!(s.chain_vaults[&1].debt_e8s, debt_after_proof);
}

#[test]
fn another_chains_cursor_cannot_prune_a_proof_chains_burn_marker() {
    use super::deposit_watch::{advance_cursor_and_prune, observer_burn_was_already_consumed};

    let mut s = state_with_open_vault(100);
    let proof_chain = ChainId(10143);
    let other_chain = ChainId(1030);
    let receipt = TxReceiptWithLogs {
        tx_hash: None,
        success: true,
        block_number: 10,
        logs: vec![(
            "0xcafe".into(),
            vec![BURN_EVENT_TOPIC0.into(), word(1), word(0xdead)],
            word(40),
            3,
        )],
    };

    apply_receipt_burns_to_state(&mut s, proof_chain, "0xcafe", "0xtx", &receipt)
        .expect("direct proof applies");
    assert!(s.processed_burn_keys.contains_key(&10));
    assert!(s.has_evm_burn_replay_id(proof_chain, 10, "0xtx", 3));

    // The processed map is globally block-keyed, but a chain's cursor cannot
    // prune another chain's marker before that marker's own proof floor covers
    // the block. This keeps both observer and proof consumers replay-safe.
    advance_cursor_and_prune(&mut s, other_chain, 20);
    assert!(s.processed_burn_keys.contains_key(&10));
    assert!(s.has_evm_burn_replay_id(proof_chain, 10, "0xtx", 3));
    assert!(observer_burn_was_already_consumed(
        &s,
        proof_chain,
        10,
        "0xtx",
        3
    ));
    assert!(!observer_burn_was_already_consumed(
        &s,
        other_chain,
        10,
        "0xtx",
        3
    ));
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
    assert_eq!(s.chain_supplies[&proof_chain], 60);
}

#[test]
fn pending_replay_ids_drain_only_after_bounded_coverage_windows() {
    use super::deposit_watch::{burn_proof_coverage_window, replay_catchup_is_required};

    let chain = ChainId(10143);
    let mut s = state_with_open_vault(100);
    let receipt = TxReceiptWithLogs {
        tx_hash: None,
        success: true,
        block_number: 1500,
        logs: vec![(
            "0xcafe".into(),
            vec![BURN_EVENT_TOPIC0.into(), word(1), word(0xdead)],
            word(40),
            3,
        )],
    };
    apply_receipt_burns_to_state(&mut s, chain, "0xcafe", "0xtx", &receipt)
        .expect("direct proof applies");

    assert!(replay_catchup_is_required(true, 0, 2048));
    let first = burn_proof_coverage_window(0, 0, 2048).unwrap();
    assert_eq!(first, (1, 1024));
    s.advance_evm_burn_proof_floor(chain, first.1);
    assert!(s.has_evm_burn_replay_id(chain, 1500, "0xtx", 3));

    let second = burn_proof_coverage_window(first.1, 0, 2048).unwrap();
    assert_eq!(second, (1025, 2048));
    s.advance_evm_burn_proof_floor(chain, second.1);
    assert!(!s.has_evm_burn_replay_id(chain, 1500, "0xtx", 3));
    assert!(!replay_catchup_is_required(false, second.1, 2048));
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
    assert_eq!(s.chain_supplies[&chain], 60);
}

#[test]
fn full_replay_index_can_catch_up_through_a_new_observer_burn() {
    use super::deposit_watch::apply_burn_log_window_and_advance;
    use crate::chains::multi_chain_state::MAX_PENDING_EVM_BURN_REPLAY_IDS;

    let chain = ChainId(10143);
    let mut s = state_with_open_vault(100);
    s.pending_evm_burn_replay_ids.insert(
        (chain, 1),
        (0..MAX_PENDING_EVM_BURN_REPLAY_IDS)
            .map(|n| format!("0xpending{}:0", n))
            .collect(),
    );
    assert!(!s.can_reserve_evm_burn_replay_ids(1));

    // A complete bounded getLogs window can still process a new observer burn
    // and commit its coverage floor atomically, without allocating durable
    // replay-index capacity for logs covered by that same window.
    let logs = vec![(
        vec![BURN_EVENT_TOPIC0.into(), word(1), word(0xdead)],
        word(40),
        "0xnew-observer-burn".into(),
        10,
        0,
    )];
    let applied = apply_burn_log_window_and_advance(&mut s, chain, &logs, 1024)
        .expect("complete observer window applies");
    assert_eq!(applied.len(), 1);

    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
    assert_eq!(s.chain_supplies[&chain], 60);
    assert_eq!(s.evm_burn_proof_floor_by_chain.get(&chain), Some(&1024));
    assert!(s.pending_evm_burn_replay_ids.is_empty());
    assert!(s.can_reserve_evm_burn_replay_ids(1));
}

#[test]
fn deferred_later_burn_rolls_back_window_prefix_then_retry_commits_once() {
    use super::deposit_watch::apply_burn_log_window_and_advance;
    use crate::chains::monad::deposit_watch::BurnApplyError;

    let chain = ChainId(10143);
    let mut s = state_with_open_vault(100);
    let mut deferred = s.chain_vaults[&1].clone();
    deferred.vault_id = 2;
    deferred.debt_e8s = 0;
    s.chain_vaults.insert(2, deferred);
    s.sp_attempted_chain_vaults.insert(2);
    let logs = vec![
        (
            vec![BURN_EVENT_TOPIC0.into(), word(1), word(0xdead)],
            word(40),
            "0xfirst".into(),
            10,
            0,
        ),
        (
            vec![BURN_EVENT_TOPIC0.into(), word(2), word(0xdead)],
            word(1),
            "0xdeferred".into(),
            11,
            1,
        ),
    ];

    assert!(matches!(
        apply_burn_log_window_and_advance(&mut s, chain, &logs, 1024),
        Err(BurnApplyError::DeferredLiquidation)
    ));
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100, "prefix debt rolls back");
    assert_eq!(s.chain_supplies[&chain], 100, "prefix supply rolls back");
    assert_eq!(s.evm_burn_proof_floor_by_chain.get(&chain), None);

    s.sp_attempted_chain_vaults.remove(&2);
    let applied = apply_burn_log_window_and_advance(&mut s, chain, &logs, 1024)
        .expect("retry completes after deferred marker clears");
    assert_eq!(applied.len(), 1, "only the valid first burn applies");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
    assert_eq!(s.chain_supplies[&chain], 60);
    assert_eq!(s.evm_burn_proof_floor_by_chain.get(&chain), Some(&1024));
}

#[test]
fn unscanned_observer_cursor_does_not_reject_later_direct_burn_proof() {
    use super::deposit_watch::advance_cursor_without_burn_coverage;

    let mut s = state_with_open_vault(100);
    let chain = ChainId(10143);
    let receipt = TxReceiptWithLogs {
        tx_hash: None,
        success: true,
        block_number: 10,
        logs: vec![(
            "0xcafe".into(),
            vec![BURN_EVENT_TOPIC0.into(), word(1), word(0xdead)],
            word(40),
            3,
        )],
    };

    // Models the no-logs path used while a mint is in flight or the
    // totalSupply probe fails: settlement may use this finalized cursor, but
    // it does not establish burn-log coverage for blocks 1 through 20.
    advance_cursor_without_burn_coverage(&mut s, chain, 20);
    assert_eq!(s.last_observed_block.get(&chain), Some(&20));
    assert_eq!(s.evm_burn_proof_floor_by_chain.get(&chain), Some(&0));

    let applied = apply_receipt_burns_to_state(&mut s, chain, "0xcafe", "0xtx", &receipt)
        .expect("a finalized proof in an unscanned gap remains admissible");
    assert_eq!(applied.len(), 1);
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
}

#[test]
fn legacy_cursor_history_is_held_as_ambiguous_until_developer_baseline() {
    let mut s = state_with_open_vault(100);
    let chain = ChainId(10143);
    s.last_observed_block.insert(chain, 20);
    let receipt = TxReceiptWithLogs {
        tx_hash: None,
        success: true,
        block_number: 10,
        logs: vec![(
            "0xcafe".into(),
            vec![BURN_EVENT_TOPIC0.into(), word(1), word(0xdead)],
            word(40),
            3,
        )],
    };

    let held = apply_receipt_burns_to_state(&mut s, chain, "0xcafe", "0xtx", &receipt);
    assert_eq!(
        held,
        Err(ApplyBurnsError::LegacyHistoryHeld {
            block: 10,
            held_through: 20,
        })
    );
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100);

    // The developer-only current-tip activation assertion explicitly chooses
    // to exclude this prior history, after which the ordinary stale guard is
    // precise and no longer conflates it with an unknown legacy gap.
    s.accept_evm_burn_proof_baseline(chain, 20);
    assert_eq!(
        apply_receipt_burns_to_state(&mut s, chain, "0xcafe", "0xtx", &receipt),
        Err(ApplyBurnsError::StaleProof {
            block: 10,
            floor: 20,
        })
    );
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100);
}

#[test]
fn rejects_log_from_wrong_contract() {
    let mut s = state_with_open_vault(100);
    let receipt = TxReceiptWithLogs {
        tx_hash: None,
        success: true,
        block_number: 10,
        logs: vec![(
            "0xnotthecontract".to_string(),
            vec![BURN_EVENT_TOPIC0.to_string(), word(1), word(0xdead)],
            word(40),
            0,
        )],
    };
    let applied = apply_receipt_burns_to_state(&mut s, ChainId(10143), "0xcafe", "0xtx", &receipt)
        .expect("apply");
    assert_eq!(applied.len(), 0, "log from a non-icUSD contract is ignored");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100);
}

// ─── F-02 residual: reorg-halted must fail closed (2026-08-22) ──────────────
//
// `run_observer` (deposit_watch.rs) already skips scanning while a chain is
// `reorg_halted`. But the independent `submit_burn_proof` notify path
// (`apply_receipt_burns_to_state`, called synchronously from inside a single
// `mutate_state` closure in `verify_and_apply_burn_proof`) was not gated on
// it at all, so a persistent reorg halt did not stop burns from applying via
// that path. These tests exercise the guard added directly to
// `apply_receipt_burns_to_state`: since that function is always the
// synchronous body of the `mutate_state` closure and never awaits internally,
// checking `reorg_halted` as its first statement IS the "immediately before
// the mutation, no `.await` in between" re-check — there is no separate
// pre-await vs. post-await code path to simulate; calling this function with
// `reorg_halted` set models both "halted before the call" and "halted by the
// time the synchronous apply actually runs" identically.

fn halted_receipt(contract: &str) -> TxReceiptWithLogs {
    TxReceiptWithLogs {
        tx_hash: None,
        success: true,
        block_number: 10,
        logs: vec![(
            contract.to_string(),
            vec![BURN_EVENT_TOPIC0.to_string(), word(1), word(0xdead)],
            word(40),
            0,
        )],
    }
}

#[test]
fn reorg_halted_before_call_is_rejected_with_zero_mutation() {
    // (a) pre-halt: chain already reorg_halted before the call.
    let mut s = state_with_open_vault(100);
    s.reorg_halted.insert(ChainId(10143), true);
    let receipt = halted_receipt("0xcafe");

    let result = apply_receipt_burns_to_state(&mut s, ChainId(10143), "0xcafe", "0xtx", &receipt);

    assert_eq!(result, Err(ApplyBurnsError::ReorgHalted));
    assert_eq!(
        s.processed_burn_keys
            .values()
            .map(|set| set.len())
            .sum::<usize>(),
        0,
        "no processed_burn_key inserted while halted"
    );
    assert_eq!(
        s.chain_vaults[&1].debt_e8s, 100,
        "no debt mutation while halted"
    );
    assert_eq!(
        s.chain_supplies[&ChainId(10143)],
        100,
        "no supply mutation while halted"
    );
}

#[test]
fn reorg_halt_after_receipt_lookup_is_rechecked_before_mutation() {
    // Model the state after receipt/finality awaits: the chain becomes
    // reorg-halted before the synchronous mutate_state closure runs. The first
    // statement in that closure's apply function is the authoritative guard.
    let mut s = state_with_open_vault(100);
    let chain = ChainId(10143);
    s.chain_configs
        .insert(chain, chain_config(ChainStatus::Disabled));
    let receipt = halted_receipt("0xcafe");
    // Simulate the halt arriving during the preceding RPC/finality awaits.
    s.reorg_halted.insert(chain, true);

    let result = apply_receipt_burns_to_state(&mut s, chain, "0xcafe", "0xtx", &receipt);

    assert_eq!(result, Err(ApplyBurnsError::ReorgHalted));
    assert!(
        s.processed_burn_keys.is_empty(),
        "no key inserted when the halt is visible at apply time"
    );
    assert_eq!(
        s.chain_vaults[&1].debt_e8s, 100,
        "no mutation when halted at apply time"
    );
}

#[test]
fn clear_reorg_halt_then_retry_applies_the_same_proof_exactly_once() {
    // (c) halted -> rejected, then clear_reorg_halt -> the SAME proof applies
    // exactly once. `clear_reorg_halt` (main.rs) clears the flag via
    // `BTreeMap::remove`, so mirror that exactly rather than `insert(false)`.
    let mut s = state_with_open_vault(100);
    let contract = "0xcafe";
    let receipt = halted_receipt(contract);

    s.reorg_halted.insert(ChainId(10143), true);
    let rejected = apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt);
    assert_eq!(rejected, Err(ApplyBurnsError::ReorgHalted));
    assert_eq!(
        s.chain_vaults[&1].debt_e8s, 100,
        "still untouched after the rejection"
    );

    s.reorg_halted.remove(&ChainId(10143)); // mirrors clear_reorg_halt

    let applied = apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt)
        .expect("apply after clear");
    assert_eq!(applied.len(), 1, "the same proof now applies");
    assert_eq!(applied[0].amount_e8s, 40);
    assert_eq!(
        s.chain_vaults[&1].debt_e8s, 60,
        "debt decremented exactly once"
    );
    assert_eq!(
        s.processed_burn_keys
            .values()
            .map(|set| set.len())
            .sum::<usize>(),
        1,
        "exactly one key recorded, from the post-clear apply"
    );
}

#[test]
fn resubmitting_after_clear_and_apply_is_a_dedup_noop_no_double_decrement() {
    // (d) idempotency: re-submitting the already-applied proof after the
    // clear is a no-op via the existing processed_burn_keys dedup.
    let mut s = state_with_open_vault(100);
    let contract = "0xcafe";
    let receipt = halted_receipt(contract);

    s.reorg_halted.insert(ChainId(10143), true);
    apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt)
        .expect_err("rejected while halted");
    s.reorg_halted.remove(&ChainId(10143));
    let first = apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt)
        .expect("first apply after clear");
    assert_eq!(first.len(), 1);
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);

    // Re-submit the identical proof again (no further halt in between).
    let second = apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt)
        .expect("second apply is not an error, just a no-op");
    assert_eq!(second.len(), 0, "deduped: nothing newly applied");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60, "no double-decrement");
}

#[test]
fn anonymous_and_multiple_distinct_principals_never_enter_burn_proof_work() {
    use super::burn_proof::{operator_may_submit_burn_proof, run_if_operator_admitted};
    use crate::chains::config::BurnProofAdmissionMode;
    use candid::Principal;
    use std::cell::Cell;

    let operator = Principal::from_slice(&[0x77]);
    let callers = [
        Principal::anonymous(),
        Principal::from_slice(&[0x11]),
        Principal::from_slice(&[0x22]),
        Principal::from_slice(&[0x33]),
    ];
    let calls = Cell::new(0);
    for caller in callers {
        let admitted =
            operator_may_submit_burn_proof(caller, operator, BurnProofAdmissionMode::OperatorOnly);
        assert!(
            run_if_operator_admitted(admitted, || calls.set(calls.get() + 1)).is_none(),
            "caller {caller} must be rejected before lookup work"
        );
    }
    assert_eq!(
        calls.get(),
        0,
        "unauthorized callers trigger no receipt lookup"
    );
}

#[test]
fn only_non_anonymous_operator_reaches_work_and_public_mode_stays_closed() {
    use super::burn_proof::{operator_may_submit_burn_proof, run_if_operator_admitted};
    use crate::chains::config::BurnProofAdmissionMode;
    use candid::Principal;
    use std::cell::Cell;

    let operator = Principal::from_slice(&[0x77]);
    let calls = Cell::new(0);
    assert!(!operator_may_submit_burn_proof(
        operator,
        operator,
        BurnProofAdmissionMode::Public,
    ));
    assert!(run_if_operator_admitted(
        operator_may_submit_burn_proof(operator, operator, BurnProofAdmissionMode::OperatorOnly,),
        || calls.set(calls.get() + 1),
    )
    .is_some());
    assert_eq!(calls.get(), 1, "operator retains recovery access");
    assert!(!operator_may_submit_burn_proof(
        Principal::anonymous(),
        Principal::anonymous(),
        BurnProofAdmissionMode::OperatorOnly,
    ));
}
