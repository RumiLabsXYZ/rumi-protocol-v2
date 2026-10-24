//! BB-09b-1: end to end scenarios for the ICP icUSD <-> Base icUSD escrow bridge.
//!
//! Every scenario runs the real bridge wasm in PocketIC against the real ICRC ledger wasm and
//! a local anvil node running the real `BridgedIcUSD` contract. The bridge's Base
//! transactions are signed with its own threshold-ECDSA minter key and really execute on
//! anvil. The three provider URLs are a pure passthrough to that one anvil (no fault hook is
//! installed anywhere in this file), so nothing here is mocked external-chain evidence. The
//! only simulated external action is the explicit admin impersonation in
//! `contract_paused_preflight_holds_then_owner_refunds` (anvil impersonates the bridge's own
//! admin address to call `pauseMint()`; test-only).
//!
//! Build once, then run:
//!
//! ```text
//! cargo build --offline --release --target wasm32-unknown-unknown -p rumi_icusd_bridge -p evm_rpc_relay
//! POCKET_IC_BIN=$PWD/pocket-ic cargo test --offline -p rumi_icusd_bridge --test bridge_e2e_pic -- --test-threads=1
//! ```

mod support;

use candid::utils::ArgumentEncoder;
use candid::{CandidType, Deserialize, Principal};
use icrc_ledger_types::icrc1::account::Account;
use serde_json::json;
use support::bridge::*;
use support::contract::encode_bridge_out;
use support::ledger::account;
use support::pic::test_principal;

fn e8(n: u64) -> u64 {
    n * E8
}

fn with_sub(owner: Principal, sub: [u8; 32]) -> Account {
    Account { owner, subaccount: Some(sub) }
}

/// Call `method` as `sender` and require an `Err`; returns it.
fn call_err<A: ArgumentEncoder, T: CandidType + for<'de> Deserialize<'de>>(
    w: &mut BridgeWorld,
    sender: Principal,
    method: &str,
    args: A,
) -> BridgeError {
    let r: BridgeResult<T> = w.update(sender, method, args);
    match r {
        Err(e) => e,
        Ok(_) => panic!("{method} unexpectedly succeeded for {sender}"),
    }
}

/// The text of anvil's JSON-RPC error when `bridgeOut(..)` is simulated from `from`
/// (a revert carries the custom-error selector in its data).
fn simulate_bridge_out_error(
    w: &BridgeWorld,
    from: &str,
    amount: u128,
    principal: &[u8],
    sub: [u8; 32],
    nonce: [u8; 32],
) -> String {
    let data = encode_bridge_out(amount, principal, sub, nonce);
    match w.anvil.try_rpc(
        "eth_call",
        json!([{"from": from, "to": w.token.address, "data": format!("0x{}", hex::encode(data))}, "latest"]),
    ) {
        Err(e) => e.to_string().to_lowercase(),
        Ok(v) => panic!("expected a revert, eth_call returned {v}"),
    }
}

// ---------------------------------------------------------------------------
// 1. Round trip ICP -> Base -> ICP with exact e8s
// ---------------------------------------------------------------------------

#[test]
fn round_trip_icp_to_base_and_back_exact_e8s() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let a = e8(100);
    let b1 = e8(40);
    let recipient = w.holder(0);
    let user = w.user;
    let user2 = w.user2;
    let info = w.bridge_info();
    assert_eq!(info.escrow_account, w.escrow_account(), "escrow account is (bridge, sha256 label)");
    assert!(info.halted.is_none() && info.contract.is_some());
    assert_eq!(info.pauses, Pauses::default());

    // --- ICP -> Base ---
    w.approve_bridge(user, a);
    let supply0 = w.ledger_total_supply();
    let user_before = w.user_balance(user);
    let args = w.deposit_args(a, &recipient);
    let dep = w.bridge_to_base(user, args).expect("bridge_to_base");
    assert_eq!(dep.status, DepositStatus::Escrowed, "pull proven: {dep:?}");
    assert!(dep.pull_block_index.is_some(), "pull block index is reported");
    assert_eq!((dep.amount_e8s, dep.deposit_fee_e8s, dep.mint_amount_e8s), (a, 0, a));
    assert_eq!(dep.owner, user);
    assert_eq!(dep.base_recipient, recipient);
    // The user paid exactly A + the ledger fee; the escrow account holds exactly A.
    assert_eq!(user_before - w.user_balance(user), a as u128 + LEDGER_FEE as u128);
    assert_eq!(w.escrow_balance(), a as u128);
    assert_eq!(w.ledger_total_supply(), supply0 - LEDGER_FEE as u128, "only the pull fee is burned");
    // Nothing exists on Base yet.
    assert_eq!(w.base_total_supply(), 0);
    assert_eq!(w.minter_nonce(), 0);

    let dep = w.wait_mint_included(dep.id);
    // The mint tx really executed on anvil: status 1, one BridgeMint log with our mint id.
    assert_eq!(dep.mint_tx_hashes.len(), 1, "exactly one signed attempt");
    let tx_hash = dep.mint_tx_hashes[0].clone();
    let rc = w.receipt(&tx_hash).expect("mint receipt on anvil");
    assert_eq!(rc["status"], "0x1", "mint tx succeeded on anvil: {rc}");
    assert_eq!(rc["from"].as_str().unwrap().to_lowercase(), w.base_addrs.minter, "signed by the tECDSA minter");
    assert_eq!(rc["to"].as_str().unwrap().to_lowercase(), w.token.address);
    let (mint_id, to, amount) = w.bridge_mint_of(&tx_hash);
    assert_eq!((hex0x(&mint_id), to.as_str(), amount), (dep.mint_id.clone(), recipient.as_str(), a as u128));
    // Included but not final: the bridge has not credited anything yet.
    assert!(matches!(dep.status, DepositStatus::MintInFlight), "{:?}", dep.status);
    assert_eq!(w.supply_audit().outstanding(), 0);

    w.mine_finalize();
    let dep = w.wait_delivered(dep.id);
    let delivered = dep.delivered.as_ref().expect("inclusion record");
    assert_eq!(delivered.tx_hash, tx_hash);
    assert_eq!(w.base_balance(&recipient), a as u128);
    assert_eq!(w.base_total_supply(), a as u128);
    assert_eq!(w.minter_nonce(), 1, "exactly one Base tx was sent");
    assert!(w.base_mint_consumed(&dep.mint_id));

    let audit = w.reconcile();
    assert_eq!(audit.coverage_ok, Some(true), "{audit:?}");
    assert_eq!(audit.base_supply_within_bound, Some(true), "{audit:?}");
    assert_eq!(audit.outstanding(), a as u128);
    assert_eq!(audit.escrow(), Some(a as u128));

    // --- Base -> ICP (partial) ---
    let user_before = w.user_balance(user);
    let nonce = w.fresh_nonce();
    let out = w.bridge_out_from(&recipient, b1 as u128, user.as_slice(), [0u8; 32], nonce);
    assert!(out.success, "bridgeOut mined with status 1");
    let ev = out.event.clone().expect("BridgeOut event");
    assert_eq!((ev.amount, ev.sequence, ev.from.as_str()), (b1 as u128, 1, recipient.as_str()));
    assert_eq!(hex0x(&ev.transfer_id), out.transfer_id);
    assert_eq!(w.base_balance(&recipient), (a - b1) as u128);
    assert_eq!(w.base_total_supply(), (a - b1) as u128, "bridgeOut burns on Base immediately");
    // A hint never settles, and the bridge must NOT release before Base finality.
    w.notify_hint(user, &out.tx_hash).expect("hint accepted");
    w.ticks(4);
    assert!(w.get_release(&out.transfer_id).is_none(), "no release before the burn is final");
    assert_eq!(w.user_balance(user), user_before, "ledger untouched before finality");
    assert_eq!(w.escrow_balance(), a as u128);

    w.mine_finalize();
    let rel = w.wait_released(&out.transfer_id);
    assert_eq!(rel.amount_e8s, b1);
    assert_eq!(rel.release_fee_e8s, 0);
    assert_eq!(rel.amount_to_receive_e8s, Some(b1 - LEDGER_FEE));
    assert_eq!(rel.destination, Some(account(user)));
    assert_eq!(rel.base_sender, recipient);
    assert!(rel.release_block_index.is_some());
    assert_eq!(w.user_balance(user) - user_before, (b1 - LEDGER_FEE) as u128, "user gets B - ledger fee - release fee");
    assert_eq!(w.escrow_balance(), (a - b1) as u128, "escrow pays B (amount + ledger fee)");

    let audit = w.reconcile();
    assert_eq!(audit.coverage_ok, Some(true), "{audit:?}");
    assert_eq!(audit.base_supply_within_bound, Some(true), "{audit:?}");
    assert_eq!(audit.outstanding(), (a - b1) as u128);
    assert_eq!(audit.base_total_supply_e8s.as_ref().map(nat_u128), Some((a - b1) as u128));
    assert_eq!(audit.base_supply_exact_match, Some(true), "quiescent: Base supply == outstanding: {audit:?}");
    assert_eq!(audit.buckets(), [(a - b1) as u128, 0, 0, 0, 0, 0, 0]);

    // --- Base -> ICP (the rest, to a different account with a subaccount) ---
    let rest = a - b1;
    let sub = [7u8; 32];
    let user2_acc = with_sub(user2, sub);
    let before2 = w.ledger_balance(&user2_acc);
    let nonce = w.fresh_nonce();
    let out2 = w.bridge_out_from(&recipient, rest as u128, user2.as_slice(), sub, nonce);
    assert!(out2.success);
    assert_eq!(out2.event.as_ref().unwrap().sequence, 2);
    w.mine_finalize();
    let rel2 = w.wait_released(&out2.transfer_id);
    assert_eq!(rel2.destination, Some(user2_acc));
    assert_eq!(w.ledger_balance(&user2_acc) - before2, (rest - LEDGER_FEE) as u128);
    assert_eq!(w.base_total_supply(), 0);
    assert_eq!(w.base_balance(&recipient), 0);
    assert_eq!(w.minter_nonce(), 1, "still exactly one tx from the minter");
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [0; 7], "every bucket is back to zero: {audit:?}");
    assert_eq!(audit.escrow(), Some(audit.fees()), "escrow equals the fee bucket (0)");
    assert_eq!(w.escrow_balance(), 0);
    assert_eq!(audit.coverage_ok, Some(true));
    assert_eq!(audit.base_supply_exact_match, Some(true), "{audit:?}");
    let info = w.bridge_info();
    assert_eq!((info.active_deposits, info.active_releases), (0, 0));
    assert!(info.halted.is_none(), "{:?}", info.halted);
    assert_eq!(info.observer.processed_count, 2);
}

// ---------------------------------------------------------------------------
// 2. Fees: exact delivered / released amounts, the fee bucket, FeeChanged, sweep
// ---------------------------------------------------------------------------

#[test]
fn fees_are_exact_and_sweep_moves_the_fee_bucket() {
    let dep_fee = 50_000_000u64; // 0.5 icUSD
    let rel_fee = 30_000_000u64; // 0.3 icUSD
    let mut opts = SetupOptions::default();
    opts.fees = BridgeFees { deposit_fee_e8s: dep_fee, release_fee_e8s: rel_fee };
    let mut w = BridgeWorld::setup(opts);
    let (user, recipient) = (w.user, w.holder(0));
    assert_eq!(w.bridge_info().fees, BridgeFees { deposit_fee_e8s: dep_fee, release_fee_e8s: rel_fee });
    let a = e8(10);

    // A stale fee quote is rejected and nothing moves.
    w.approve_bridge(user, a);
    let mut stale = w.deposit_args(a, &recipient);
    stale.expected_deposit_fee_e8s = 0;
    let before = w.user_balance(user);
    assert_eq!(w.bridge_to_base(user, stale), Err(BridgeError::FeeChanged { current_fee_e8s: dep_fee }));
    assert_eq!(w.user_balance(user), before);
    assert!(w.list_my_deposits(user, None, 10).is_empty(), "a rejected request leaves no record");

    // The right quote: A is pulled, A - fee is minted.
    let args = w.deposit_args(a, &recipient);
    assert_eq!(args.expected_deposit_fee_e8s, dep_fee);
    let dep = w.bridge_to_base(user, args).expect("bridge_to_base");
    assert_eq!((dep.amount_e8s, dep.deposit_fee_e8s, dep.mint_amount_e8s), (a, dep_fee, a - dep_fee));
    assert_eq!(w.escrow_balance(), a as u128);
    w.wait_mint_included(dep.id);
    w.mine_finalize();
    let dep = w.wait_delivered(dep.id);
    let m = (a - dep_fee) as u128;
    assert_eq!(w.base_balance(&recipient), m, "recipient got A - deposit fee");
    assert_eq!(w.base_total_supply(), m);
    let (mint_id, _, minted) = w.bridge_mint_of(&dep.mint_tx_hashes[0]);
    assert_eq!((hex0x(&mint_id), minted), (dep.mint_id.clone(), m));
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [m, 0, 0, 0, 0, dep_fee as u128, 0], "O = A - fee, F = fee: {audit:?}");
    assert_eq!(audit.escrow(), Some(a as u128));
    assert_eq!(audit.coverage_ok, Some(true));

    // Bridge everything back: the user gets B - release fee - ledger fee.
    let b = m as u64;
    let user_before = w.user_balance(user);
    let (_, rel) = w.bridge_out_and_release(&recipient, b as u128, user);
    let received = b - rel_fee - LEDGER_FEE;
    assert_eq!((rel.amount_e8s, rel.release_fee_e8s, rel.amount_to_receive_e8s), (b, rel_fee, Some(received)));
    assert_eq!(w.user_balance(user) - user_before, received as u128);
    let fees_total = (dep_fee + rel_fee) as u128;
    assert_eq!(w.escrow_balance(), fees_total, "escrow keeps exactly the fees");
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [0, 0, 0, 0, 0, fees_total, 0], "{audit:?}");
    assert_eq!(audit.escrow(), Some(fees_total));
    assert_eq!(audit.coverage_ok, Some(true));
    assert_eq!(w.base_total_supply(), 0);

    // Sweeps: more than the bucket, or not above the ledger fee, are refused.
    let rec = w.fee_recipient;
    assert!(matches!(w.sweep_fees(fees_total as u64 + 1), Err(BridgeError::InvalidArgument(_))));
    assert!(matches!(w.sweep_fees(LEDGER_FEE), Err(BridgeError::InvalidArgument(_))));
    assert_eq!(w.ledger_balance(&rec), 0);
    // Sweep part: the recipient gets exactly S - ledger fee; the escrow pays S.
    let s1 = 30_000_000u64;
    w.sweep_fees(s1).expect("first sweep");
    w.ticks(2);
    assert_eq!(w.ledger_balance(&rec), (s1 - LEDGER_FEE) as u128);
    assert_eq!(w.escrow_balance(), fees_total - s1 as u128);
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [0, 0, 0, 0, 0, fees_total - s1 as u128, 0], "{audit:?}");
    assert_eq!(audit.coverage_ok, Some(true));
    // Sweep the rest.
    let s2 = (fees_total - s1 as u128) as u64;
    w.sweep_fees(s2).expect("second sweep");
    w.ticks(2);
    assert_eq!(w.ledger_balance(&rec), (s1 + s2 - 2 * LEDGER_FEE) as u128);
    assert_eq!(w.escrow_balance(), 0);
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [0; 7], "{audit:?}");
    assert_eq!(audit.escrow(), Some(0));
    assert!(w.bridge_info().halted.is_none());
}

// ---------------------------------------------------------------------------
// 3. Duplicate requests
// ---------------------------------------------------------------------------

#[test]
fn duplicate_requests_do_not_double_pull_or_double_release() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, recipient, other) = (w.user, w.holder(0), w.holder(1));
    let a = e8(20);

    // Only A + fee is approved: a second pull would fail, so a duplicate that did try to
    // pull again would show as a PullFailed or a changed balance.
    w.approve_bridge(user, a);
    let args = w.deposit_args(a, &recipient);
    let d1 = w.bridge_to_base(user, args.clone()).expect("first request");
    assert_eq!(d1.status, DepositStatus::Escrowed);
    let (bal, esc, supply) = (w.user_balance(user), w.escrow_balance(), w.ledger_total_supply());
    let d2 = w.bridge_to_base(user, args.clone()).expect("identical retry");
    assert_eq!(d2.id, d1.id, "same client_request_id + same args returns the same deposit");
    assert_eq!(d2.pull_block_index, d1.pull_block_index, "no second pull block");
    assert_eq!((w.user_balance(user), w.escrow_balance(), w.ledger_total_supply()), (bal, esc, supply));
    assert_eq!(w.list_my_deposits(user, None, 10).len(), 1);

    // Same id, different arguments: conflict (amount, recipient, subaccount).
    let mut diff = args.clone();
    diff.amount_e8s = a + 1;
    assert_eq!(w.bridge_to_base(user, diff), Err(BridgeError::RequestIdConflict));
    let mut diff = args.clone();
    diff.base_recipient = other.clone();
    assert_eq!(w.bridge_to_base(user, diff), Err(BridgeError::RequestIdConflict));
    let mut diff = args.clone();
    diff.from_subaccount = Some(serde_bytes::ByteBuf::from(vec![9u8; 32]));
    assert_eq!(w.bridge_to_base(user, diff), Err(BridgeError::RequestIdConflict));
    assert_eq!((w.user_balance(user), w.escrow_balance()), (bal, esc), "conflicts move nothing");
    assert_eq!(w.list_my_deposits(user, None, 10).len(), 1);

    // Deliver, then repeat once more: still the same (now Delivered) deposit, still one mint.
    w.wait_mint_included(d1.id);
    w.mine_finalize();
    let delivered = w.wait_delivered(d1.id);
    let d3 = w.bridge_to_base(user, args).expect("retry after delivery");
    assert_eq!((d3.id, d3.status.clone()), (d1.id, DepositStatus::Delivered));
    assert_eq!(w.minter_nonce(), 1);
    assert_eq!(w.base_total_supply(), a as u128);
    assert_eq!(delivered.mint_tx_hashes.len(), 1);

    // bridgeOut: the same (sender, nonce) twice. The second reverts on anvil
    // (BridgeOutAlreadyUsed) and never reaches the bridge.
    let b = e8(5);
    let nonce = w.fresh_nonce();
    let first = w.bridge_out_from(&recipient, b as u128, user.as_slice(), [0u8; 32], nonce);
    assert!(first.success);
    let err = simulate_bridge_out_error(&w, &recipient, b as u128, user.as_slice(), [0u8; 32], nonce);
    let used = selector_hex("BridgeOutAlreadyUsed(bytes32)");
    assert!(err.contains(used.trim_start_matches("0x")), "revert is BridgeOutAlreadyUsed ({used}): {err}");
    let second = w.bridge_out_from(&recipient, b as u128, user.as_slice(), [0u8; 32], nonce);
    assert!(!second.success, "replayed (sender, nonce) is mined as a failed receipt");
    assert!(second.event.is_none());
    assert_eq!(second.transfer_id, first.transfer_id, "same sender + nonce => same transfer id");
    assert_eq!(w.base_bridge_out_count(), 1);
    assert_eq!(w.base_balance(&recipient), (a - b) as u128, "burned once");
    assert_eq!(w.base_total_supply(), (a - b) as u128);

    let user_before = w.user_balance(user);
    w.mine_finalize();
    let rel = w.wait_released(&first.transfer_id);
    w.ticks(4);
    assert_eq!(w.list_releases_for_owner(user, None, 10).len(), 1, "exactly one release record");
    assert_eq!(w.user_balance(user) - user_before, (b - LEDGER_FEE) as u128, "released exactly once");
    assert_eq!(rel.sequence, 1);
    assert_eq!(w.bridge_info().observer.processed_count, 1);
    assert_eq!(w.escrow_balance(), (a - b) as u128);
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [(a - b) as u128, 0, 0, 0, 0, 0, 0], "{audit:?}");
    assert_eq!(audit.coverage_ok, Some(true));
}

// ---------------------------------------------------------------------------
// 4. Rejections: allowance, balance, bounds, caller checks
// ---------------------------------------------------------------------------

#[test]
fn rejected_requests_move_no_funds_and_release_capacity() {
    let mut opts = SetupOptions::default();
    // Capacity for exactly one 10 icUSD deposit: a failed pull must hand its reservation back.
    opts.limits = BridgeLimits { max_deposit_e8s: e8(10), max_outstanding_e8s: e8(10), ..default_limits() };
    let mut w = BridgeWorld::setup(opts);
    let (user, recipient, admin) = (w.user, w.holder(0), w.admin);
    let initial_info = w.bridge_info();

    // (a) No approval at all: nothing moves and no capacity stays reserved. Older canisters
    // create the deposit and report `PullFailed` (ledger refused the pull); newer ones
    // pre-check the allowance and reject with `InsufficientAllowance` before creating any
    // deposit record. Both are accepted here.
    let user_before = w.user_balance(user);
    let args = w.deposit_args(e8(10), &recipient);
    let n_records = w.list_my_deposits(user, None, 100).len();
    let mut failed_id = None;
    match w.bridge_to_base(user, args.clone()) {
        Ok(failed) => {
            assert!(
                matches!(&failed.status, DepositStatus::PullFailed { reason } if reason.to_lowercase().contains("allowance")),
                "{failed:?}"
            );
            assert!(failed.pull_block_index.is_none() && failed.mint_tx_hashes.is_empty());
            // The same request id returns the recorded failure; it is not retried.
            let again = w.bridge_to_base(user, args).expect("same request");
            assert_eq!((again.id, again.status), (failed.id, failed.status.clone()));
            failed_id = Some(failed.id);
        }
        Err(BridgeError::InsufficientAllowance { allowance_e8s, required_e8s }) => {
            assert_eq!((allowance_e8s, required_e8s), (0, e8(10) + LEDGER_FEE));
            assert_eq!(w.list_my_deposits(user, None, 100).len(), n_records, "no deposit record is created");
        }
        other => panic!("unexpected result for a request without approval: {other:?}"),
    }
    assert_eq!(w.user_balance(user), user_before, "no funds moved");
    assert_eq!(w.escrow_balance(), 0);
    let info = w.bridge_info();
    assert_eq!((info.active_deposits, info.mint_window_used_e8s), (0, 0), "capacity and window usage released");
    let audit = w.supply_audit();
    assert_eq!(audit.buckets(), [0; 7], "{audit:?}");
    // A failed deposit cannot be refunded (nothing was escrowed).
    if let Some(id) = failed_id {
        assert!(matches!(w.request_refund(user, id), Err(BridgeError::InvalidState(_))));
    }
    // Capacity is truly free again: a funded, approved 10 icUSD deposit is admitted.
    w.approve_bridge(user, e8(10));
    let args = w.deposit_args(e8(10), &recipient);
    let ok = w.bridge_to_base(user, args).expect("second request");
    assert_eq!(ok.status, DepositStatus::Escrowed, "{ok:?}");
    assert_eq!(w.escrow_balance(), e8(10) as u128);
    // ...and now the single slot is taken: any further deposit hits the cap.
    w.approve_bridge(user, e8(10));
    let args = w.deposit_args(e8(10), &recipient);
    assert_eq!(w.bridge_to_base(user, args), Err(BridgeError::CapacityExceeded { available_e8s: 0 }));

    // (b) Allowance but not enough balance: a user with 0.0003 icUSD approves, then asks for 2.
    let poor = test_principal(0xB3);
    w.ledger_transfer(user, account(poor), 30_000);
    w.approve(poor, w.bridge, e8(5) as u128); // costs the 10_000 fee
    let poor_before = w.user_balance(poor);
    assert_eq!(poor_before, 20_000);
    // restore wide limits so the capacity check is not what rejects it
    w.set_limits(default_limits()).expect("limits");
    let args = w.deposit_args(e8(2), &recipient);
    match w.bridge_to_base(poor, args) {
        Ok(failed) => assert!(
            matches!(&failed.status, DepositStatus::PullFailed { reason } if reason.to_lowercase().contains("insufficient funds")),
            "{failed:?}"
        ),
        Err(BridgeError::InsufficientFunds { balance_e8s, required_e8s }) => {
            assert_eq!((balance_e8s, required_e8s), (poor_before as u64, e8(2) + LEDGER_FEE));
        }
        other => panic!("unexpected result for an underfunded request: {other:?}"),
    }
    assert_eq!(w.user_balance(poor), poor_before, "no funds moved");
    assert_eq!(w.escrow_balance(), e8(10) as u128, "escrow holds only the one good deposit");
    assert!(w.bridge_info().halted.is_none(), "an ordinary user shortfall must not halt the bridge");

    // (c) Amount bounds and recipients (nothing is recorded for these).
    let n_before = w.list_my_deposits(user, None, 100).len();
    let args = w.deposit_args(e8(1_001), &recipient);
    assert_eq!(w.bridge_to_base(user, args), Err(BridgeError::AmountAboveMaximum { max_e8s: e8(1_000) }));
    let args = w.deposit_args(e8(1) - 1, &recipient);
    assert_eq!(w.bridge_to_base(user, args), Err(BridgeError::AmountBelowMinimum { min_e8s: e8(1) }));
    let args = w.deposit_args(e8(2), "0x0000000000000000000000000000000000000000");
    assert!(matches!(w.bridge_to_base(user, args), Err(BridgeError::InvalidRecipient(_))));
    let contract = w.token.address.clone();
    let args = w.deposit_args(e8(2), &contract);
    assert!(matches!(w.bridge_to_base(user, args), Err(BridgeError::InvalidRecipient(_))), "minting to the contract is refused");
    let args = w.deposit_args(e8(2), "not-an-address");
    assert!(matches!(w.bridge_to_base(user, args), Err(BridgeError::InvalidRecipient(_))));
    assert_eq!(w.list_my_deposits(user, None, 100).len(), n_before);

    // (d) Anonymous callers.
    let args = w.deposit_args(e8(2), &recipient);
    assert_eq!(w.bridge_to_base(Principal::anonymous(), args), Err(BridgeError::Anonymous));
    assert_eq!(w.request_refund(Principal::anonymous(), 0), Err(BridgeError::Anonymous));
    assert_eq!(w.notify_hint(Principal::anonymous(), &hex0x(&[1u8; 32])), Err(BridgeError::Anonymous));

    // (e) Every admin method refuses a non-admin (and the anonymous principal).
    let probe = |w: &mut BridgeWorld, who: Principal, want: BridgeError| {
        let got = vec![
            call_err::<_, ()>(w, who, "admin_set_pauses", (Pauses::default(),)),
            call_err::<_, ()>(w, who, "admin_set_limits", (default_limits(),)),
            call_err::<_, ()>(w, who, "admin_set_fees", (BridgeFees { deposit_fee_e8s: 1, release_fee_e8s: 1 },)),
            call_err::<_, ContractBindingView>(w, who, "admin_bind_contract", (contract.clone(),)),
            call_err::<_, u64>(w, who, "admin_sweep_fees", (1u64,)),
            call_err::<_, u64>(w, who, "admin_submit_contract_action", (ContractAdminAction::PauseMint,)),
            call_err::<_, ()>(w, who, "admin_cancel_stuck_tx", (0u64,)),
            call_err::<_, ()>(w, who, "admin_clear_lane_halt", (LaneKind::Minter,)),
            call_err::<_, DepositView>(w, who, "admin_retry_deposit", (0u64,)),
            call_err::<_, SupplyAudit>(w, who, "admin_reconcile", ()),
            call_err::<_, SupplyAudit>(w, who, "admin_clear_halt", ()),
            call_err::<_, ()>(w, who, "admin_run_tick", ()),
            call_err::<_, ()>(w, who, "set_admins", (vec![who],)),
        ];
        for (i, e) in got.iter().enumerate() {
            assert_eq!(*e, want, "admin method #{i} for {who}");
        }
    };
    probe(&mut w, user, BridgeError::Unauthorized);
    probe(&mut w, Principal::anonymous(), BridgeError::Anonymous);
    // Nothing a non-admin tried changed the configuration.
    let info = w.bridge_info();
    assert_eq!((info.pauses.clone(), info.limits.clone(), info.fees.clone()), (Pauses::default(), default_limits(), initial_info.fees));
    // The real admin is of course still able to act.
    w.update::<_, BridgeResult<()>>(admin, "admin_run_tick", ()).expect("admin tick");
}

// ---------------------------------------------------------------------------
// 5. Caps and pauses
// ---------------------------------------------------------------------------

#[test]
fn capacity_window_and_deposit_pause_are_enforced() {
    let mut opts = SetupOptions::default();
    opts.limits = BridgeLimits { max_deposit_e8s: e8(100), max_outstanding_e8s: e8(150), ..default_limits() };
    let mut w = BridgeWorld::setup(opts);
    let (user, recipient) = (w.user, w.holder(0));
    w.approve(user, w.bridge, e8(1_000) as u128);

    let args = w.deposit_args(e8(100), &recipient);
    assert_eq!(w.bridge_to_base(user, args).expect("100").status, DepositStatus::Escrowed);
    // 100 used of 150: another 100 does not fit, 50 does.
    let args = w.deposit_args(e8(100), &recipient);
    assert_eq!(w.bridge_to_base(user, args), Err(BridgeError::CapacityExceeded { available_e8s: e8(50) }));
    let args = w.deposit_args(e8(50), &recipient);
    assert_eq!(w.bridge_to_base(user, args).expect("50").status, DepositStatus::Escrowed);
    let args = w.deposit_args(e8(1), &recipient);
    assert_eq!(w.bridge_to_base(user, args), Err(BridgeError::CapacityExceeded { available_e8s: 0 }));
    assert_eq!(w.escrow_balance(), e8(150) as u128, "only admitted deposits were pulled");

    // Paused deposits are refused (the pause check comes before the capacity check).
    w.set_pauses(Pauses { deposits: true, ..Pauses::default() }).expect("pause deposits");
    let args = w.deposit_args(e8(1), &recipient);
    assert!(matches!(w.bridge_to_base(user, args), Err(BridgeError::Paused(_))));
    w.set_pauses(Pauses::default()).expect("unpause");
    let args = w.deposit_args(e8(1), &recipient);
    assert!(matches!(w.bridge_to_base(user, args), Err(BridgeError::CapacityExceeded { .. })), "back to the capacity error");

    // Daily window: 150 admitted so far; lower the window limit below that.
    w.set_limits(BridgeLimits { mint_window_limit_e8s: e8(120), ..default_limits() }).expect("limits");
    let args = w.deposit_args(e8(10), &recipient);
    match w.bridge_to_base(user, args) {
        Err(BridgeError::RateLimited { available_e8s, .. }) => assert_eq!(available_e8s, 0),
        other => panic!("expected RateLimited, got {other:?}"),
    }
    assert_eq!(w.escrow_balance(), e8(150) as u128);
    let info = w.bridge_info();
    assert_eq!(info.mint_window_used_e8s, e8(150));
}

#[test]
fn releases_paused_keeps_a_finalized_bridge_out_pending_then_releases_once() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, recipient) = (w.user, w.holder(0));
    let (a, b) = (e8(50), e8(20));
    w.deposit_and_deliver(user, a, &recipient);

    let nonce = w.fresh_nonce();
    let out = w.bridge_out_from(&recipient, b as u128, user.as_slice(), [0u8; 32], nonce);
    assert!(out.success);
    w.set_pauses(Pauses { releases: true, ..Pauses::default() }).expect("pause releases");
    w.mine_finalize();
    // The observer still records the finalized burn; only the ICP transfer waits.
    let pending = w.wait_release(&out.transfer_id, "recorded Pending", |r| r.status == ReleaseStatus::Pending);
    assert!(pending.release_block_index.is_none());
    let (user_bal, esc, supply) = (w.user_balance(user), w.escrow_balance(), w.ledger_total_supply());
    w.ticks(6);
    let r = w.get_release(&out.transfer_id).expect("release");
    assert_eq!(r.status, ReleaseStatus::Pending, "still pending while paused");
    assert!(r.release_block_index.is_none() && r.amount_to_receive_e8s.is_none(), "no transfer was even prepared: {r:?}");
    assert_eq!((w.user_balance(user), w.escrow_balance(), w.ledger_total_supply()), (user_bal, esc, supply), "ledger untouched");
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [(a - b) as u128, 0, b as u128, 0, 0, 0, 0], "burn moved O to PR: {audit:?}");
    assert_eq!(audit.coverage_ok, Some(true));

    w.set_pauses(Pauses::default()).expect("unpause");
    let rel = w.wait_released(&out.transfer_id);
    assert_eq!(rel.amount_to_receive_e8s, Some(b - LEDGER_FEE));
    w.ticks(4);
    assert_eq!(w.user_balance(user) - user_bal, (b - LEDGER_FEE) as u128, "released exactly once");
    assert_eq!(w.escrow_balance(), (a - b) as u128);
    assert_eq!(w.list_releases_for_owner(user, None, 10).len(), 1);
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [(a - b) as u128, 0, 0, 0, 0, 0, 0], "{audit:?}");
}

#[test]
fn mints_paused_keeps_deposit_escrowed_and_sends_nothing() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, recipient) = (w.user, w.holder(0));
    w.set_pauses(Pauses { mints: true, ..Pauses::default() }).expect("pause mints");
    let a = e8(30);
    let dep = w.approve_and_deposit(user, a, &recipient);
    assert_eq!(dep.status, DepositStatus::Escrowed);
    let height = w.anvil.block_number("latest");
    w.ticks(6);
    let d = w.get_deposit(dep.id).expect("deposit");
    assert_eq!(d.status, DepositStatus::Escrowed, "no mint while paused: {d:?}");
    assert!(d.mint_tx_hashes.is_empty() && d.mint_stage.is_none());
    assert_eq!(w.anvil.block_number("latest"), height, "no Base transaction was sent");
    assert_eq!((w.minter_nonce(), w.base_total_supply()), (0, 0));
    assert_eq!(w.bridge_info().lane(LaneKind::Minter).unincluded_tx, None);
    assert_eq!(w.escrow_balance(), a as u128, "the pulled funds stay safely in escrow");
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [0, a as u128, 0, 0, 0, 0, 0], "A sits in pending-mint: {audit:?}");
    assert_eq!(audit.coverage_ok, Some(true));

    w.set_pauses(Pauses::default()).expect("unpause");
    w.wait_mint_included(dep.id);
    w.mine_finalize();
    w.wait_delivered(dep.id);
    assert_eq!((w.base_balance(&recipient), w.minter_nonce()), (a as u128, 1));
}

// ---------------------------------------------------------------------------
// 6. Held claims: re-mint to the Base sender, no ICP transfer
// ---------------------------------------------------------------------------

#[test]
fn bridge_out_to_bridge_or_minting_account_is_held_and_reminted() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, minting, bridge, sender) = (w.user, w.minting, w.bridge, w.holder(0));
    let a = e8(100);
    w.deposit_and_deliver(user, a, &sender);
    let (b1, b2) = (e8(30), e8(20));

    // Bridge-outs whose ICP destination is the bridge canister itself and the ledger's
    // minting account (a transfer there would burn the funds).
    let n1 = w.fresh_nonce();
    let out1 = w.bridge_out_from(&sender, b1 as u128, bridge.as_slice(), [0u8; 32], n1);
    let n2 = w.fresh_nonce();
    let out2 = w.bridge_out_from(&sender, b2 as u128, minting.as_slice(), [0u8; 32], n2);
    assert!(out1.success && out2.success);
    assert_eq!(w.base_balance(&sender), (a - b1 - b2) as u128, "both burned on Base");
    // Pause mints so the re-mint stays observable, then make the burns final.
    w.set_pauses(Pauses { mints: true, ..Pauses::default() }).expect("pause mints");
    w.mine_finalize();
    let r1 = w.wait_release(&out1.transfer_id, "Reminting", |r| matches!(r.status, ReleaseStatus::Reminting { .. }));
    let r2 = w.wait_release(&out2.transfer_id, "Reminting", |r| matches!(r.status, ReleaseStatus::Reminting { .. }));
    let (ReleaseStatus::Reminting { deposit_id: d1 }, ReleaseStatus::Reminting { deposit_id: d2 }) =
        (r1.status.clone(), r2.status.clone())
    else {
        unreachable!()
    };
    assert!(r1.destination.is_none() && r2.destination.is_none(), "no valid ICP destination");
    assert!(r1.release_block_index.is_none() && r2.release_block_index.is_none());
    // No ICP transfer happened: escrow, users and the minting account are untouched.
    assert_eq!(w.escrow_balance(), a as u128);
    assert_eq!(w.user_balance(minting), 0);
    assert_eq!(w.user_balance(bridge), 0);
    let remint = w.get_deposit(d1).expect("remint deposit");
    assert_eq!(remint.kind, DepositKindView::Remint { transfer_id: out1.transfer_id.clone() });
    assert_eq!((remint.owner, remint.from.clone()), (bridge, None));
    assert_eq!((remint.base_recipient.as_str(), remint.amount_e8s, remint.deposit_fee_e8s), (sender.as_str(), b1, 0));
    assert_eq!((remint.mint_amount_e8s, remint.status.clone()), (b1, DepositStatus::Escrowed));
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [(a - b1 - b2) as u128, (b1 + b2) as u128, 0, 0, 0, 0, 0], "held claims became pending mints: {audit:?}");
    assert_eq!(audit.coverage_ok, Some(true));
    assert_eq!(audit.escrow(), Some(a as u128));

    // Unpause: the sender receives exactly B back on Base, once each.
    w.set_pauses(Pauses::default()).expect("unpause");
    w.wait_mint_included(d1);
    w.wait_mint_included(d2);
    w.mine_finalize();
    w.wait_delivered(d1);
    w.wait_delivered(d2);
    let r1 = w.wait_release(&out1.transfer_id, "Reminted", |r| matches!(r.status, ReleaseStatus::Reminted { .. }));
    let r2 = w.wait_release(&out2.transfer_id, "Reminted", |r| matches!(r.status, ReleaseStatus::Reminted { .. }));
    assert_eq!(r1.status, ReleaseStatus::Reminted { deposit_id: d1 });
    assert_eq!(r2.status, ReleaseStatus::Reminted { deposit_id: d2 });
    assert_eq!(w.base_balance(&sender), a as u128, "the sender is made whole");
    assert_eq!(w.base_total_supply(), a as u128);
    assert_eq!(w.minter_nonce(), 3, "one deposit mint + two re-mints");
    // Re-mint ids are distinct from deposit mint ids and consumed on Base.
    let dv = (w.get_deposit(d1).unwrap(), w.get_deposit(d2).unwrap());
    assert_ne!(dv.0.mint_id, dv.1.mint_id);
    assert!(w.base_mint_consumed(&dv.0.mint_id) && w.base_mint_consumed(&dv.1.mint_id));
    // Still no ICP movement; buckets consistent, coverage holds.
    assert_eq!(w.escrow_balance(), a as u128);
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [a as u128, 0, 0, 0, 0, 0, 0], "{audit:?}");
    assert_eq!(audit.coverage_ok, Some(true));
    assert_eq!(audit.base_supply_within_bound, Some(true));
    let log = w.audit_log_all();
    let has = |f: &dyn Fn(&Transition) -> bool| log.iter().any(|e| e.transition.as_ref().map(f).unwrap_or(false));
    assert!(has(&|t| *t == Transition::BridgeOutHeld { amount: b1 }));
    assert!(has(&|t| *t == Transition::BridgeOutHeld { amount: b2 }));
    assert!(has(&|t| *t == Transition::RemintStarted { amount: b1 }));
    assert!(has(&|t| *t == Transition::RemintDelivered { amount: b2 }));
    assert!(!has(&|t| matches!(t, Transition::ReleaseCompleted { .. })), "no ICP release ever happened");
    assert!(w.bridge_info().halted.is_none());
}

// ---------------------------------------------------------------------------
// 7. Contract paused: preflight hold, then owner refund
// ---------------------------------------------------------------------------

#[test]
fn contract_paused_preflight_holds_then_owner_refunds() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, user2, ok_recipient, recipient) = (w.user, w.user2, w.holder(0), w.holder(1));
    // One normal, delivered deposit first (it must stay untouched by everything below).
    let d0 = w.deposit_and_deliver(user, e8(10), &ok_recipient);
    assert_eq!((w.minter_nonce(), w.base_total_supply()), (1, e8(10) as u128));

    // TEST-ONLY admin simulation: anvil impersonates the bridge's own admin address and
    // pauses minting on the contract. This is NOT a canister action.
    w.contract_pause_mint_as_admin();
    assert!(w.contract_mint_paused());
    let height = w.anvil.block_number("latest");

    let a = e8(30);
    let dep = w.approve_and_deposit(user, a, &recipient);
    assert_eq!(dep.status, DepositStatus::Escrowed);
    let held = w.wait_deposit(dep.id, "MintHeld", |d| matches!(d.status, DepositStatus::MintHeld { .. }));
    let DepositStatus::MintHeld { reason } = &held.status else { unreachable!() };
    let paused_sel = selector_hex("MintIsPaused()");
    assert!(reason.contains(&paused_sel), "hold reason carries the revert selector {paused_sel}: {reason}");
    assert!(held.mint_tx_hashes.is_empty() && held.mint_stage.is_none(), "no tx was signed or sent");
    assert_eq!(w.anvil.block_number("latest"), height, "nothing hit the chain");
    assert_eq!(w.minter_nonce(), 1);
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [e8(10) as u128, a as u128, 0, 0, 0, 0, 0], "A is parked in pending-mint: {audit:?}");
    assert_eq!(audit.coverage_ok, Some(true));

    // Refund guards.
    assert_eq!(w.request_refund(user2, dep.id), Err(BridgeError::NotOwner));
    assert!(matches!(w.request_refund(user, d0.id), Err(BridgeError::InvalidState(_))), "a delivered deposit is not refundable");
    assert_eq!(w.request_refund(user, 9_999), Err(BridgeError::NotFound));
    assert_eq!(w.escrow_balance(), (e8(10) + a) as u128, "nothing moved yet");

    // The owner refunds: A - ledger fee comes back, the escrow pays A.
    let before = w.user_balance(user);
    let refunded = w.request_refund(user, dep.id).expect("refund");
    assert_eq!(refunded.status, DepositStatus::Refunded, "{refunded:?}");
    assert_eq!(refunded.refund_amount_e8s, Some(a - LEDGER_FEE));
    assert!(refunded.refund_block_index.is_some());
    assert_eq!(w.user_balance(user) - before, (a - LEDGER_FEE) as u128);
    assert_eq!(w.escrow_balance(), e8(10) as u128, "only the delivered deposit's backing remains");
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [e8(10) as u128, 0, 0, 0, 0, 0, 0], "{audit:?}");
    assert_eq!(audit.coverage_ok, Some(true));
    assert!(matches!(w.request_refund(user, dep.id), Err(BridgeError::InvalidState(_))), "no double refund");
    assert_eq!(w.user_balance(user) - before, (a - LEDGER_FEE) as u128);

    // A refunded deposit is never minted later, even once the contract is unpaused and the
    // 10 minute hold-retry delay has passed.
    w.contract_unpause_mint_as_admin();
    assert!(!w.contract_mint_paused());
    w.advance_secs(11 * 60);
    w.ticks(6);
    let d = w.get_deposit(dep.id).unwrap();
    assert_eq!(d.status, DepositStatus::Refunded);
    assert_eq!((w.minter_nonce(), w.base_total_supply(), w.base_balance(&recipient)), (1, e8(10) as u128, 0));
    assert!(w.bridge_info().halted.is_none());
}

#[test]
fn preflight_hold_retries_automatically_once_the_contract_is_unpaused() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, recipient) = (w.user, w.holder(0));
    w.contract_pause_mint_as_admin(); // TEST-ONLY admin simulation (see module docs)
    let dep = w.approve_and_deposit(user, e8(15), &recipient);
    w.wait_deposit(dep.id, "MintHeld", |d| matches!(d.status, DepositStatus::MintHeld { .. }));
    w.contract_unpause_mint_as_admin();
    // Before the 10 minute retry delay the hold stays.
    w.ticks(3);
    assert!(matches!(w.get_deposit(dep.id).unwrap().status, DepositStatus::MintHeld { .. }));
    // After it the deposit is requeued by the timer and delivered with the SAME mint id.
    let mint_id = w.get_deposit(dep.id).unwrap().mint_id;
    w.advance_secs(11 * 60);
    w.wait_mint_included(dep.id);
    w.mine_finalize();
    let done = w.wait_delivered(dep.id);
    assert_eq!(done.mint_id, mint_id);
    assert_eq!((w.base_balance(&recipient), w.minter_nonce()), (e8(15) as u128, 1));
}

// ---------------------------------------------------------------------------
// 8. Upgrade mid-flight with outstanding liabilities
// ---------------------------------------------------------------------------

#[test]
fn upgrade_mid_flight_preserves_state_and_completes_exactly_once() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, holder0, holder1) = (w.user, w.holder(0), w.holder(1));
    let (a0, b, a1) = (e8(100), e8(40), e8(50));

    // d0 delivered; a finalized bridge-out held Pending by pausing releases; d1 mined but
    // not final (MintInFlight).
    let d0 = w.deposit_and_deliver(user, a0, &holder0);
    let nonce = w.fresh_nonce();
    let out = w.bridge_out_from(&holder0, b as u128, user.as_slice(), [0u8; 32], nonce);
    w.set_pauses(Pauses { releases: true, ..Pauses::default() }).expect("pause releases");
    w.mine_finalize();
    w.wait_release(&out.transfer_id, "Pending", |r| r.status == ReleaseStatus::Pending);
    let d1 = w.approve_and_deposit(user, a1, &holder1);
    let d1 = w.wait_mint_included(d1.id);
    assert_eq!(d1.status, DepositStatus::MintInFlight);

    // Snapshot everything the frontend and operators rely on.
    let (dv0, dv1) = (w.get_deposit(d0.id).unwrap(), w.get_deposit(d1.id).unwrap());
    let rel = w.get_release(&out.transfer_id).unwrap();
    let audit = w.supply_audit();
    let log = w.audit_log_all();
    let info = w.bridge_info();
    let minter = info.lane(LaneKind::Minter).clone();
    assert_eq!((minter.next_nonce, minter.unfinalized_txs, minter.unincluded_tx), (Some(2), 1, None));
    let user_bal = w.user_balance(user);
    let (esc, ledger_supply) = (w.escrow_balance(), w.ledger_total_supply());
    assert_eq!(audit.pending_release_e8s, candid::Nat::from(b));
    assert_eq!(audit.pending_mint_e8s, candid::Nat::from(a1));

    w.upgrade_bridge_same_wasm();

    // Nothing was lost: same records, same audit snapshot, same lane position.
    assert_eq!(w.get_deposit(d0.id).unwrap(), dv0);
    assert_eq!(w.get_deposit(d1.id).unwrap(), dv1);
    assert_eq!(w.get_release(&out.transfer_id).unwrap(), rel);
    assert_eq!(w.supply_audit(), audit, "stored accounting survives the upgrade");
    let log2 = w.audit_log_all();
    assert_eq!(&log2[..log.len()], &log[..], "every earlier audit entry is unchanged");
    assert_eq!(log2.len(), log.len() + 1, "post_upgrade appends exactly one entry");
    assert_eq!(log2.last().unwrap().note, "upgraded");
    let info2 = w.bridge_info();
    assert_eq!(info2.lane(LaneKind::Minter), &minter, "lane nonce and unfinalized txs preserved");
    assert_eq!(info2.pauses, info.pauses);
    assert_eq!(info2.contract, info.contract);
    assert_eq!(info2.observer.processed_count, info.observer.processed_count);
    assert_eq!(info2.observer.cursor_block, info.observer.cursor_block);
    assert_eq!(info2.escrow_account, info.escrow_account);
    assert_eq!((w.user_balance(user), w.escrow_balance(), w.ledger_total_supply()), (user_bal, esc, ledger_supply));

    // Unpause and finish both flows: exactly once each.
    w.set_pauses(Pauses::default()).expect("unpause");
    w.mine_finalize();
    let done = w.wait_delivered(d1.id);
    assert_eq!(done.mint_tx_hashes, dv1.mint_tx_hashes, "the same signed tx finalized; nothing re-signed");
    let rel = w.wait_released(&out.transfer_id);
    assert_eq!(rel.amount_to_receive_e8s, Some(b - LEDGER_FEE));
    w.ticks(4);
    assert_eq!(w.user_balance(user) - user_bal, (b - LEDGER_FEE) as u128, "released exactly once");
    assert_eq!(w.minter_nonce(), 2, "exactly two mint txs ever: no duplicate after the upgrade");
    assert_eq!(w.base_total_supply(), (a0 - b + a1) as u128);
    assert_eq!(w.base_balance(&holder1), a1 as u128);
    assert_eq!(w.escrow_balance(), (a0 - b + a1) as u128);
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [(a0 - b + a1) as u128, 0, 0, 0, 0, 0, 0], "{audit:?}");
    assert_eq!(audit.coverage_ok, Some(true));
    assert_eq!(audit.base_supply_within_bound, Some(true));
    assert!(w.bridge_info().halted.is_none());
}

// ---------------------------------------------------------------------------
// 9. Reload-style recovery: what the frontend reads after a page reload
// ---------------------------------------------------------------------------

#[test]
fn history_lists_are_newest_first_with_correct_statuses() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, user2, h0, h1) = (w.user, w.user2, w.holder(0), w.holder(1));

    let u1 = w.deposit_and_deliver(user, e8(30), &h0);
    let u2 = w.deposit_and_deliver(user, e8(20), &h0);
    let v1 = w.deposit_and_deliver(user2, e8(10), &h1);
    // A third deposit of the user's is still waiting for its mint.
    w.set_pauses(Pauses { mints: true, ..Pauses::default() }).expect("pause mints");
    let u3 = w.approve_and_deposit(user, e8(5), &h0);
    assert_eq!((u1.id, u2.id, v1.id, u3.id), (0, 1, 2, 3));

    let list = w.list_my_deposits(user, None, 10);
    assert_eq!(list.iter().map(|d| d.id).collect::<Vec<_>>(), vec![3, 1, 0], "newest first, only the caller's");
    assert_eq!(
        list.iter().map(|d| d.status.clone()).collect::<Vec<_>>(),
        vec![DepositStatus::Escrowed, DepositStatus::Delivered, DepositStatus::Delivered]
    );
    for d in &list {
        assert_eq!(Some(d), w.get_deposit(d.id).as_ref(), "list entries equal get_deposit");
        assert_eq!(d.owner, user);
    }
    assert_eq!(list[1].amount_e8s, e8(20));
    assert_eq!(list[2].delivered.as_ref().map(|i| i.tx_hash.clone()), Some(list[2].mint_tx_hashes[0].clone()));
    // Paging.
    assert_eq!(w.list_my_deposits(user, Some(3), 1).iter().map(|d| d.id).collect::<Vec<_>>(), vec![1]);
    assert_eq!(w.list_my_deposits(user, Some(1), 10).iter().map(|d| d.id).collect::<Vec<_>>(), vec![0]);
    assert!(w.list_my_deposits(user, Some(0), 10).is_empty());
    assert_eq!(w.list_my_deposits(user2, None, 10).iter().map(|d| d.id).collect::<Vec<_>>(), vec![2]);
    assert!(w.list_my_deposits(Principal::anonymous(), None, 10).is_empty());
    assert!(w.list_my_deposits(w.admin, None, 10).is_empty());

    // Releases: seq 1 to user, seq 2 to user2 (both released), seq 3 to user (held Pending).
    let (_, r1) = w.bridge_out_and_release(&h0, e8(10) as u128, user);
    let (_, r2) = w.bridge_out_and_release(&h0, e8(5) as u128, user2);
    w.set_pauses(Pauses { mints: true, releases: true, ..Pauses::default() }).expect("pause releases");
    let n3 = w.fresh_nonce();
    let out3 = w.bridge_out_from(&h0, e8(8) as u128, user.as_slice(), [0u8; 32], n3);
    w.mine_finalize();
    let r3 = w.wait_release(&out3.transfer_id, "Pending", |r| r.status == ReleaseStatus::Pending);
    assert_eq!((r1.sequence, r2.sequence, r3.sequence), (1, 2, 3));

    let rl = w.list_releases_for_owner(user, None, 10);
    assert_eq!(rl.iter().map(|r| r.sequence).collect::<Vec<_>>(), vec![3, 1], "newest first by sequence");
    assert_eq!(
        rl.iter().map(|r| r.status.clone()).collect::<Vec<_>>(),
        vec![ReleaseStatus::Pending, ReleaseStatus::Released]
    );
    for r in &rl {
        assert_eq!(Some(r), w.get_release(&r.transfer_id).as_ref());
        assert_eq!(r.destination, Some(account(user)));
        assert_eq!(r.base_sender, h0);
    }
    assert_eq!((rl[0].amount_e8s, rl[1].amount_e8s), (e8(8), e8(10)));
    assert_eq!(w.list_releases_for_owner(user, Some(3), 10).iter().map(|r| r.sequence).collect::<Vec<_>>(), vec![1]);
    assert!(w.list_releases_for_owner(user, Some(1), 10).is_empty());
    let r2l = w.list_releases_for_owner(user2, None, 10);
    assert_eq!(r2l.len(), 1);
    assert_eq!((r2l[0].sequence, r2l[0].status.clone()), (2, ReleaseStatus::Released));
    assert!(w.list_releases_for_owner(w.admin, None, 10).is_empty());

    // Everything finishes after unpausing, and the lists reflect it.
    w.set_pauses(Pauses::default()).expect("unpause");
    w.wait_released(&out3.transfer_id);
    w.wait_mint_included(u3.id);
    w.mine_finalize();
    w.wait_delivered(u3.id);
    assert!(w.list_my_deposits(user, None, 10).iter().all(|d| d.status == DepositStatus::Delivered));
    assert!(w.list_releases_for_owner(user, None, 10).iter().all(|r| r.status == ReleaseStatus::Released));
    assert_eq!(w.base_total_supply(), (e8(30 + 20 + 10 + 5) - e8(10 + 5 + 8)) as u128);
}

// ===========================================================================
// Adversarial extras (beyond the nine required scenarios)
// ===========================================================================

// ---------------------------------------------------------------------------
// 10. Observer completeness on a real chain: many chunks, several burns in ONE block
// ---------------------------------------------------------------------------

#[test]
fn observer_finds_every_burn_across_chunks_and_inside_one_block() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, h0) = (w.user, w.holder(0));
    w.deposit_and_deliver(user, e8(60), &h0);

    // Tiny scan chunks so the backlog needs several `eth_getLogs` ranges and several passes.
    let mut oc = w.bridge_info().observer_config;
    oc.logs_chunk_blocks = 3;
    w.admin_call::<_, BridgeResult<()>>("admin_set_observer_config", (oc,)).expect("observer config");

    // burn #1, a gap of empty blocks, then burns #2 and #3 in the SAME block, a longer gap, burn #4.
    let n1 = w.fresh_nonce();
    let t1 = w.bridge_out_from(&h0, e8(5) as u128, user.as_slice(), [0u8; 32], n1);
    w.mine(7);
    let (n2, n3) = (w.fresh_nonce(), w.fresh_nonce());
    w.set_automine(false);
    let h2 = w.send_bridge_out_unmined(&h0, e8(3) as u128, user.as_slice(), [0u8; 32], n2);
    let h3 = w.send_bridge_out_unmined(&h0, e8(2) as u128, w.user2.as_slice(), [0u8; 32], n3);
    w.mine_block();
    w.set_automine(true);
    let (rc2, rc3) = (w.receipt(&h2).expect("receipt 2"), w.receipt(&h3).expect("receipt 3"));
    assert_eq!((rc2["status"].as_str(), rc3["status"].as_str()), (Some("0x1"), Some("0x1")));
    assert_eq!(rc2["blockNumber"], rc3["blockNumber"], "two burns in one block");
    w.mine(20);
    let n4 = w.fresh_nonce();
    let t4 = w.bridge_out_from(&h0, e8(1) as u128, user.as_slice(), [0u8; 32], n4);
    assert_eq!(w.base_bridge_out_count(), 4);
    w.mine_finalize();

    let tid = |n: [u8; 32]| hex0x(&w.token.compute_transfer_id(&w.anvil, &h0, n));
    let (tid2, tid3) = (tid(n2), tid(n3));
    for (t, seq) in [(&t1.transfer_id, 1u64), (&tid2, 2), (&tid3, 3), (&t4.transfer_id, 4)] {
        let r = w.wait_release(t, "Released", |r| r.status == ReleaseStatus::Released);
        assert_eq!(r.sequence, seq, "sequences are gapless and in chain order");
    }
    let (r2, r3) = (w.get_release(&tid2).unwrap(), w.get_release(&tid3).unwrap());
    assert_eq!(r2.base_block_number, r3.base_block_number, "same block");
    assert!(r3.log_index > r2.log_index, "distinct log positions in that block");
    assert_eq!(r2.destination, Some(account(user)));
    assert_eq!(r3.destination, Some(account(w.user2)));
    let info = w.bridge_info();
    assert_eq!(info.observer.processed_count, 4);
    assert!(info.halted.is_none(), "{:?}", info.halted);
    assert_eq!(w.list_releases_for_owner(user, None, 10).len(), 3);
    assert_eq!(w.list_releases_for_owner(w.user2, None, 10).len(), 1);
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [e8(60 - 11) as u128, 0, 0, 0, 0, 0, 0], "{audit:?}");
    assert_eq!(w.escrow_balance(), e8(49) as u128);
    assert_eq!(audit.coverage_ok, Some(true));
}

// ---------------------------------------------------------------------------
// 11. Exact e8s fee boundaries: dust is held and re-minted, one e8 over is released
// ---------------------------------------------------------------------------

#[test]
fn exact_fee_boundaries_dust_is_held_and_one_e8_over_is_released() {
    let (dep_fee, rel_fee) = (E8 - 1, E8); // deposit fee just under the 1 icUSD minimum deposit
    let mut opts = SetupOptions::default();
    opts.fees = BridgeFees { deposit_fee_e8s: dep_fee, release_fee_e8s: rel_fee };
    let mut w = BridgeWorld::setup(opts);
    let (user, h0) = (w.user, w.holder(0));

    // A = exactly the minimum deposit: only 1 e8s is minted.
    let d1 = w.deposit_and_deliver(user, E8, &h0);
    assert_eq!((d1.amount_e8s, d1.deposit_fee_e8s, d1.mint_amount_e8s), (E8, dep_fee, 1));
    assert_eq!(w.base_balance(&h0), 1);
    // A fee equal to the deposit would leave nothing to mint and is refused up front.
    w.set_fees(BridgeFees { deposit_fee_e8s: dep_fee, release_fee_e8s: rel_fee }).expect("fees");
    assert!(matches!(
        w.set_fees(BridgeFees { deposit_fee_e8s: E8, release_fee_e8s: rel_fee }),
        Err(BridgeError::InvalidArgument(_))
    ));
    // Second deposit to have something to burn.
    let d2 = w.deposit_and_deliver(user, e8(10), &h0);
    assert_eq!(d2.mint_amount_e8s, e8(10) - dep_fee);
    let held_total = 1 + (e8(10) - dep_fee) as u128;
    assert_eq!(w.base_balance(&h0), held_total);

    // B = release fee + ledger fee exactly: nothing would be left for the user, so the claim
    // is held and re-minted to the Base sender (no ICP transfer).
    let user_before = w.user_balance(user);
    let escrow_before = w.escrow_balance();
    let b_dust = rel_fee + LEDGER_FEE;
    let nonce = w.fresh_nonce();
    let dust = w.bridge_out_from(&h0, b_dust as u128, user.as_slice(), [0u8; 32], nonce);
    assert!(dust.success);
    w.mine_finalize();
    let r = w.wait_release(&dust.transfer_id, "Reminted", |r| matches!(r.status, ReleaseStatus::Reminting { .. } | ReleaseStatus::Reminted { .. }));
    let (ReleaseStatus::Reminting { deposit_id } | ReleaseStatus::Reminted { deposit_id }) = r.status else { unreachable!() };
    w.wait_mint_included(deposit_id);
    w.mine_finalize();
    w.wait_delivered(deposit_id);
    w.wait_release(&dust.transfer_id, "Reminted", |r| matches!(r.status, ReleaseStatus::Reminted { .. }));
    assert_eq!(w.base_balance(&h0), held_total, "the sender is made whole");
    assert_eq!((w.user_balance(user), w.escrow_balance()), (user_before, escrow_before), "no ICP moved");

    // B = release fee + ledger fee + 1: exactly 1 e8s reaches the user.
    let b_ok = rel_fee + LEDGER_FEE + 1;
    let nonce = w.fresh_nonce();
    let ok = w.bridge_out_from(&h0, b_ok as u128, user.as_slice(), [0u8; 32], nonce);
    w.mine_finalize();
    let rel = w.wait_released(&ok.transfer_id);
    assert_eq!((rel.amount_to_receive_e8s, rel.release_fee_e8s), (Some(1), rel_fee));
    assert_eq!(w.user_balance(user) - user_before, 1, "exactly one e8s");
    assert_eq!(escrow_before - w.escrow_balance(), (b_ok - rel_fee) as u128, "escrow pays B - release fee");
    let audit = w.reconcile();
    let fees = (2 * dep_fee + rel_fee) as u128;
    assert_eq!(audit.fees(), fees, "{audit:?}");
    assert_eq!(audit.outstanding(), held_total - b_ok as u128);
    assert_eq!(audit.coverage_ok, Some(true));
    assert_eq!(w.base_total_supply(), held_total - b_ok as u128);
    assert!(w.bridge_info().halted.is_none());
}

// ---------------------------------------------------------------------------
// 12. Unbacked Base supply (simulated compromised minter) halts the bridge
// ---------------------------------------------------------------------------

#[test]
fn unbacked_base_supply_halts_the_bridge_and_nothing_leaves_escrow() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, h0, attacker) = (w.user, w.holder(0), w.holder(1));
    w.deposit_and_deliver(user, e8(40), &h0);
    let audit = w.reconcile();
    assert_eq!(audit.base_supply_within_bound, Some(true));
    assert!(w.bridge_info().halted.is_none());

    // SIMULATED ATTACK (test-only): anvil impersonates the bridge's minter address and mints
    // 5 icUSD the escrow does not back. The canister never signed this transaction.
    let minter = w.base_addrs.minter.clone();
    let data = support::contract::encode_mint(&attacker, e8(5) as u128, parse_hex32(&hex0x(&[0xEE; 32])));
    let tx = w.impersonate_send(&minter, &w.token.address.clone(), &data);
    assert!(w.anvil.tx_succeeded(&tx));
    w.mine_finalize();
    assert_eq!(w.base_total_supply(), e8(45) as u128);

    // The next supply check sees supply > outstanding + in-flight and halts everything.
    let audit = w.reconcile();
    assert_eq!(audit.base_supply_within_bound, Some(false), "{audit:?}");
    let halted = w.bridge_info().halted.expect("bridge halted");
    assert!(halted.contains("exceeds outstanding"), "{halted}");
    let escrow = w.escrow_balance();
    assert_eq!(escrow, e8(40) as u128);

    // While halted: no new deposits, no refunds, no un-pausing, no clearing the halt.
    w.approve_bridge(user, e8(5));
    let args = w.deposit_args(e8(5), &h0);
    assert!(matches!(w.bridge_to_base(user, args), Err(BridgeError::Halted(_))));
    assert!(matches!(w.request_refund(user, 0), Err(BridgeError::Halted(_)) | Err(BridgeError::InvalidState(_))));
    let r = w.set_pauses(Pauses { releases: true, ..Pauses::default() });
    assert!(r.is_ok(), "pausing is always allowed");
    assert!(matches!(w.set_pauses(Pauses::default()), Err(BridgeError::Halted(_))), "un-pausing needs the halt cleared");
    let r: BridgeResult<SupplyAudit> = w.admin_call("admin_clear_halt", ());
    assert!(matches!(r, Err(BridgeError::Halted(_))), "the halt cannot be cleared while supply is unbacked: {r:?}");

    // The attacker burns the unbacked tokens to redeem escrow. The burn is final and recorded,
    // but while halted nothing is paid out.
    let nonce = w.fresh_nonce();
    let out = w.bridge_out_from(&attacker, e8(5) as u128, w.user2.as_slice(), [0u8; 32], nonce);
    assert!(out.success);
    w.mine_finalize();
    w.ticks(8);
    if let Some(r) = w.get_release(&out.transfer_id) {
        assert_ne!(r.status, ReleaseStatus::Released, "unbacked tokens must not redeem escrow: {r:?}");
    }
    assert_eq!(w.user_balance(w.user2), e8(1_000) as u128, "no ICP reached the redeemer");
    assert_eq!(w.escrow_balance(), escrow, "escrow untouched");
    assert!(w.bridge_info().halted.is_some());
}

// ---------------------------------------------------------------------------
// 13. Daily windows: mint admission window and release window, then the window rolls
// ---------------------------------------------------------------------------

#[test]
fn mint_and_release_windows_defer_excess_and_roll_after_a_day() {
    let mut opts = SetupOptions::default();
    opts.limits = BridgeLimits {
        mint_window_limit_e8s: e8(25),
        release_window_limit_e8s: e8(15),
        ..default_limits()
    };
    let mut w = BridgeWorld::setup(opts);
    let (user, h0) = (w.user, w.holder(0));
    w.approve(user, w.bridge, e8(1_000) as u128);

    // Admission window: 25 icUSD per day.
    let args = w.deposit_args(e8(25), &h0);
    let d = w.bridge_to_base(user, args).expect("25 fits exactly");
    assert_eq!(d.status, DepositStatus::Escrowed);
    let args = w.deposit_args(e8(1), &h0);
    match w.bridge_to_base(user, args) {
        Err(BridgeError::RateLimited { available_e8s: 0, window_resets_at_ns }) => {
            assert_eq!(window_resets_at_ns, w.bridge_info().window_resets_at_ns);
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
    w.wait_mint_included(d.id);
    w.mine_finalize();
    w.wait_delivered(d.id);

    // Release window: 15 icUSD per day. 10 goes, the next 10 waits, a following 5 still fits.
    let release = |w: &mut BridgeWorld, amount: u64| {
        let nonce = w.fresh_nonce();
        let h0 = w.holder(0);
        let user = w.user;
        w.bridge_out_from(&h0, amount as u128, user.as_slice(), [0u8; 32], nonce)
    };
    let (ra, rb, rc) = (release(&mut w, e8(10)), release(&mut w, e8(10)), release(&mut w, e8(5)));
    let before = w.user_balance(user);
    w.mine_finalize();
    w.wait_released(&ra.transfer_id);
    w.wait_released(&rc.transfer_id);
    w.ticks(4);
    let b = w.get_release(&rb.transfer_id).expect("recorded");
    assert_eq!(b.status, ReleaseStatus::Pending, "over the daily release window: waits, nothing lost");
    assert!(b.release_block_index.is_none());
    assert_eq!(w.user_balance(user) - before, (e8(15) - 2 * LEDGER_FEE) as u128);
    assert_eq!(w.bridge_info().release_window_used_e8s, e8(15));
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [0, 0, e8(10) as u128, 0, 0, 0, 0], "{audit:?}");
    assert_eq!(audit.coverage_ok, Some(true));

    // A day later the window has rolled: the waiting release goes out, new deposits are admitted.
    w.advance_secs(86_400 + 60);
    w.wait_released(&rb.transfer_id);
    assert_eq!(w.user_balance(user) - before, (e8(25) - 3 * LEDGER_FEE) as u128);
    let args = w.deposit_args(e8(25), &h0);
    let again = w.bridge_to_base(user, args).expect("window rolled");
    assert_eq!(again.status, DepositStatus::Escrowed);
    assert_eq!(w.bridge_info().mint_window_used_e8s, e8(25), "window usage restarted from zero");
    assert!(w.bridge_info().halted.is_none());
}

// ---------------------------------------------------------------------------
// 14. Depositing from a subaccount; refunds go back to that exact account
// ---------------------------------------------------------------------------

#[test]
fn subaccount_deposit_pulls_from_and_refunds_to_the_same_subaccount() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, recipient) = (w.user, w.holder(0));
    let sub = [3u8; 32];
    let acc = with_sub(user, sub);
    w.ledger_transfer(user, acc, e8(50) as u128);
    // approve FROM the subaccount
    ledger_approve_from_sub(&w, user, sub, w.bridge, e8(20) as u128 + LEDGER_FEE as u128);
    let s1 = w.ledger_balance(&acc);
    let default_before = w.user_balance(user);

    w.contract_pause_mint_as_admin(); // TEST-ONLY admin simulation: force a preflight hold
    let mut args = w.deposit_args(e8(20), &recipient);
    args.from_subaccount = Some(serde_bytes::ByteBuf::from(sub.to_vec()));
    let dep = w.bridge_to_base(user, args).expect("deposit");
    assert_eq!(dep.status, DepositStatus::Escrowed, "{dep:?}");
    assert_eq!(dep.from, Some(acc));
    assert_eq!(s1 - w.ledger_balance(&acc), (e8(20) + LEDGER_FEE) as u128, "pulled from the subaccount");
    assert_eq!(w.user_balance(user), default_before, "the default account is untouched");
    w.wait_deposit(dep.id, "MintHeld", |d| matches!(d.status, DepositStatus::MintHeld { .. }));

    let refunded = w.request_refund(user, dep.id).expect("refund");
    assert_eq!(refunded.status, DepositStatus::Refunded);
    assert_eq!(s1 - w.ledger_balance(&acc), 2 * LEDGER_FEE as u128, "refund returned A - fee to the subaccount");
    assert_eq!(w.user_balance(user), default_before, "never to the default account");
    assert_eq!(w.escrow_balance(), 0);
    let audit = w.reconcile();
    assert_eq!(audit.buckets(), [0; 7], "{audit:?}");
}

fn ledger_approve_from_sub(w: &BridgeWorld, owner: Principal, sub: [u8; 32], spender: Principal, amount: u128) {
    use icrc_ledger_types::icrc2::approve::ApproveArgs;
    support::ledger::icrc2_approve(
        &w.pic,
        w.ledger,
        owner,
        ApproveArgs {
            from_subaccount: Some(sub),
            spender: account(spender),
            amount: candid::Nat::from(amount),
            expected_allowance: None,
            expires_at: None,
            fee: Some(candid::Nat::from(LEDGER_FEE)),
            memo: None,
            created_at_time: None,
        },
    )
    .expect("approve from subaccount");
}

// ---------------------------------------------------------------------------
// 15. Hints are bounded and never settle; active-operation limits
// ---------------------------------------------------------------------------

#[test]
fn hints_are_bounded_and_active_operation_limits_hold() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, user2, h0) = (w.user, w.user2, w.holder(0));

    // Hints: 5 per caller, idempotent, malformed rejected, anonymous refused; none settle anything.
    for i in 1..=5u8 {
        w.notify_hint(user, &hex0x(&[i; 32])).expect("hint");
    }
    assert_eq!(w.notify_hint(user, &hex0x(&[6u8; 32])), Err(BridgeError::TooManyActiveOperations));
    w.notify_hint(user, &hex0x(&[1u8; 32])).expect("a known hash is idempotent");
    w.notify_hint(user2, &hex0x(&[1u8; 32])).expect("another caller re-submitting a known hash is idempotent");
    w.notify_hint(user2, &hex0x(&[9u8; 32])).expect("other callers have their own budget");
    assert!(matches!(w.notify_hint(user, "0x1234"), Err(BridgeError::InvalidArgument(_))));
    assert!(matches!(w.notify_hint(user, "not hex"), Err(BridgeError::InvalidArgument(_))));
    assert_eq!(w.notify_hint(Principal::anonymous(), &hex0x(&[7u8; 32])), Err(BridgeError::Anonymous));
    let info = w.bridge_info();
    assert_eq!(info.observer.pending_hints.len(), 6);
    w.ticks(3);
    let info = w.bridge_info();
    assert_eq!((info.active_releases, info.observer.processed_count), (0, 0), "hints settle nothing");
    assert!(info.halted.is_none());
    // Hints expire after 2 hours; the next submission prunes them.
    w.advance_secs(2 * 3600 + 60);
    w.notify_hint(user, &hex0x(&[6u8; 32])).expect("old hints expired");
    assert_eq!(w.bridge_info().observer.pending_hints.len(), 1);

    // Active-operation limits: 2 per caller, 3 globally (deposits stay active while mints are paused).
    w.set_pauses(Pauses { mints: true, ..Pauses::default() }).expect("pause mints");
    w.set_limits(BridgeLimits { max_active_per_caller: 2, max_active_global: 3, ..default_limits() }).expect("limits");
    w.approve(user, w.bridge, e8(100) as u128);
    w.approve(user2, w.bridge, e8(100) as u128);
    for _ in 0..2 {
        let args = w.deposit_args(e8(1), &h0);
        assert_eq!(w.bridge_to_base(user, args).expect("within limit").status, DepositStatus::Escrowed);
    }
    let args = w.deposit_args(e8(1), &h0);
    assert_eq!(w.bridge_to_base(user, args), Err(BridgeError::TooManyActiveOperations), "per-caller limit");
    let args = w.deposit_args(e8(1), &h0);
    assert_eq!(w.bridge_to_base(user2, args).expect("other callers are unaffected").status, DepositStatus::Escrowed);
    let args = w.deposit_args(e8(1), &h0);
    assert_eq!(w.bridge_to_base(user2, args), Err(BridgeError::TooManyActiveOperations), "global limit (3)");
    assert_eq!(w.bridge_info().active_deposits, 3);
    assert_eq!(w.escrow_balance(), e8(3) as u128, "rejected requests pulled nothing");
}

// ---------------------------------------------------------------------------
// 16. C3 (exact match) must be reported on a quiet system, even when the observer idled
// ---------------------------------------------------------------------------

/// Design 7.2 / 7.8: C3 holds, and is reported, "when no mint attempt is outstanding and the
/// observer has consumed every bridge-out through finalized block F (processed_count ==
/// bridgeOutCount(F))". The code additionally requires `observer.cursor_block >= F`, but the
/// idle observer (cheap `bridgeOutCount()` probe) does not move the cursor until the next full
/// pass (hourly), so as soon as the finalized head moves on after the last burn,
/// `base_supply_exact_match` becomes `None` although every burn up to F is recorded.
#[test]
#[ignore = "BUG (reported to coordinator): check_base_supply requires observer.cursor_block >= finalized head, which an idle observer never advances, so C3 exact_match is None whenever the finalized head moved after the last full pass. Remove this ignore once the cursor condition is dropped."]
fn exact_match_is_reported_when_quiescent_even_if_the_observer_idled() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, h0) = (w.user, w.holder(0));
    w.deposit_and_deliver(user, e8(10), &h0);
    w.bridge_out_and_release(&h0, e8(4) as u128, user);
    let audit = w.reconcile();
    assert_eq!(audit.base_supply_exact_match, Some(true), "right after the last release: {audit:?}");

    // The chain moves on with no bridge activity (production: ~2 s blocks, finalized trails by minutes).
    w.mine(10);
    w.ticks(2);
    let audit = w.reconcile();
    let info = w.bridge_info();
    assert_eq!(audit.base_supply_within_bound, Some(true), "{audit:?}");
    assert_eq!(info.observer.processed_count, w.base_bridge_out_count(), "every burn is recorded");
    assert_eq!(
        audit.base_supply_exact_match,
        Some(true),
        "quiescent (processed_count == bridgeOutCount at the finalized head, nothing pending) but C3 is not reported: \
         cursor={} last_finalized={:?} supply_block={:?} {audit:?}",
        info.observer.cursor_block,
        info.observer.last_finalized_block,
        audit.base_supply_block
    );
}

// ---------------------------------------------------------------------------
// 17. Measurements (printed with --nocapture): cost of an idle tick
// ---------------------------------------------------------------------------

#[test]
fn measure_idle_tick_outcalls_and_cycles() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, h0) = (w.user, w.holder(0));
    w.deposit_and_deliver(user, e8(10), &h0);
    w.bridge_out_and_release(&h0, e8(4) as u128, user);
    w.mine(10);
    w.ticks(2);
    // Everything settled, nothing pending: provider requests and IC-side cycles per idle tick.
    let mut by_method = std::collections::BTreeMap::new();
    w.router.clear_log();
    let cycles_before = w.pic.cycle_balance(w.bridge);
    let n = 4u128;
    for _ in 0..n {
        w.tick();
    }
    for r in w.router.log() {
        for m in &r.rpc_methods {
            *by_method.entry(m.clone()).or_insert(0u32) += 1;
        }
    }
    let spent = cycles_before.saturating_sub(w.pic.cycle_balance(w.bridge));
    println!("MEASURE {n} idle ticks: provider requests by method {by_method:?}");
    println!("MEASURE IC-side bridge cycles per idle tick (excludes the EVM RPC canister's own fees): {}", spent / n);
    assert!(w.bridge_info().halted.is_none());
}
