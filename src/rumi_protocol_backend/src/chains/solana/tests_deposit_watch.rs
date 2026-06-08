//! Pure tests for the Solana observer (Task 7), mirroring
//! `chains::monad::tests_deposit_watch`.
//!
//! Two surfaces are covered:
//!
//! 1. The Solana deposit-watch state transition, exercised through the SHARED
//!    `chains::vault::verify_deposit_and_enqueue_mint_in_state` helper (the same
//!    function `run_observer` calls after its `get_balance` await). We open a
//!    Solana vault in `AwaitingDeposit` via `open_chain_vault_in_state` with the
//!    Solana seams (`is_valid_solana_address`, `"SOL"`), then assert it flips to
//!    `MintPending` + enqueues exactly one `Mint` when the observed lamports
//!    cover the declared collateral, and is a no-op below it. This documents the
//!    Solana deposit path; the helper itself is shared and exhaustively covered
//!    by the Monad/`chains::tests_vault` suites, so this stays concise.
//!
//! 2. The `supply_drop_detected` M2 backstop predicate.

use super::config::SOLANA_CHAIN_ID;
use super::deposit_watch::supply_drop_detected;
use super::ted25519::{is_valid_solana_address, solana_address_from_pubkey};
use crate::chains::config::{GasStrategy, RegisterChainArg};
use crate::chains::multi_chain_state::MultiChainStateV4;
use crate::chains::settlement_queue::{SettlementOpKind, SettlementOpStatus};
use crate::chains::vault::{
    open_chain_vault_in_state, verify_deposit_and_enqueue_mint_in_state, ChainVaultStatus,
};
use candid::Principal;

/// $150.00 as e8 (a plausible SOL price).
const PRICE_150_USD_E8: u64 = 150_0000_0000;
/// 1 SOL in lamports (9 decimals), Solana's NATIVE base unit.
const ONE_SOL_LAMPORTS: u128 = 1_000_000_000;
/// 100 icUSD intended mint (8 decimals == e8s).
const HUNDRED_ICUSD_E8S: u128 = 100_00000000;

/// A guaranteed-valid base58 Solana address (32 bytes), built from the address
/// encoder rather than a hardcoded magic string (so the test cannot drift if the
/// validator's rules change). Distinct byte patterns give distinct addresses.
fn valid_solana_address(byte: u8) -> String {
    solana_address_from_pubkey(&[byte; 32]).expect("32-byte pubkey encodes")
}

/// Register Solana (chain 501) with 9-decimal native units and a manual `"SOL"`
/// price, then open an `AwaitingDeposit` vault (id 7) declaring `declared_lamports`
/// collateral and `HUNDRED_ICUSD_E8S` debt. Returns the seeded state.
fn seeded_awaiting_deposit(declared_lamports: u128) -> MultiChainStateV4 {
    let mut s = MultiChainStateV4::default();
    let arg = RegisterChainArg {
        chain_id: SOLANA_CHAIN_ID,
        display_name: "SolanaDevnet".into(),
        // register_chain_in_state requires at least one URL; the value is
        // irrelevant to these pure-state tests (no RPC is ever made).
        rpc_endpoints: vec!["https://rpc".into()],
        finality_depth: 0,
        gas_strategy: GasStrategy::SolanaPriorityFee {
            lamports_per_cu_ceiling: 10_000,
        },
        chain_native_decimals: 9,
        min_quorum_providers: None,
    };
    crate::chains::admin::register_chain_in_state(&mut s, arg, 0).expect("register chain");
    s.manual_prices
        .insert((SOLANA_CHAIN_ID, "SOL".into()), PRICE_150_USD_E8);

    let mint_recipient = valid_solana_address(7);
    open_chain_vault_in_state(
        &mut s,
        SOLANA_CHAIN_ID,
        Principal::anonymous(),
        valid_solana_address(1), // custody (never validated by the helper)
        declared_lamports,
        HUNDRED_ICUSD_E8S,
        mint_recipient.clone(),
        is_valid_solana_address, // the REAL Solana validator seam
        "SOL",                   // the Solana native-asset price symbol
        super::config::SOLANA_MIN_CR_E4,
        12345,
        7,
    )
    .expect("open AwaitingDeposit Solana vault");

    // Sanity: it opened in AwaitingDeposit with the declared collateral + intended
    // mint, and enqueued NOTHING (open-then-verify).
    let v = s.chain_vaults.get(&7).expect("vault 7 created");
    assert_eq!(v.status, ChainVaultStatus::AwaitingDeposit);
    assert_eq!(v.collateral_amount_native, declared_lamports);
    assert_eq!(v.pending_mint_e8s, HUNDRED_ICUSD_E8S);
    assert_eq!(v.debt_e8s, 0);
    assert!(
        s.settlement_queues
            .get(&SOLANA_CHAIN_ID)
            .map(|q| q.pending_len())
            .unwrap_or(0)
            == 0,
        "open must enqueue nothing"
    );
    s
}

// ─── Deposit-watch transition (via the shared helper run_observer calls) ─────

#[test]
fn deposit_watch_flips_to_mintpending_and_enqueues_mint_when_balance_covers_declared() {
    // Declare 100 SOL collateral. The observer would call get_balance, then this
    // helper with the observed lamports. 100 SOL observed (== declared) covers it.
    let declared = 100 * ONE_SOL_LAMPORTS;
    let mut s = seeded_awaiting_deposit(declared);

    let observed = declared; // exactly covers (>= is the gate)
    let transitioned = verify_deposit_and_enqueue_mint_in_state(&mut s, 7, observed, 999);
    assert_eq!(transitioned, Ok(true), "balance covers declared -> transition + enqueue");

    let v = s.chain_vaults.get(&7).expect("vault present");
    assert_eq!(v.status, ChainVaultStatus::MintPending, "flipped to MintPending");

    // Exactly one Mint op enqueued, for this vault and the intended amount.
    let q = s
        .settlement_queues
        .get(&SOLANA_CHAIN_ID)
        .expect("queue exists");
    assert_eq!(q.pending_len(), 1, "exactly one op enqueued");
    let op = q.pending.values().next().expect("the enqueued op");
    assert!(matches!(op.status, SettlementOpStatus::Queued), "op is Queued");
    match &op.kind {
        SettlementOpKind::Mint { recipient, amount_e8s, vault_id } => {
            assert_eq!(*vault_id, 7);
            assert_eq!(*amount_e8s, HUNDRED_ICUSD_E8S);
            assert_eq!(recipient, &valid_solana_address(7));
        }
        other => panic!("expected a Mint op, got {other:?}"),
    }
}

#[test]
fn deposit_watch_is_noop_when_balance_below_declared() {
    let declared = 100 * ONE_SOL_LAMPORTS;
    let mut s = seeded_awaiting_deposit(declared);

    // Observed strictly BELOW declared -> no transition, no enqueue, no mutation.
    let observed = declared - 1;
    let res = verify_deposit_and_enqueue_mint_in_state(&mut s, 7, observed, 999);
    assert_eq!(res, Ok(false), "below declared -> no-op");

    let v = s.chain_vaults.get(&7).expect("vault present");
    assert_eq!(v.status, ChainVaultStatus::AwaitingDeposit, "stays AwaitingDeposit");
    assert_eq!(
        s.settlement_queues
            .get(&SOLANA_CHAIN_ID)
            .map(|q| q.pending_len())
            .unwrap_or(0),
        0,
        "nothing enqueued"
    );
}

#[test]
fn deposit_watch_second_call_is_idempotent() {
    // After a successful transition, a second observe (the next observer tick)
    // is a no-op (status is no longer AwaitingDeposit) -> never double-enqueues.
    let declared = 50 * ONE_SOL_LAMPORTS;
    let mut s = seeded_awaiting_deposit(declared);

    assert_eq!(
        verify_deposit_and_enqueue_mint_in_state(&mut s, 7, declared, 1),
        Ok(true)
    );
    // Second call with an even larger observed balance: still a no-op.
    assert_eq!(
        verify_deposit_and_enqueue_mint_in_state(&mut s, 7, declared * 2, 2),
        Ok(false)
    );
    assert_eq!(
        s.settlement_queues
            .get(&SOLANA_CHAIN_ID)
            .map(|q| q.pending_len())
            .unwrap_or(0),
        1,
        "still exactly one Mint op (no double-enqueue)"
    );
}

// ─── supply_drop_detected (M2 backstop predicate) ────────────────────────────

#[test]
fn supply_drop_detected_cases() {
    // onchain == recorded -> in sync, no burn -> false.
    assert!(!supply_drop_detected(1_000, 1_000, false));
    // onchain < recorded && no mint in flight -> an unsubmitted burn -> true.
    assert!(supply_drop_detected(900, 1_000, false));
    // onchain < recorded BUT a mint is in flight -> stay cheap (M3 reconciles) -> false.
    assert!(!supply_drop_detected(900, 1_000, true));
    // onchain > recorded -> a mint EXCESS (false-negative mint landed, never
    // credited), NOT a burn -> false.
    assert!(!supply_drop_detected(1_100, 1_000, false));
    // Equal + in-flight -> false.
    assert!(!supply_drop_detected(1_000, 1_000, true));
    // Zero/zero -> false.
    assert!(!supply_drop_detected(0, 0, false));
}
