//! Task 13: withdraw + close pure-helper tests.
//!
//! `withdraw_collateral_in_state` is the CDP collateral-out path. It CR-checks
//! the REMAINING collateral against `min_cr_e4` (skipping the check entirely
//! when the vault is debt-free), RESERVES the withdrawn collateral by
//! decrementing `collateral_amount_native` at enqueue time (so a second withdraw
//! cannot double-spend the same collateral before the first confirms), and
//! enqueues a `NativeWithdrawal` op carrying the real `vault_id`. A vault that
//! becomes empty AND is debt-free flips to `Closing` (the worker flips it to
//! `Closed` once the on-chain transfer confirms).
//!
//! `close_chain_vault_in_state` is debt-free full withdrawal: it requires
//! `debt_e8s == 0` (`HasDebt` otherwise), then withdraws the full remaining
//! collateral via the same helper.
//!
//! Mutation ordering (no-mutation-on-rejection): validate -> enqueue (can fail
//! -> `QueueError`, no collateral mutation) -> decrement collateral + set
//! `Closing`. A rejected enqueue leaves the vault untouched.

use super::chain_vault::{
    close_chain_vault_in_state, open_chain_vault_in_state, withdraw_collateral_in_state,
    ChainVaultStatus, WithdrawError,
};
use crate::chains::config::{ChainId, GasStrategy, RegisterChainArg};
use crate::chains::multi_chain_state::MultiChainStateV5;
use crate::chains::settlement_queue::SettlementOpKind;
use candid::Principal;

const CHAIN: ChainId = ChainId(10143);
/// USD price as e8: $100.00.
const PRICE_100_USD_E8: u64 = 100_0000_0000;
/// 1 MON in e18.
const ONE_MON_E18: u128 = 1_000_000_000_000_000_000;
/// 130.00% min CR (matches `MONAD_MIN_CR_E4`).
const MIN_CR_E4: u64 = 13_000;

/// Register chain 10143 and set its manual MON price (mirrors `tests_open_vault`).
fn setup(price_e8: u64) -> MultiChainStateV5 {
    let mut s = MultiChainStateV5::default();
    let arg = RegisterChainArg {
        chain_id: CHAIN,
        display_name: "MonadTestnet".into(),
        rpc_endpoints: vec!["https://rpc".into()],
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 2,
            max_fee_gwei_ceiling: 500,
        },
        chain_native_decimals: 18,
        min_quorum_providers: None,
    };
    crate::chains::admin::register_chain_in_state(&mut s, arg, 0).expect("register chain");
    s.manual_prices.insert((CHAIN, "MON".into()), price_e8);
    s
}

fn owner() -> Principal {
    Principal::anonymous()
}

/// Open a vault at `collateral_e18` / `debt_e8s` and drive it to the `Open`
/// state with confirmed debt, so the withdraw tests exercise a live vault. (The
/// open helper records `pending_mint_e8s` and `debt_e8s == 0`; we set the
/// fields directly here to skip the deposit-watch + settlement round trip.)
fn open_and_fund(
    s: &mut MultiChainStateV5,
    vault_id: u64,
    collateral_e18: u128,
    debt_e8s: u128,
) {
    // Use a debt large enough that the open CR check passes regardless, then
    // overwrite the fields to the desired live shape.
    open_chain_vault_in_state(
        s,
        CHAIN,
        owner(),
        "0xcustody".into(),
        collateral_e18,
        // Open requires non-zero debt; pass a token amount, then overwrite.
        1,
        "0x000000000000000000000000000000000000c0de".into(),
        0, // min_cr 0 so open always succeeds for setup
        0,
        vault_id,
    )
    .expect("open for setup");
    let v = s.chain_vaults.get_mut(&vault_id).expect("vault present");
    v.debt_e8s = debt_e8s;
    v.pending_mint_e8s = 0;
    v.status = ChainVaultStatus::Open;
}

// 1. full withdraw of a debt-free vault closes it (Closing) + enqueues one
//    NativeWithdrawal carrying the real vault_id and the e18 amount.
#[test]
fn full_withdraw_when_debt_free_sets_closing_and_enqueues() {
    let mut s = setup(PRICE_100_USD_E8);
    open_and_fund(&mut s, 7, 5 * ONE_MON_E18, 0); // 5 MON, debt 0

    let res = withdraw_collateral_in_state(
        &mut s,
        7,
        5 * ONE_MON_E18, // withdraw the full 5 MON
        "0x000000000000000000000000000000000000dead".into(),
        MIN_CR_E4,
        555,
    );
    assert!(res.is_ok(), "full debt-free withdraw should succeed: {res:?}");

    let v = s.chain_vaults.get(&7).expect("vault present");
    assert_eq!(v.collateral_amount_native, 0, "collateral fully reserved out");
    assert!(
        matches!(v.status, ChainVaultStatus::Closing),
        "empty + debt-free vault flips to Closing, got {:?}",
        v.status
    );

    let q = &s.settlement_queues[&CHAIN];
    assert_eq!(q.pending_len(), 1, "exactly one NativeWithdrawal enqueued");
    let op = q.pending.values().next().expect("op present");
    match &op.kind {
        SettlementOpKind::NativeWithdrawal { recipient, amount_e18, vault_id } => {
            assert_eq!(recipient, "0x000000000000000000000000000000000000dead");
            assert_eq!(*amount_e18, 5 * ONE_MON_E18);
            assert_eq!(*vault_id, 7, "op carries the real vault_id");
        }
        other => panic!("expected NativeWithdrawal op, got {other:?}"),
    }
    assert_eq!(op.idempotency_key, format!("withdraw-{}-{}-{}", CHAIN.0, 7, 555));
}

// 2. partial withdraw keeping CR above min stays Open.
#[test]
fn partial_withdraw_keeping_cr_above_min_is_allowed() {
    let mut s = setup(PRICE_100_USD_E8);
    // debt 100 icUSD ($100), price $100/MON, 5 MON ($500).
    open_and_fund(&mut s, 7, 5 * ONE_MON_E18, 100_00000000);

    // withdraw 1 MON -> 4 MON ($400) / $100 = 400% CR -> well above 130%.
    let res =
        withdraw_collateral_in_state(&mut s, 7, ONE_MON_E18, "0x000000000000000000000000000000000000dead".into(), MIN_CR_E4, 100);
    assert!(res.is_ok(), "partial withdraw above min CR should succeed: {res:?}");

    let v = s.chain_vaults.get(&7).expect("vault present");
    assert_eq!(v.collateral_amount_native, 4 * ONE_MON_E18, "1 MON reserved out");
    assert!(
        matches!(v.status, ChainVaultStatus::Open),
        "vault with remaining debt + collateral stays Open, got {:?}",
        v.status
    );
    assert_eq!(s.settlement_queues[&CHAIN].pending_len(), 1, "one withdrawal enqueued");
}

// 3. withdraw that would break min CR is rejected, no mutation, no enqueue.
#[test]
fn withdraw_breaking_min_cr_is_rejected() {
    let mut s = setup(PRICE_100_USD_E8);
    // debt 100 icUSD ($100), 5 MON ($500).
    open_and_fund(&mut s, 7, 5 * ONE_MON_E18, 100_00000000);

    // withdraw 4.9 MON -> 0.1 MON ($10) left -> CR 10% < 130% -> reject.
    let withdraw_amt = 4 * ONE_MON_E18 + ONE_MON_E18 * 9 / 10; // 4.9 MON
    let res =
        withdraw_collateral_in_state(&mut s, 7, withdraw_amt, "0x000000000000000000000000000000000000dead".into(), MIN_CR_E4, 0);
    assert!(matches!(res, Err(WithdrawError::BelowMinCr { .. })), "got {res:?}");

    let v = s.chain_vaults.get(&7).expect("vault present");
    assert_eq!(v.collateral_amount_native, 5 * ONE_MON_E18, "collateral unchanged on reject");
    assert!(matches!(v.status, ChainVaultStatus::Open), "status unchanged");
    assert_eq!(s.settlement_queues[&CHAIN].pending_len(), 0, "queue must stay empty");
}

// 4. withdraw exceeding balance is rejected, unchanged.
#[test]
fn withdraw_exceeding_balance_is_rejected() {
    let mut s = setup(PRICE_100_USD_E8);
    open_and_fund(&mut s, 7, ONE_MON_E18, 0); // 1 MON, debt-free

    let res = withdraw_collateral_in_state(
        &mut s,
        7,
        2 * ONE_MON_E18, // withdraw 2 MON > 1 MON balance
        "0x000000000000000000000000000000000000dead".into(),
        MIN_CR_E4,
        0,
    );
    assert!(matches!(res, Err(WithdrawError::InsufficientCollateral)), "got {res:?}");

    let v = s.chain_vaults.get(&7).expect("vault present");
    assert_eq!(v.collateral_amount_native, ONE_MON_E18, "collateral unchanged");
    assert_eq!(s.settlement_queues[&CHAIN].pending_len(), 0, "queue empty");
}

// 5. unknown vault errors.
#[test]
fn withdraw_unknown_vault_errors() {
    let mut s = setup(PRICE_100_USD_E8);
    let res =
        withdraw_collateral_in_state(&mut s, 999, ONE_MON_E18, "0x000000000000000000000000000000000000dead".into(), MIN_CR_E4, 0);
    assert!(matches!(res, Err(WithdrawError::UnknownVault)), "got {res:?}");
}

// 6. close requires debt == 0: a vault with debt is rejected with HasDebt, no
//    mutation, no enqueue.
#[test]
fn close_with_debt_is_rejected() {
    let mut s = setup(PRICE_100_USD_E8);
    open_and_fund(&mut s, 7, 5 * ONE_MON_E18, 100_00000000); // has debt

    let res = close_chain_vault_in_state(&mut s, 7, "0x000000000000000000000000000000000000dead".into(), MIN_CR_E4, 0);
    assert!(matches!(res, Err(WithdrawError::HasDebt)), "got {res:?}");

    let v = s.chain_vaults.get(&7).expect("vault present");
    assert_eq!(v.collateral_amount_native, 5 * ONE_MON_E18, "unchanged");
    assert!(matches!(v.status, ChainVaultStatus::Open), "unchanged");
    assert_eq!(s.settlement_queues[&CHAIN].pending_len(), 0, "queue empty");
}

// 7. close on a debt-free vault withdraws the full remainder + sets Closing.
#[test]
fn close_debt_free_withdraws_full_and_sets_closing() {
    let mut s = setup(PRICE_100_USD_E8);
    open_and_fund(&mut s, 7, 3 * ONE_MON_E18, 0); // 3 MON, debt-free

    let res = close_chain_vault_in_state(&mut s, 7, "0x000000000000000000000000000000000000dead".into(), MIN_CR_E4, 999);
    assert!(res.is_ok(), "debt-free close should succeed: {res:?}");

    let v = s.chain_vaults.get(&7).expect("vault present");
    assert_eq!(v.collateral_amount_native, 0, "full collateral reserved out");
    assert!(
        matches!(v.status, ChainVaultStatus::Closing),
        "close flips to Closing, got {:?}",
        v.status
    );
    let q = &s.settlement_queues[&CHAIN];
    assert_eq!(q.pending_len(), 1, "one NativeWithdrawal enqueued");
    let op = q.pending.values().next().expect("op present");
    match &op.kind {
        SettlementOpKind::NativeWithdrawal { recipient, amount_e18, vault_id } => {
            assert_eq!(recipient, "0x000000000000000000000000000000000000dead");
            assert_eq!(*amount_e18, 3 * ONE_MON_E18, "full remaining collateral");
            assert_eq!(*vault_id, 7);
        }
        other => panic!("expected NativeWithdrawal op, got {other:?}"),
    }
}

// 8. CRITICAL: withdraw from an AwaitingDeposit vault is rejected with no
//    mutation and no enqueue. The declared collateral was NEVER deposited
//    on-chain; paying it out from the settlement hot wallet would drain
//    protocol funds for phantom collateral.
#[test]
fn withdraw_from_awaiting_deposit_is_rejected() {
    let mut s = setup(PRICE_100_USD_E8);
    // Open a vault: it starts AwaitingDeposit with declared collateral but NO
    // on-chain deposit and debt 0. Do NOT drive it to Open.
    open_chain_vault_in_state(
        &mut s,
        CHAIN,
        owner(),
        "0xcustody".into(),
        5 * ONE_MON_E18, // declared collateral, never deposited
        1,               // open requires non-zero debt
        "0x000000000000000000000000000000000000c0de".into(),
        0, // min_cr 0 so open succeeds
        0,
        7,
    )
    .expect("open for setup");
    assert!(
        matches!(s.chain_vaults[&7].status, ChainVaultStatus::AwaitingDeposit),
        "vault should start AwaitingDeposit"
    );

    let res = withdraw_collateral_in_state(
        &mut s,
        7,
        5 * ONE_MON_E18,
        "0x000000000000000000000000000000000000dead".into(),
        MIN_CR_E4,
        555,
    );
    assert!(
        matches!(
            res,
            Err(WithdrawError::WrongStatus { status: ChainVaultStatus::AwaitingDeposit })
        ),
        "withdraw from AwaitingDeposit must reject with WrongStatus, got {res:?}"
    );

    let v = s.chain_vaults.get(&7).expect("vault present");
    assert_eq!(
        v.collateral_amount_native,
        5 * ONE_MON_E18,
        "collateral UNCHANGED on rejection"
    );
    assert!(
        matches!(v.status, ChainVaultStatus::AwaitingDeposit),
        "status unchanged"
    );
    assert!(
        s.settlement_queues.get(&CHAIN).map(|q| q.pending_len()).unwrap_or(0) == 0,
        "settlement queue must be EMPTY — no payout enqueued"
    );
}

// 9. CRITICAL: withdraw from a MintPending vault is rejected with no mutation
//    and no enqueue. A mint is in flight authorized against this collateral;
//    pulling it out from under the in-flight mint must be impossible.
#[test]
fn withdraw_from_mint_pending_is_rejected() {
    let mut s = setup(PRICE_100_USD_E8);
    open_and_fund(&mut s, 7, 5 * ONE_MON_E18, 0);
    // Force MintPending directly (pure-state test).
    s.chain_vaults.get_mut(&7).unwrap().status = ChainVaultStatus::MintPending;

    let res = withdraw_collateral_in_state(
        &mut s,
        7,
        ONE_MON_E18,
        "0x000000000000000000000000000000000000dead".into(),
        MIN_CR_E4,
        100,
    );
    assert!(
        matches!(
            res,
            Err(WithdrawError::WrongStatus { status: ChainVaultStatus::MintPending })
        ),
        "withdraw from MintPending must reject with WrongStatus, got {res:?}"
    );

    let v = s.chain_vaults.get(&7).expect("vault present");
    assert_eq!(v.collateral_amount_native, 5 * ONE_MON_E18, "collateral UNCHANGED");
    assert!(matches!(v.status, ChainVaultStatus::MintPending), "status unchanged");
    assert_eq!(
        s.settlement_queues.get(&CHAIN).map(|q| q.pending_len()).unwrap_or(0),
        0,
        "settlement queue must be EMPTY"
    );
}

// 10. close inherits the FIX 1 gate via delegation: a close on a non-Open,
//     debt-free vault (here Closing) rejects with WrongStatus AFTER the
//     debt==0 check, with no mutation and no enqueue. (Closing is reached via
//     a prior full withdraw; a reverted close returns the vault to Open via
//     the settlement revert path, so retry works without close-from-Closing.)
#[test]
fn close_from_closing_is_rejected_via_gate() {
    let mut s = setup(PRICE_100_USD_E8);
    open_and_fund(&mut s, 7, 3 * ONE_MON_E18, 0); // debt-free
    // Force Closing directly (a vault mid-withdrawal). debt is still 0.
    s.chain_vaults.get_mut(&7).unwrap().status = ChainVaultStatus::Closing;

    let res = close_chain_vault_in_state(&mut s, 7, "0x000000000000000000000000000000000000dead".into(), MIN_CR_E4, 777);
    assert!(
        matches!(
            res,
            Err(WithdrawError::WrongStatus { status: ChainVaultStatus::Closing })
        ),
        "close on a Closing vault must reject with WrongStatus, got {res:?}"
    );
    let v = s.chain_vaults.get(&7).expect("vault present");
    assert_eq!(v.collateral_amount_native, 3 * ONE_MON_E18, "collateral unchanged");
    assert!(matches!(v.status, ChainVaultStatus::Closing), "status unchanged");
    assert_eq!(
        s.settlement_queues.get(&CHAIN).map(|q| q.pending_len()).unwrap_or(0),
        0,
        "queue empty",
    );
}

// 11. FIX 3: closing an Open, debt-free, ZERO-collateral vault short-circuits
//     to Closed WITHOUT enqueuing a wasted zero-value NativeWithdrawal.
#[test]
fn close_zero_collateral_short_circuits_to_closed_no_enqueue() {
    let mut s = setup(PRICE_100_USD_E8);
    open_and_fund(&mut s, 7, 0, 0); // Open, debt-free, zero collateral

    let res = close_chain_vault_in_state(&mut s, 7, "0x000000000000000000000000000000000000dead".into(), MIN_CR_E4, 888);
    assert!(res.is_ok(), "zero-collateral close should succeed: {res:?}");

    let v = s.chain_vaults.get(&7).expect("vault present");
    assert_eq!(v.collateral_amount_native, 0, "still zero collateral");
    assert!(
        matches!(v.status, ChainVaultStatus::Closed),
        "zero-collateral close goes straight to Closed, got {:?}",
        v.status
    );
    assert_eq!(
        s.settlement_queues.get(&CHAIN).map(|q| q.pending_len()).unwrap_or(0),
        0,
        "NO zero-value NativeWithdrawal enqueued",
    );
}

// 12. withdraw with a malformed dest_address is rejected at the boundary with no
//     mutation and no enqueue. An unvalidated dest would later panic the
//     tx-building helper (tx::parse_address) deep on the settlement worker path,
//     after the re-entrancy guard + awaits, permanently blocking the chain's
//     worker. Fail-fast here makes that panic unreachable in practice. The vault
//     is Open with confirmed collateral + debt, so the InvalidAddress check is
//     reached (it sits after the status==Open + balance + CR gates).
#[test]
fn withdraw_rejects_invalid_dest() {
    let mut s = setup(PRICE_100_USD_E8);
    // 5 MON ($500), debt 100 icUSD ($100) — a healthy Open vault. A 1 MON
    // withdraw would leave 400% CR, so only the bad dest can reject it.
    open_and_fund(&mut s, 7, 5 * ONE_MON_E18, 100_00000000);

    // "0x123" is 0x-prefixed but only 3 hex digits (not 40) — a realistic typo.
    let res = withdraw_collateral_in_state(&mut s, 7, ONE_MON_E18, "0x123".into(), MIN_CR_E4, 100);
    assert!(matches!(res, Err(WithdrawError::InvalidAddress(_))), "got {res:?}");

    let v = s.chain_vaults.get(&7).expect("vault present");
    assert_eq!(v.collateral_amount_native, 5 * ONE_MON_E18, "collateral UNCHANGED on reject");
    assert!(matches!(v.status, ChainVaultStatus::Open), "status unchanged");
    assert_eq!(
        s.settlement_queues.get(&CHAIN).map(|q| q.pending_len()).unwrap_or(0),
        0,
        "settlement queue must be EMPTY — nothing enqueued",
    );

    // A non-hex body is rejected too (still no mutation, still no enqueue).
    let res =
        withdraw_collateral_in_state(&mut s, 7, ONE_MON_E18, "0xnothex".into(), MIN_CR_E4, 101);
    assert!(matches!(res, Err(WithdrawError::InvalidAddress(_))), "got {res:?}");
    let v = s.chain_vaults.get(&7).expect("vault present");
    assert_eq!(v.collateral_amount_native, 5 * ONE_MON_E18, "still unchanged");
    assert_eq!(
        s.settlement_queues.get(&CHAIN).map(|q| q.pending_len()).unwrap_or(0),
        0,
        "still empty",
    );
}
