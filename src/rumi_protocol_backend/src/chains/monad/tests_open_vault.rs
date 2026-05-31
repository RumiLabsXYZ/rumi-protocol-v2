//! Task 12: open_chain_vault (open-then-verify) pure-helper tests.
//!
//! The owner deviated from the written plan: a vault is OPENED in a new
//! `AwaitingDeposit` status with NO mint enqueued. deposit-watch later verifies
//! the custody-address balance covers the DECLARED collateral and only THEN
//! flips the vault to `MintPending` and enqueues the `Mint` op. icUSD is only
//! ever minted against a verified on-chain deposit (CDP backing invariant).
//!
//! These tests exercise the three pure helpers in `chain_vault.rs`:
//! - `collateral_ratio_e4`
//! - `open_chain_vault_in_state` (creates AwaitingDeposit, enqueues NOTHING)
//! - `verify_deposit_and_enqueue_mint_in_state` (transitions + enqueues)

use super::chain_vault::{
    collateral_ratio_e4, open_chain_vault_in_state, verify_deposit_and_enqueue_mint_in_state,
    ChainVaultStatus, OpenVaultError,
};
use crate::chains::config::{ChainId, GasStrategy, RegisterChainArg};
use crate::chains::multi_chain_state::MultiChainStateV2;
use crate::chains::settlement_queue::SettlementOpKind;
use candid::Principal;

const CHAIN: ChainId = ChainId(10143);
/// USD price as e8: $2.00.
const PRICE_2_USD_E8: u64 = 2_0000_0000;
/// USD price as e8: $100.00.
const PRICE_100_USD_E8: u64 = 100_0000_0000;
/// 1 MON in e18.
const ONE_MON_E18: u128 = 1_000_000_000_000_000_000;

/// Register chain 10143 and set its manual MON price. Mirrors what the live
/// register_chain endpoint + a manual price override do (chain_supplies and
/// settlement_queues are seeded by register_chain_in_state).
fn setup(price_e8: u64) -> MultiChainStateV2 {
    let mut s = MultiChainStateV2::default();
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
    };
    crate::chains::admin::register_chain_in_state(&mut s, arg, 0).expect("register chain");
    s.manual_prices.insert((CHAIN, "MON".into()), price_e8);
    s
}

fn owner() -> Principal {
    Principal::anonymous()
}

// 1. CR math
#[test]
fn cr_computed_from_collateral_price_and_debt() {
    // 5 MON e18 at $2.00 e8, debt $4.00 e8s -> $10 collateral / $4 debt = 250.00% -> 25000.
    assert_eq!(
        collateral_ratio_e4(5 * ONE_MON_E18, PRICE_2_USD_E8, 4_00000000),
        25000
    );
}

#[test]
fn cr_is_max_when_debt_zero() {
    assert_eq!(collateral_ratio_e4(5 * ONE_MON_E18, PRICE_2_USD_E8, 0), u64::MAX);
}

// 1b. Degenerate open: zero debt is rejected (would enqueue a wasted 0-mint).
#[test]
fn open_rejects_zero_debt() {
    let mut s = setup(PRICE_100_USD_E8);
    let res = open_chain_vault_in_state(
        &mut s,
        CHAIN,
        owner(),
        "0xcustody".into(),
        100 * ONE_MON_E18, // plenty of collateral...
        0,                 // ...but zero debt
        "0x000000000000000000000000000000000000c0de".into(),
        13_000,
        0,
        9,
    );
    assert!(matches!(res, Err(OpenVaultError::ZeroDebt)));
    assert!(s.chain_vaults.is_empty());
    assert!(s
        .settlement_queues
        .get(&CHAIN)
        .map(|q| q.pending.is_empty())
        .unwrap_or(true));
}

// 2. open rejects below min CR, creates nothing, enqueues nothing
#[test]
fn open_rejects_below_min_cr() {
    // 1 MON ($2) collateral, 100 icUSD ($100) debt, min_cr 13000 (130%).
    // CR = $2 / $100 = 2.00% -> 200 e4 -> well below 13000 -> reject.
    let mut s = setup(PRICE_2_USD_E8);
    let res = open_chain_vault_in_state(
        &mut s,
        CHAIN,
        owner(),
        "0xcustody".into(),
        ONE_MON_E18,
        100_00000000, // 100 icUSD debt
        "0x000000000000000000000000000000000000c0de".into(),
        13000,
        0,
        1,
    );
    assert!(matches!(res, Err(OpenVaultError::BelowMinCr { .. })), "got {res:?}");
    assert!(s.chain_vaults.is_empty(), "no vault should be created");
    assert_eq!(s.settlement_queues[&CHAIN].pending_len(), 0, "queue must stay empty");
}

// 3. open creates AwaitingDeposit + does NOT enqueue a mint (the core deviation)
#[test]
fn open_creates_awaiting_deposit_vault_and_enqueues_nothing() {
    // MON price $100, big collateral (100 MON = $10_000), debt 100 icUSD ($100), min_cr 13000.
    // CR = $10_000 / $100 = 10000% -> way above min -> accept.
    let mut s = setup(PRICE_100_USD_E8);
    let declared = 100 * ONE_MON_E18;
    let res = open_chain_vault_in_state(
        &mut s,
        CHAIN,
        owner(),
        "0xcustody".into(),
        declared,
        100_00000000, // 100 icUSD intended mint
        "0x000000000000000000000000000000000000c0de".into(),
        13000,
        12345,
        7,
    );
    assert!(res.is_ok(), "open should succeed: {res:?}");
    let v = s.chain_vaults.get(&7).expect("vault 7 created");
    assert!(matches!(v.status, ChainVaultStatus::AwaitingDeposit), "status {:?}", v.status);
    assert_eq!(v.pending_mint_e8s, 100_00000000, "intended mint stored in pending");
    assert_eq!(v.debt_e8s, 0, "no debt until verified deposit + confirmed mint");
    assert_eq!(v.collateral_amount_e18, declared, "declared collateral recorded");
    assert_eq!(v.owner, owner());
    assert_eq!(v.custody_address, "0xcustody");
    assert_eq!(v.mint_recipient, "0x000000000000000000000000000000000000c0de");
    assert_eq!(v.opened_at_ns, 12345);
    // THE CORE DEVIATION: nothing enqueued at open.
    assert_eq!(
        s.settlement_queues[&CHAIN].pending_len(),
        0,
        "open_chain_vault_in_state must NOT enqueue a mint"
    );
}

// 4. deposit below declared collateral does NOT transition/enqueue
#[test]
fn insufficient_deposit_does_not_enqueue() {
    let mut s = setup(PRICE_100_USD_E8);
    let declared = 100 * ONE_MON_E18;
    open_chain_vault_in_state(
        &mut s, CHAIN, owner(), "0xcustody".into(), declared, 100_00000000,
        "0x000000000000000000000000000000000000c0de".into(), 13000, 0, 7,
    )
    .expect("open");

    // Observed only 99 MON — below the 100 MON declared.
    let observed = 99 * ONE_MON_E18;
    let res = verify_deposit_and_enqueue_mint_in_state(&mut s, 7, observed, 100);
    assert!(matches!(res, Ok(false)), "got {res:?}");
    assert!(
        matches!(s.chain_vaults[&7].status, ChainVaultStatus::AwaitingDeposit),
        "must stay AwaitingDeposit"
    );
    assert_eq!(s.settlement_queues[&CHAIN].pending_len(), 0, "queue must stay empty");
}

// 5. sufficient deposit transitions to MintPending and enqueues the mint
#[test]
fn sufficient_deposit_transitions_and_enqueues_mint() {
    let mut s = setup(PRICE_100_USD_E8);
    let declared = 100 * ONE_MON_E18;
    open_chain_vault_in_state(
        &mut s, CHAIN, owner(), "0xcustody".into(), declared, 100_00000000,
        "0x000000000000000000000000000000000000c0de".into(), 13000, 0, 7,
    )
    .expect("open");

    // Observed exactly the declared collateral.
    let res = verify_deposit_and_enqueue_mint_in_state(&mut s, 7, declared, 200);
    assert!(matches!(res, Ok(true)), "got {res:?}");
    assert!(
        matches!(s.chain_vaults[&7].status, ChainVaultStatus::MintPending),
        "must flip to MintPending"
    );
    let q = &s.settlement_queues[&CHAIN];
    assert_eq!(q.pending_len(), 1, "exactly one Mint enqueued");
    let op = q.pending.values().next().expect("op present");
    match &op.kind {
        SettlementOpKind::Mint { recipient, amount_e8s, vault_id } => {
            assert_eq!(recipient, "0x000000000000000000000000000000000000c0de");
            assert_eq!(*amount_e8s, 100_00000000);
            assert_eq!(*vault_id, 7);
        }
        other => panic!("expected Mint op, got {other:?}"),
    }
    assert_eq!(op.idempotency_key, format!("mint-{}-{}", CHAIN.0, 7));
}

// 6. re-verifying an already-transitioned vault does not double-enqueue
#[test]
fn reverify_after_transition_is_noop() {
    let mut s = setup(PRICE_100_USD_E8);
    let declared = 100 * ONE_MON_E18;
    open_chain_vault_in_state(
        &mut s, CHAIN, owner(), "0xcustody".into(), declared, 100_00000000,
        "0x000000000000000000000000000000000000c0de".into(), 13000, 0, 7,
    )
    .expect("open");
    // First verify transitions + enqueues.
    assert!(matches!(
        verify_deposit_and_enqueue_mint_in_state(&mut s, 7, declared, 200),
        Ok(true)
    ));
    assert_eq!(s.settlement_queues[&CHAIN].pending_len(), 1);

    // Re-verify with even MORE balance — must be an idempotent no-op.
    let res = verify_deposit_and_enqueue_mint_in_state(&mut s, 7, 200 * ONE_MON_E18, 300);
    assert!(matches!(res, Ok(false)), "got {res:?}");
    assert_eq!(
        s.settlement_queues[&CHAIN].pending_len(),
        1,
        "must NOT double-enqueue on re-verify"
    );
    assert!(matches!(s.chain_vaults[&7].status, ChainVaultStatus::MintPending));
}

// 7. verify unknown vault errors
#[test]
fn verify_unknown_vault_errors() {
    let mut s = setup(PRICE_100_USD_E8);
    let res = verify_deposit_and_enqueue_mint_in_state(&mut s, 999, 100 * ONE_MON_E18, 0);
    assert!(matches!(res, Err(OpenVaultError::UnknownVault)), "got {res:?}");
}

// 8. open rejects an unregistered chain
#[test]
fn open_rejects_unknown_chain() {
    let mut s = MultiChainStateV2::default(); // no chain registered
    let res = open_chain_vault_in_state(
        &mut s, CHAIN, owner(), "0xcustody".into(), 100 * ONE_MON_E18, 100_00000000,
        "0x000000000000000000000000000000000000c0de".into(), 13000, 0, 7,
    );
    assert!(matches!(res, Err(OpenVaultError::UnknownChain)), "got {res:?}");
    assert!(s.chain_vaults.is_empty());
}

// 8b. open rejects a malformed mint_recipient at the boundary, creating nothing
//     and enqueuing nothing. An unvalidated recipient would later panic the
//     tx-building helper (tx::abi_word_address) deep on the settlement worker
//     path, after the re-entrancy guard + awaits, permanently blocking the
//     chain's worker. Fail-fast here makes that panic unreachable in practice.
#[test]
fn open_rejects_invalid_recipient() {
    let mut s = setup(PRICE_100_USD_E8);
    // "0x123" is a realistic typo: 0x-prefixed but only 3 hex digits, not 40.
    let res = open_chain_vault_in_state(
        &mut s,
        CHAIN,
        owner(),
        "0xcustody".into(), // custody is derived/valid in production; not validated
        100 * ONE_MON_E18,  // plenty of collateral
        100_00000000,       // non-zero debt (so we pass the ZeroDebt gate)
        "0x123".into(),     // malformed recipient: too short
        13_000,
        12345,
        7,
    );
    assert!(matches!(res, Err(OpenVaultError::InvalidAddress(_))), "got {res:?}");
    assert!(s.chain_vaults.is_empty(), "no vault should be created on a bad recipient");
    assert_eq!(
        s.settlement_queues[&CHAIN].pending_len(),
        0,
        "queue must stay empty — nothing enqueued"
    );

    // A non-hex body is rejected too.
    let res = open_chain_vault_in_state(
        &mut s,
        CHAIN,
        owner(),
        "0xcustody".into(),
        100 * ONE_MON_E18,
        100_00000000,
        "0xnothex".into(), // 0x-prefixed but contains non-hex chars
        13_000,
        12345,
        8,
    );
    assert!(matches!(res, Err(OpenVaultError::InvalidAddress(_))), "got {res:?}");
    assert!(s.chain_vaults.is_empty(), "still no vault");
    assert_eq!(s.settlement_queues[&CHAIN].pending_len(), 0, "still empty");
}

// 9. open rejects when no MON price is configured
#[test]
fn open_rejects_when_no_price() {
    let mut s = MultiChainStateV2::default();
    // Register the chain but DON'T set a MON price.
    let arg = RegisterChainArg {
        chain_id: CHAIN,
        display_name: "MonadTestnet".into(),
        rpc_endpoints: vec!["https://rpc".into()],
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 { max_priority_fee_gwei: 2, max_fee_gwei_ceiling: 500 },
        chain_native_decimals: 18,
    };
    crate::chains::admin::register_chain_in_state(&mut s, arg, 0).expect("register chain");
    let res = open_chain_vault_in_state(
        &mut s, CHAIN, owner(), "0xcustody".into(), 100 * ONE_MON_E18, 100_00000000,
        "0x000000000000000000000000000000000000c0de".into(), 13000, 0, 7,
    );
    assert!(matches!(res, Err(OpenVaultError::NoPrice)), "got {res:?}");
    assert!(s.chain_vaults.is_empty());
}
