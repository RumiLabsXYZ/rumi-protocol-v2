use crate::chains::config::ChainId;
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
            pending_liquidation: None,        },
    );
    s
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
    let applied =
        apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt)
            .expect("apply");
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].vault_id, 1);
    assert_eq!(applied[0].amount_e8s, 40);
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
    // Re-apply same receipt → deduped, no change.
    let again =
        apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt)
            .expect("apply again");
    assert_eq!(again.len(), 0);
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
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
    let applied =
        apply_receipt_burns_to_state(&mut s, ChainId(10143), "0xcafe", "0xtx", &receipt)
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

    let result =
        apply_receipt_burns_to_state(&mut s, ChainId(10143), "0xcafe", "0xtx", &receipt);

    assert_eq!(result, Err(ApplyBurnsError::ReorgHalted));
    assert_eq!(
        s.processed_burn_keys.values().map(|set| set.len()).sum::<usize>(),
        0,
        "no processed_burn_key inserted while halted"
    );
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100, "no debt mutation while halted");
    assert_eq!(
        s.chain_supplies[&ChainId(10143)],
        100,
        "no supply mutation while halted"
    );
}

#[test]
fn reorg_halted_set_immediately_before_the_synchronous_apply_is_also_refused() {
    // (b) halt-during-await-equivalent: the guard is the first statement of
    // the synchronous fn that `mutate_state` invokes, so driving the guard
    // directly (rather than through the async `verify_and_apply_burn_proof`
    // wrapper, which would need a live RPC canister to reach this point) is
    // the deterministic way to prove a halt raised anywhere before this
    // synchronous call refuses the apply. This models a halt landing between
    // the finality check and the apply: whatever set the flag, the guard
    // sees it and refuses before touching state.
    let mut s = state_with_open_vault(100);
    let receipt = halted_receipt("0xcafe");
    // Simulate the halt arriving "during the preceding awaits", i.e. it is
    // already visible by the time the synchronous mutate_state body runs.
    s.reorg_halted.insert(ChainId(10143), true);

    let result =
        apply_receipt_burns_to_state(&mut s, ChainId(10143), "0xcafe", "0xtx", &receipt);

    assert_eq!(result, Err(ApplyBurnsError::ReorgHalted));
    assert!(
        s.processed_burn_keys.is_empty(),
        "no key inserted when the halt is visible at apply time"
    );
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100, "no mutation when halted at apply time");
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
    let rejected =
        apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt);
    assert_eq!(rejected, Err(ApplyBurnsError::ReorgHalted));
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100, "still untouched after the rejection");

    s.reorg_halted.remove(&ChainId(10143)); // mirrors clear_reorg_halt

    let applied =
        apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt)
            .expect("apply after clear");
    assert_eq!(applied.len(), 1, "the same proof now applies");
    assert_eq!(applied[0].amount_e8s, 40);
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60, "debt decremented exactly once");
    assert_eq!(
        s.processed_burn_keys.values().map(|set| set.len()).sum::<usize>(),
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
    let first =
        apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt)
            .expect("first apply after clear");
    assert_eq!(first.len(), 1);
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);

    // Re-submit the identical proof again (no further halt in between).
    let second =
        apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt)
            .expect("second apply is not an error, just a no-op");
    assert_eq!(second.len(), 0, "deduped: nothing newly applied");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60, "no double-decrement");
}
