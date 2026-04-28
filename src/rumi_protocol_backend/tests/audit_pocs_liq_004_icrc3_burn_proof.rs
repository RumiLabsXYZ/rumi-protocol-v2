//! LIQ-004 regression fence: ICRC-3 burn / transfer proof verification for
//! SP-triggered writedowns.
//!
//! Audit report:
//!   `audit-reports/2026-04-22-28e9896/raw-pass-results/liquidation-mechanics.json`
//!   finding LIQ-004.
//!
//! # What the gap was
//!
//! `liquidate_vault_debt_already_burned` (in `vault.rs`) intentionally omits
//! the `ratio < min_liq_ratio` check (the "NO CR CHECK HERE" comment). The
//! trust boundary was a single principal compare:
//! `caller == stability_pool_canister`. There was no independent verification
//! that:
//!   (a) the icUSD was actually burned (legacy path), or
//!   (b) the 3USD was actually transferred to the backend's reserves
//!       subaccount (reserves path).
//!
//! Three failure modes the gap enables:
//!   1. SP bug double-counts a 3pool burn -> backend writes down debt
//!      without the corresponding icUSD destruction.
//!   2. SP is upgraded to buggy code -> same outcome.
//!   3. `stability_pool_canister` is rotated to a malicious / compromised
//!      principal -> drain healthy vaults one writedown at a time.
//!
//! # How this file tests the fix
//!
//! Wave-8c adds defense-in-depth via:
//!   * `icrc3_proof::SpWritedownProof` — typed argument the SP passes that
//!     points at a real ICRC-3 block on the relevant ledger.
//!   * `icrc3_proof::decode_block` + `validate_block` — pure-logic verifier
//!     that asserts the block matches expected memo, amount, and accounts.
//!   * `State::sp_writedown_disabled` — admin kill switch (independent of
//!     `liquidation_frozen` and `frozen`).
//!   * `State::consumed_writedown_proofs` — replay defense set keyed by
//!     `(SpProofLedger, block_index)`.
//!
//! Layer 1 (this file): pure verification logic on the public helpers.
//!   * Memo round-trip / negative cases.
//!   * Burn / transfer block decode against both block formats (standard
//!     ic-icrc1-ledger top-level `btype` and 3pool `tx.op`).
//!   * `validate_block` accepts correct shapes and rejects every variant of
//!     wrong-amount, wrong-kind, wrong-from, wrong-to, wrong-memo.
//!
//! Layer 2 (this file): state-level fence on the kill switch and the
//! consumed-proof set.
//!   * `sp_writedown_disabled` defaults false, round-trips through CBOR,
//!     is independent of `liquidation_frozen` and `frozen`.
//!   * `consumed_writedown_proofs` round-trips through CBOR (so replay
//!     defense survives canister upgrades).
//!
//! # Why no dedicated PocketIC file in Phase 1
//!
//! Phase-1 of LIQ-004 keeps the proof argument Optional and `None` is
//! accepted with a WARN log so the existing SP can keep calling. The fix
//! is the *infrastructure* (types, decoder, validator, kill switch, replay
//! defense), not yet the enforcement. Three orthogonal checks cover the
//! Phase-1 surface:
//!
//!   1. The Rust compiler enforces the wiring at every call site —
//!      `vault::liquidate_vault_debt_already_burned` takes
//!      `proof: Option<SpWritedownProof>` and the entry points in
//!      `main.rs` thread it through.
//!   2. Layer 1 here covers `decode_block` + `validate_block` against
//!      both the standard ic-icrc1-ledger format (top-level `btype`) and
//!      the in-tree rumi_3pool format (`tx.op`).
//!   3. Layer 2 here covers the state machinery (kill switch +
//!      consumed-proof set serde/round-trip).
//!
//! The remaining surface is the IC inter-canister call boilerplate
//! (`ic_cdk::call(ledger, "icrc3_get_blocks", ...)`), which is well-trodden
//! and exercised by every other ledger-touching path in this canister
//! (e.g., Wave-3 idempotent transfers, ICRC-2 transfer_from, sweep_deposit).
//!
//! Wave 8d (Phase 2 — flips Optional to required) is the right time for a
//! PocketIC fence: when the proof becomes mandatory, the integration is
//! load-bearing and worth the per-test setup cost. This file documents
//! that boundary explicitly so the regression suite stays honest about
//! what Phase 1 does and does not cover.

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;

use rumi_protocol_backend::icrc3_proof::{
    decode_block, decode_writedown_memo, encode_writedown_memo, make_test_block_without_memo,
    make_test_burn_block, make_test_transfer_block, validate_block, ProofExpectations,
    SpProofLedger,
};
use rumi_protocol_backend::state::State;
use rumi_protocol_backend::InitArg;

// ─── Test fixtures ─────────────────────────────────────────────────────────

fn sp_principal() -> Principal {
    Principal::from_slice(&[0xaa; 29])
}

fn other_principal() -> Principal {
    Principal::from_slice(&[0xbb; 29])
}

fn backend_principal() -> Principal {
    Principal::from_slice(&[0x01; 29])
}

fn reserves_account() -> Account {
    Account {
        owner: backend_principal(),
        subaccount: Some([0x42; 32]),
    }
}

fn sp_account() -> Account {
    Account {
        owner: sp_principal(),
        subaccount: None,
    }
}

fn burn_expectations(vault_id: u64, amount: u64) -> ProofExpectations {
    ProofExpectations {
        ledger_kind: SpProofLedger::IcusdBurn,
        expected_amount_e8s: amount,
        sp_principal: sp_principal(),
        reserves_account: reserves_account(),
        vault_id_memo: vault_id,
    }
}

fn transfer_expectations(vault_id: u64, amount: u64) -> ProofExpectations {
    ProofExpectations {
        ledger_kind: SpProofLedger::ThreePoolTransfer,
        expected_amount_e8s: amount,
        sp_principal: sp_principal(),
        reserves_account: reserves_account(),
        vault_id_memo: vault_id,
    }
}

fn fresh_state() -> State {
    State::from(InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: Principal::anonymous(),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    })
}

// ============================================================================
// Layer 1 — pure verification logic
// ============================================================================

#[test]
fn liq_004_writedown_memo_round_trip() {
    for vid in [0u64, 1, 7, 12345, u64::MAX] {
        let memo = encode_writedown_memo(vid);
        assert_eq!(memo.len(), 21, "memo length must be prefix(13) + u64(8)");
        assert_eq!(decode_writedown_memo(&memo).unwrap(), vid);
    }
}

#[test]
fn liq_004_writedown_memo_rejects_wrong_prefix() {
    let mut memo = encode_writedown_memo(7);
    memo[0] = b'X';
    assert!(
        decode_writedown_memo(&memo).is_err(),
        "memos without RUMI-LIQ-004 prefix must be rejected"
    );
}

#[test]
fn liq_004_writedown_memo_rejects_wrong_length() {
    let memo = b"RUMI-LIQ-004:";
    assert!(decode_writedown_memo(memo).is_err());
    let memo = b"RUMI-LIQ-004:\x00\x00\x00\x00\x00\x00\x00";
    assert!(
        decode_writedown_memo(memo).is_err(),
        "memos shorter than prefix+8 must be rejected"
    );
}

#[test]
fn liq_004_decode_block_accepts_3pool_format() {
    // 3pool ledger embeds op inside tx (no top-level btype).
    let memo = encode_writedown_memo(42);
    let value = make_test_burn_block(sp_account(), 10_000, &memo, false);
    let decoded = decode_block(&value).expect("decode 3pool-format burn");
    assert_eq!(decoded.op, "burn");
    assert_eq!(decoded.amount, 10_000);
    assert_eq!(decoded.from.unwrap().owner, sp_principal());
    assert_eq!(decoded.memo.unwrap(), memo);
}

#[test]
fn liq_004_decode_block_accepts_standard_btype_format() {
    // Standard ic-icrc1-ledger emits `btype = "1burn"` at top level. The
    // decoder must strip the schema prefix so downstream validation sees
    // `op == "burn"`.
    let memo = encode_writedown_memo(7);
    let value = make_test_burn_block(sp_account(), 100, &memo, true);
    let decoded = decode_block(&value).expect("decode standard-format burn");
    assert_eq!(
        decoded.op, "burn",
        "btype `1burn` must normalize to `burn` so the verifier matches both ledgers"
    );
}

#[test]
fn liq_004_proof_with_correct_burn_block_passes() {
    let memo = encode_writedown_memo(99);
    let block =
        decode_block(&make_test_burn_block(sp_account(), 5_000, &memo, false)).unwrap();
    let exp = burn_expectations(99, 5_000);
    assert_eq!(
        validate_block(&block, &exp).expect("valid burn proof"),
        99,
        "validator must return the vault id from the memo"
    );
}

#[test]
fn liq_004_proof_with_correct_transfer_block_passes() {
    let memo = encode_writedown_memo(7);
    let block = decode_block(&make_test_transfer_block(
        sp_account(),
        reserves_account(),
        2_500,
        &memo,
        false,
    ))
    .unwrap();
    let exp = transfer_expectations(7, 2_500);
    assert_eq!(validate_block(&block, &exp).unwrap(), 7);
}

#[test]
fn liq_004_proof_with_wrong_amount_rejected() {
    let memo = encode_writedown_memo(1);
    let block =
        decode_block(&make_test_burn_block(sp_account(), 5_000, &memo, false)).unwrap();
    let exp = burn_expectations(1, 6_000);
    assert!(
        validate_block(&block, &exp).is_err(),
        "amount mismatch must be rejected"
    );
}

#[test]
fn liq_004_proof_with_wrong_kind_rejected() {
    // Transfer block validated against burn expectations.
    let memo = encode_writedown_memo(1);
    let block = decode_block(&make_test_transfer_block(
        sp_account(),
        reserves_account(),
        500,
        &memo,
        false,
    ))
    .unwrap();
    let exp = burn_expectations(1, 500);
    assert!(
        validate_block(&block, &exp).is_err(),
        "transfer block must not satisfy burn expectations"
    );

    // And the reverse: a burn block validated against transfer expectations.
    let burn_block =
        decode_block(&make_test_burn_block(sp_account(), 500, &memo, false)).unwrap();
    let xfer_exp = transfer_expectations(1, 500);
    assert!(
        validate_block(&burn_block, &xfer_exp).is_err(),
        "burn block must not satisfy transfer expectations"
    );
}

#[test]
fn liq_004_proof_with_wrong_from_rejected() {
    let memo = encode_writedown_memo(1);
    let imposter = Account {
        owner: other_principal(),
        subaccount: None,
    };
    let block =
        decode_block(&make_test_burn_block(imposter, 500, &memo, false)).unwrap();
    let exp = burn_expectations(1, 500);
    assert!(
        validate_block(&block, &exp).is_err(),
        "burn from a non-SP principal must be rejected even with valid memo"
    );
}

#[test]
fn liq_004_proof_with_wrong_to_rejected_on_transfer() {
    let memo = encode_writedown_memo(1);
    let bad_to = Account {
        owner: Principal::from_slice(&[0xee; 29]),
        subaccount: None,
    };
    let block = decode_block(&make_test_transfer_block(
        sp_account(),
        bad_to,
        500,
        &memo,
        false,
    ))
    .unwrap();
    let exp = transfer_expectations(1, 500);
    assert!(
        validate_block(&block, &exp).is_err(),
        "transfer to a non-reserves account must be rejected"
    );
}

#[test]
fn liq_004_proof_with_wrong_to_subaccount_rejected_on_transfer() {
    // Same `to.owner` as expected (backend principal) but a different
    // subaccount. This catches a transfer landing in a wrong protocol
    // subaccount.
    let memo = encode_writedown_memo(1);
    let bad_to = Account {
        owner: backend_principal(),
        subaccount: Some([0x99; 32]),
    };
    let block = decode_block(&make_test_transfer_block(
        sp_account(),
        bad_to,
        500,
        &memo,
        false,
    ))
    .unwrap();
    let exp = transfer_expectations(1, 500);
    assert!(
        validate_block(&block, &exp).is_err(),
        "transfer to wrong subaccount on backend must still be rejected"
    );
}

#[test]
fn liq_004_proof_with_wrong_memo_vault_id_rejected() {
    // Memo encodes vault 7 but the call is on vault 8. Cross-vault replay
    // must be impossible.
    let memo = encode_writedown_memo(7);
    let block =
        decode_block(&make_test_burn_block(sp_account(), 500, &memo, false)).unwrap();
    let exp = burn_expectations(8, 500);
    assert!(
        validate_block(&block, &exp).is_err(),
        "memo binding to wrong vault id must reject"
    );
}

#[test]
fn liq_004_proof_with_missing_memo_rejected() {
    let block = decode_block(&make_test_block_without_memo(
        "burn",
        sp_account(),
        None,
        500,
    ))
    .unwrap();
    let exp = burn_expectations(1, 500);
    assert!(
        validate_block(&block, &exp).is_err(),
        "burns without a Wave-8c memo must be rejected"
    );
}

// ============================================================================
// Layer 2a — admin kill switch (sp_writedown_disabled)
// ============================================================================

#[test]
fn liq_004_kill_switch_default_off() {
    let state = fresh_state();
    assert!(
        !state.sp_writedown_disabled,
        "fresh state must NOT have SP writedowns disabled"
    );
}

#[test]
fn liq_004_kill_switch_round_trips_through_cbor() {
    let mut state = fresh_state();
    state.sp_writedown_disabled = true;

    let mut buf = Vec::new();
    ciborium::ser::into_writer(&state, &mut buf).expect("encode state");
    let restored: State = ciborium::de::from_reader(buf.as_slice()).expect("decode state");

    assert!(
        restored.sp_writedown_disabled,
        "sp_writedown_disabled=true must survive an upgrade round-trip"
    );
}

#[test]
fn liq_004_kill_switch_independent_of_liquidation_frozen() {
    let mut state = fresh_state();
    assert!(!state.sp_writedown_disabled);
    assert!(!state.liquidation_frozen);

    // Flip sp_writedown_disabled — liquidation_frozen must NOT change.
    state.sp_writedown_disabled = true;
    assert!(
        !state.liquidation_frozen,
        "flipping sp_writedown_disabled must not flip liquidation_frozen"
    );

    // Flip liquidation_frozen — sp_writedown_disabled must stay set.
    state.liquidation_frozen = true;
    assert!(
        state.sp_writedown_disabled,
        "flipping liquidation_frozen must not clear sp_writedown_disabled"
    );

    // The two are orthogonal kill switches.
    state.sp_writedown_disabled = false;
    assert!(
        state.liquidation_frozen,
        "flipping sp_writedown_disabled off must not flip liquidation_frozen off"
    );
}

#[test]
fn liq_004_kill_switch_independent_of_global_frozen() {
    let mut state = fresh_state();
    state.sp_writedown_disabled = true;
    state.frozen = true;
    state.frozen = false;
    assert!(
        state.sp_writedown_disabled,
        "global unfreeze must not clear sp_writedown_disabled"
    );
}

// ============================================================================
// Layer 2b — replay defense (consumed_writedown_proofs)
// ============================================================================

#[test]
fn liq_004_consumed_proofs_default_empty() {
    let state = fresh_state();
    assert!(
        state.consumed_writedown_proofs.is_empty(),
        "fresh state must have no consumed proofs"
    );
}

#[test]
fn liq_004_consumed_proofs_round_trip_through_cbor() {
    // Replay defense survives canister upgrades.
    let mut state = fresh_state();
    state
        .consumed_writedown_proofs
        .insert((SpProofLedger::IcusdBurn, 17));
    state
        .consumed_writedown_proofs
        .insert((SpProofLedger::ThreePoolTransfer, 42));

    let mut buf = Vec::new();
    ciborium::ser::into_writer(&state, &mut buf).expect("encode state");
    let restored: State = ciborium::de::from_reader(buf.as_slice()).expect("decode state");

    assert_eq!(restored.consumed_writedown_proofs.len(), 2);
    assert!(restored
        .consumed_writedown_proofs
        .contains(&(SpProofLedger::IcusdBurn, 17)));
    assert!(restored
        .consumed_writedown_proofs
        .contains(&(SpProofLedger::ThreePoolTransfer, 42)));
}

#[test]
fn liq_004_consumed_proofs_distinguish_ledger_kinds() {
    // The same block index on different ledgers must NOT collide. A burn
    // block 17 on icUSD is different from transfer block 17 on 3USD.
    let mut state = fresh_state();
    state
        .consumed_writedown_proofs
        .insert((SpProofLedger::IcusdBurn, 17));

    assert!(state
        .consumed_writedown_proofs
        .contains(&(SpProofLedger::IcusdBurn, 17)));
    assert!(
        !state
            .consumed_writedown_proofs
            .contains(&(SpProofLedger::ThreePoolTransfer, 17)),
        "block 17 on a different ledger must NOT be considered consumed"
    );
}
