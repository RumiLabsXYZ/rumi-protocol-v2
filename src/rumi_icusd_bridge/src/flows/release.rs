//! Base -> ICP: recording finalized bridge-outs (single settlement path), releases,
//! held claims and their re-mint to the Base sender, and user hints.

use super::ledger_ops::{self, OpRef};
use crate::accounting::Transition;
use crate::admin;
use crate::env;
use crate::evm::abi::{BridgeOutEvent, RawLog};
use crate::ids;
use crate::state::{
    self, AuditCtx, DepositKind, DepositRecord, DepositState, Hint, LedgerOp, LedgerOpKind, LedgerOpStatus,
    ReleaseRecord, ReleaseState,
};
use crate::types::*;
use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;

pub const MAX_RELEASES_PER_TICK: usize = 10;
pub const MAX_REMINTS_PER_TICK: usize = 10;
/// Hints older than this are dropped.
pub const HINT_TTL_NS: u64 = 2 * 3_600 * 1_000_000_000;
/// Pending hints kept per caller.
pub const MAX_HINTS_PER_CALLER: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordOutcome {
    Recorded,
    AlreadyRecorded,
    /// Not recorded yet, and not an error: the burn exceeds settled outstanding supply but
    /// in-flight mints explain it. The observer stops here and retries next pass without
    /// counting a failure (so no backoff delays the release).
    Deferred(String),
}

/// Parse the destination named in a bridge-out. `Err` gives the hold reason.
pub fn parse_destination(raw_principal: &[u8], subaccount: &[u8; 32], bridge: &Principal) -> Result<Account, String> {
    let owner = Principal::try_from_slice(raw_principal).map_err(|e| format!("invalid ICP principal bytes: {e}"))?;
    if owner == Principal::anonymous() {
        return Err("destination is the anonymous principal".into());
    }
    if owner == Principal::management_canister() {
        return Err("destination is the management canister".into());
    }
    if owner == *bridge {
        return Err("destination is the bridge canister itself".into());
    }
    let sub = if *subaccount == [0u8; 32] { None } else { Some(*subaccount) };
    Ok(Account { owner, subaccount: sub })
}

fn same_account(a: &Account, b: &Account) -> bool {
    a.owner == b.owner && a.subaccount.unwrap_or([0u8; 32]) == b.subaccount.unwrap_or([0u8; 32])
}

/// Record one bridge-out that is at or below the finalized head. This is the ONLY
/// function that creates release records. Idempotent on the transfer id.
pub fn record_finalized_bridge_out(ev: &BridgeOutEvent, log: &RawLog) -> Result<RecordOutcome, String> {
    if let Some(existing) = state::get_release(&ev.transfer_id) {
        if existing.sequence != ev.sequence || existing.amount_e8s != ev.amount_e8s || existing.base_tx_hash != log.tx_hash {
            state::halt(format!("transfer id {} observed twice with different contents", ids::hex0x(&ev.transfer_id)));
            return Err("conflicting bridge-out for a known transfer id".into());
        }
        return Ok(RecordOutcome::AlreadyRecorded);
    }
    if let Some(other) = state::release_by_sequence(ev.sequence) {
        if other != ev.transfer_id {
            state::halt(format!("bridge-out sequence {} maps to two transfer ids", ev.sequence));
            return Err("conflicting bridge-out sequence".into());
        }
    }
    let canister = env::canister_id();
    let now = env::now_ns();
    let (release_fee, ledger_fee, minting) =
        state::read_core(|c| (c.config.fees.release_fee_e8s, c.ledger_fee_e8s, c.ledger_minting_account));
    let Some(ledger_fee) = ledger_fee else {
        return Err("ledger fee unknown; refusing to classify the release yet".into());
    };
    let Some(minting) = minting else {
        return Err("ledger minting account unknown; refusing to classify the release yet".into());
    };
    let destination = parse_destination(&ev.icp_principal, &ev.icp_subaccount, &canister).and_then(|a| {
        if same_account(&a, &minting) {
            Err("destination is the icUSD minting account (a transfer there would burn the funds)".into())
        } else {
            Ok(a)
        }
    });
    let releasable = match &destination {
        Ok(_) if (ev.amount_e8s as u128) > release_fee as u128 + ledger_fee as u128 => Ok(()),
        Ok(_) => Err(format!(
            "amount {} e8s does not cover the bridge fee ({release_fee}) and ledger fee ({ledger_fee})",
            ev.amount_e8s
        )),
        Err(e) => Err(e.clone()),
    };
    // A burn can be finalized in the same head step as the mint that funded it, before
    // the lane has settled that mint. If in-flight mints explain the shortfall, wait and
    // retry (the observer does not advance); only a burn no mint can explain halts.
    let outstanding = state::read_core(|c| c.buckets.outstanding_base);
    if (ev.amount_e8s as u128) > outstanding {
        let inflight = state::inflight_mint_e8s();
        if (ev.amount_e8s as u128) <= outstanding.saturating_add(inflight) {
            return Ok(RecordOutcome::Deferred(format!(
                "bridge-out {} of {} e8s waits for in-flight mint settlement (settled outstanding {outstanding}, in flight {inflight})",
                ids::hex0x(&ev.transfer_id),
                ev.amount_e8s
            )));
        }
    }
    let (t, st) = match &releasable {
        Ok(()) => (Transition::BridgeOutAccepted { amount: ev.amount_e8s }, ReleaseState::Pending),
        Err(reason) => (Transition::BridgeOutHeld { amount: ev.amount_e8s }, ReleaseState::Held { reason: reason.clone() }),
    };
    if let Err(e) = state::apply_transition(t, AuditCtx::release(ev.transfer_id, format!("bridge-out seq {} finalized", ev.sequence))) {
        // O < B means Base supply exceeds what this canister minted.
        state::halt(format!("bridge-out {} exceeds outstanding Base supply: {e}", ids::hex0x(&ev.transfer_id)));
        return Err(e.to_string());
    }
    let r = ReleaseRecord {
        transfer_id: ev.transfer_id,
        sequence: ev.sequence,
        base_sender: ev.from,
        amount_e8s: ev.amount_e8s,
        raw_icp_principal: ev.icp_principal.clone(),
        icp_subaccount: ev.icp_subaccount,
        destination: destination.ok(),
        base_tx_hash: log.tx_hash,
        base_block_number: log.block_number,
        base_block_hash: log.block_hash,
        log_index: log.log_index,
        release_fee_e8s: release_fee,
        state: st,
        transfer: None,
        created_at_ns: now,
        updated_at_ns: now,
    };
    state::put_release(&r);
    state::index_release(&r);
    // The hint for this tx, if any, has served its purpose.
    state::mutate_core(|c| c.hints.retain(|h| h.tx_hash != log.tx_hash));
    Ok(RecordOutcome::Recorded)
}

/// User hint: "check this Base tx sooner". Never settles anything.
pub fn record_hint(tx_hash: String) -> BridgeResult<()> {
    let caller = admin::require_authenticated()?;
    let h = ids::parse_bytes32(&tx_hash).map_err(BridgeError::InvalidArgument)?;
    let now = env::now_ns();
    state::mutate_core(|c| {
        c.hints.retain(|x| now.saturating_sub(x.submitted_at_ns) < HINT_TTL_NS);
        if c.hints.iter().any(|x| x.tx_hash == h) {
            return Ok(());
        }
        if c.hints.iter().filter(|x| x.submitted_by == caller).count() >= MAX_HINTS_PER_CALLER {
            return Err(BridgeError::TooManyActiveOperations);
        }
        if c.hints.len() >= state::MAX_HINTS {
            c.hints.remove(0);
        }
        c.hints.push(Hint { tx_hash: h, submitted_by: caller, submitted_at_ns: now });
        Ok(())
    })
}

/// Create (if needed) and drive release transfers.
pub async fn process_releases() {
    let paused = state::read_core(|c| c.config.pauses.releases || c.halted.is_some());
    if paused {
        return;
    }
    let pending: Vec<[u8; 32]> = state::releases_in_state(|r| matches!(r.state, ReleaseState::Pending), MAX_RELEASES_PER_TICK)
        .into_iter()
        .map(|r| r.transfer_id)
        .collect();
    for tid in pending {
        // Re-read: an earlier iteration's await may have let another tick change it.
        let Some(r) = state::get_release(&tid) else { continue };
        if !matches!(r.state, ReleaseState::Pending) {
            continue;
        }
        if r.transfer.is_none() && !prepare_release_op(&tid) {
            continue;
        }
        let op_ref = OpRef::Release(tid);
        let Some(op) = ledger_ops::load_op(&op_ref) else { continue };
        if matches!(op.status, LedgerOpStatus::Pending) && op.attempts == 0 {
            if let Some(sent) = ledger_ops::attempt(&op_ref).await {
                ledger_ops::apply_outcome(&op_ref, sent);
            }
        } else {
            ledger_ops::reconcile(&op_ref).await;
        }
    }
}

/// Build the transfer tuple for a pending release, applying the release window.
/// Returns false if the release must wait.
fn prepare_release_op(tid: &[u8; 32]) -> bool {
    let Some(mut r) = state::get_release(tid) else { return false };
    if !matches!(r.state, ReleaseState::Pending) {
        return false;
    }
    // Never replace a tuple that may already have been sent: a second distinct tuple
    // with the same memo would be a second, independent transfer.
    if r.transfer.is_some() {
        return true;
    }
    let Some(dest) = r.destination else { return false };
    let now = env::now_ns();
    let canister = env::canister_id();
    let (fee, escrow, tip) = state::read_core(|c| (c.ledger_fee_e8s, c.escrow_account(canister), c.ledger_tip_lower_bound));
    let Some(fee) = fee else { return false };
    let outflow = r.amount_e8s - r.release_fee_e8s;
    if outflow <= fee {
        // Fee rose after recording: hold for re-mint instead of releasing dust.
        if state::apply_transition(Transition::ReleaseHeld { amount: r.amount_e8s }, AuditCtx::release(*tid, "fees exceed release")).is_ok() {
            r.state = ReleaseState::Held { reason: "fees exceed the released amount".into() };
            r.updated_at_ns = now;
            state::put_release(&r);
        }
        return false;
    }
    // Window: a release that alone exceeds the limit may go out at the start of a fresh
    // window, so no valid release waits forever.
    let window_ok = state::mutate_core(|c| {
        c.roll_window(now);
        let used = c.windows.released_in_window;
        let ok = used == 0
            || used.saturating_add(r.amount_e8s as u128) <= c.config.limits.release_window_limit_e8s as u128;
        if ok {
            c.windows.released_in_window = used.saturating_add(r.amount_e8s as u128);
        }
        ok
    });
    if !window_ok {
        return false;
    }
    r.transfer = Some(LedgerOp {
        kind: LedgerOpKind::Release,
        from: escrow,
        to: dest,
        spender: None,
        amount_e8s: outflow - fee,
        fee_e8s: Some(fee),
        memo: ids::release_memo(tid),
        created_at_time: now,
        attempts: 0,
        ambiguous_seen: false,
        status: LedgerOpStatus::Pending,
        search_from_block: tip,
        search_next_block: None,
        last_attempt_ns: None,
        outflow_reserved: false,
    });
    r.updated_at_ns = now;
    state::put_release(&r);
    true
}

/// Move held claims into re-mint deposits addressed to their Base sender. The sender
/// is fixed by the finalized event, so no claimant can redirect the funds.
pub fn start_remints() {
    let (ok, chain, contract, salt) = state::read_core(|c| {
        (c.halted.is_none() && c.config.contract.is_some(), c.config.chain_id, c.config.contract.as_ref().map(|b| b.address), c.instance_salt)
    });
    let (true, Some(contract)) = (ok, contract) else { return };
    let canister = env::canister_id();
    for mut r in state::releases_in_state(|r| matches!(r.state, ReleaseState::Held { .. }), MAX_REMINTS_PER_TICK) {
        let now = env::now_ns();
        let id = state::mutate_core(|c| {
            let id = c.next_deposit_id;
            c.next_deposit_id += 1;
            id
        });
        if let Err(e) = state::apply_transition(Transition::RemintStarted { amount: r.amount_e8s }, AuditCtx::release(r.transfer_id, format!("re-mint as deposit {id}"))) {
            state::halt(format!("re-mint accounting: {e}"));
            return;
        }
        let d = DepositRecord {
            id,
            kind: DepositKind::Remint { transfer_id: r.transfer_id },
            owner: canister,
            from: None,
            client_request_id: r.transfer_id.to_vec(),
            base_recipient: r.base_sender,
            amount_e8s: r.amount_e8s,
            deposit_fee_e8s: 0,
            mint_amount_e8s: r.amount_e8s,
            mint_id: ids::remint_id(chain, &contract, &salt, &r.transfer_id),
            state: DepositState::Escrowed,
            pull: None,
            refund: None,
            tx_ids: vec![],
            active_tx: None,
            delivered: None,
            hold_retry_at_ns: None,
            created_at_ns: now,
            updated_at_ns: now,
        };
        state::put_deposit(&d);
        r.state = ReleaseState::Reminting { deposit_id: id };
        r.updated_at_ns = now;
        state::put_release(&r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::CallOutcome;

    const E8: u64 = 100_000_000;

    fn setup() {
        crate::flows::deposit::tests::setup_bound();
        // Seed outstanding Base supply as if 1000 icUSD had been delivered.
        state::mutate_core(|c| {
            c.buckets.outstanding_base = (1_000 * E8) as u128;
            c.ledger_minting_account = Some(Account { owner: minter(), subaccount: None });
        });
    }

    fn user() -> Principal {
        Principal::from_slice(&[0x11; 29])
    }

    fn minter() -> Principal {
        Principal::from_slice(&[0x66; 10])
    }

    fn ev(seq: u64, amount: u64, principal: Vec<u8>) -> (BridgeOutEvent, RawLog) {
        let mut tid = [0u8; 32];
        tid[..8].copy_from_slice(&seq.to_be_bytes());
        tid[31] = 0xaa;
        let e = BridgeOutEvent { transfer_id: tid, from: [0x55; 20], amount_e8s: amount, sequence: seq, icp_principal: principal, icp_subaccount: [0; 32] };
        let mut txh = [0u8; 32];
        txh[0] = seq as u8;
        let l = RawLog { address: [0xcc; 20], topics: vec![], data: vec![], block_number: 100 + seq, block_hash: [7; 32], tx_hash: txh, log_index: 3, removed: false };
        (e, l)
    }

    #[test]
    fn valid_bridge_out_moves_outstanding_to_pending_release_once() {
        setup();
        let (e, l) = ev(1, 100 * E8, user().as_slice().to_vec());
        assert_eq!(record_finalized_bridge_out(&e, &l).unwrap(), RecordOutcome::Recorded);
        let b = state::read_core(|c| c.buckets);
        assert_eq!(b.outstanding_base, (900 * E8) as u128);
        assert_eq!(b.pending_release, (100 * E8) as u128);
        // Replay through any path is a no-op.
        assert_eq!(record_finalized_bridge_out(&e, &l).unwrap(), RecordOutcome::AlreadyRecorded);
        assert_eq!(state::read_core(|c| c.buckets), b);
        let r = state::get_release(&e.transfer_id).unwrap();
        assert_eq!(r.destination, Some(Account { owner: user(), subaccount: None }));
        assert_eq!(state::list_owner_releases(&user(), None, 10).len(), 1);
        assert_eq!(state::list_sender_releases(&[0x55; 20], None, 10).len(), 1);
        assert!(state::list_sender_releases(&[0x56; 20], None, 10).is_empty());
    }

    #[test]
    fn conflicting_replay_halts() {
        setup();
        let (e, l) = ev(1, 100 * E8, user().as_slice().to_vec());
        record_finalized_bridge_out(&e, &l).unwrap();
        let mut e2 = e.clone();
        e2.amount_e8s += 1;
        assert!(record_finalized_bridge_out(&e2, &l).is_err());
        assert!(state::is_halted());
    }

    #[test]
    fn invalid_destinations_are_held_for_remint_to_sender() {
        setup();
        // Releases are not classified until the minting account is known.
        state::mutate_core(|c| c.ledger_minting_account = None);
        let (e0, l0) = ev(99, E8, user().as_slice().to_vec());
        assert!(record_finalized_bridge_out(&e0, &l0).is_err());
        state::mutate_core(|c| c.ledger_minting_account = Some(Account { owner: minter(), subaccount: None }));
        for (seq, p) in [
            (1u64, vec![0x04]),
            (2, vec![]),
            (3, env::canister_id().as_slice().to_vec()),
            (4, vec![0xff; 30]),
            (5, minter().as_slice().to_vec()),
        ] {
            let (e, l) = ev(seq, 10 * E8, p);
            record_finalized_bridge_out(&e, &l).unwrap();
            let r = state::get_release(&e.transfer_id).unwrap();
            assert!(matches!(r.state, ReleaseState::Held { .. }), "seq {seq}: {:?}", r.state);
        }
        assert_eq!(state::read_core(|c| c.buckets.held), (50 * E8) as u128);
        start_remints();
        let b = state::read_core(|c| c.buckets);
        assert_eq!(b.held, 0);
        assert_eq!(b.pending_mint, (50 * E8) as u128);
        let remints = state::deposits_in_state(|d| matches!(d.kind, DepositKind::Remint { .. }), 10);
        assert_eq!(remints.len(), 5);
        assert!(remints.iter().all(|d| d.base_recipient == [0x55; 20] && d.state == DepositState::Escrowed));
        // Re-mint ids differ from deposit mint ids for the same numeric id.
        for d in &remints {
            assert_ne!(d.mint_id, ids::mint_id(state::CHAIN_BASE_SEPOLIA, &[0xcc; 20], &[9; 32], d.id));
        }
    }

    #[test]
    fn dust_bridge_out_is_held() {
        setup();
        let (e, l) = ev(1, 10_000, user().as_slice().to_vec());
        record_finalized_bridge_out(&e, &l).unwrap();
        assert!(matches!(state::get_release(&e.transfer_id).unwrap().state, ReleaseState::Held { .. }));
    }

    #[test]
    fn burn_explained_by_an_in_flight_mint_waits_instead_of_halting() {
        setup();
        state::mutate_core(|c| c.buckets.outstanding_base = 0);
        // A user deposit of 100 icUSD whose mint was signed (and possibly included) but not
        // yet settled by the lane.
        let mut d = state::tests::sample_deposit(50, user());
        d.amount_e8s = 100 * E8;
        d.mint_amount_e8s = 100 * E8;
        d.state = DepositState::MintInFlight;
        d.active_tx = Some(9);
        state::put_deposit(&d);
        state::put_tx(&state::EvmTxRecord {
            id: 9,
            lane: crate::ids::LaneKind::Minter,
            nonce: 0,
            purpose: state::TxPurpose::Mint { deposit_id: 50 },
            to: [0xcc; 20],
            value_wei: 0,
            data: vec![],
            gas_limit: 150_000,
            attempts: vec![state::TxAttempt {
                kind: state::AttemptKind::Original,
                to: [0xcc; 20],
                value_wei: 0,
                data: vec![],
                max_fee_per_gas: 1,
                max_priority_fee_per_gas: 1,
                gas_limit: 150_000,
                raw: vec![2],
                tx_hash: [3; 32],
                created_at_ns: 0,
                broadcast_count: 1,
                last_broadcast_ns: Some(0),
                last_broadcast_note: None,
            }],
            status: state::TxStatus::Included { attempt: 0, block_number: 10, block_hash: [4; 32], success: true },
            cancel_requested: false,
            outcome_applied: false,
            created_at_ns: 0,
            updated_at_ns: 0,
            last_checked_ns: None,
            note: None,
        });
        let (e, l) = ev(1, 100 * E8, user().as_slice().to_vec());
        assert!(matches!(record_finalized_bridge_out(&e, &l), Ok(RecordOutcome::Deferred(_))));
        assert!(!state::is_halted(), "an in-flight mint explains the burn");
        assert!(state::get_release(&e.transfer_id).is_none());
        // More than the in-flight mint can explain still halts.
        let (e2, l2) = ev(2, 101 * E8, user().as_slice().to_vec());
        assert!(record_finalized_bridge_out(&e2, &l2).is_err());
        assert!(state::is_halted());
    }

    #[test]
    fn preparing_twice_never_replaces_a_tuple_that_may_have_been_sent() {
        setup();
        let (e, l) = ev(1, 10 * E8, user().as_slice().to_vec());
        record_finalized_bridge_out(&e, &l).unwrap();
        assert!(prepare_release_op(&e.transfer_id));
        let first = state::get_release(&e.transfer_id).unwrap().transfer.unwrap();
        // Simulate the first send (as attempt() would persist it).
        let op_ref = OpRef::Release(e.transfer_id);
        let mut sent = first.clone();
        sent.attempts = 1;
        sent.outflow_reserved = true;
        ledger_ops::store_op(&op_ref, sent.clone());
        // A second, overlapping tick prepares the same release later.
        crate::env::test_env::advance_secs(7);
        assert!(prepare_release_op(&e.transfer_id));
        let after = state::get_release(&e.transfer_id).unwrap().transfer.unwrap();
        assert_eq!(after, sent, "the persisted tuple must be untouched");
        assert_eq!(state::read_core(|c| c.windows.released_in_window), (10 * E8) as u128, "window charged once");
    }

    #[test]
    fn a_release_larger_than_the_window_goes_out_alone_in_a_fresh_window() {
        setup();
        state::mutate_core(|c| c.config.limits.release_window_limit_e8s = 50 * E8);
        let (e, l) = ev(1, 100 * E8, user().as_slice().to_vec());
        record_finalized_bridge_out(&e, &l).unwrap();
        assert!(prepare_release_op(&e.transfer_id), "fresh window admits one oversized release");
        let (e2, l2) = ev(2, 10 * E8, user().as_slice().to_vec());
        record_finalized_bridge_out(&e2, &l2).unwrap();
        assert!(!prepare_release_op(&e2.transfer_id), "window now used");
        crate::env::test_env::advance_secs(86_401);
        assert!(prepare_release_op(&e2.transfer_id));
    }

    #[test]
    fn dust_does_not_consume_the_release_window() {
        setup();
        state::mutate_core(|c| {
            c.config.limits.release_window_limit_e8s = 50 * E8;
            c.config.fees.release_fee_e8s = 0;
        });
        let (e, l) = ev(1, 100 * E8, user().as_slice().to_vec());
        record_finalized_bridge_out(&e, &l).unwrap();
        // Ledger fee rises above the release before the transfer is prepared.
        state::mutate_core(|c| c.ledger_fee_e8s = Some(200 * E8));
        assert!(!prepare_release_op(&e.transfer_id));
        assert!(matches!(state::get_release(&e.transfer_id).unwrap().state, ReleaseState::Held { .. }));
        assert_eq!(state::read_core(|c| c.windows.released_in_window), 0);
    }

    #[test]
    fn bridge_out_larger_than_outstanding_halts_and_records_nothing() {
        setup();
        let (e, l) = ev(1, 1_001 * E8, user().as_slice().to_vec());
        assert!(record_finalized_bridge_out(&e, &l).is_err());
        assert!(state::is_halted());
        assert!(state::get_release(&e.transfer_id).is_none());
    }

    #[test]
    fn release_tuple_and_completion_accounting() {
        setup();
        state::mutate_core(|c| c.config.fees.release_fee_e8s = E8 / 100);
        let (e, l) = ev(1, 100 * E8, user().as_slice().to_vec());
        record_finalized_bridge_out(&e, &l).unwrap();
        assert!(prepare_release_op(&e.transfer_id));
        let r = state::get_release(&e.transfer_id).unwrap();
        let op = r.transfer.clone().unwrap();
        assert_eq!(op.fee_e8s, Some(10_000));
        assert_eq!(op.amount_e8s, 100 * E8 - E8 / 100 - 10_000);
        assert_eq!(op.escrow_outflow(), 100 * E8 - E8 / 100);
        assert_eq!(op.memo, e.transfer_id.to_vec());
        assert_eq!(op.from.subaccount, Some(ids::escrow_subaccount(state::CHAIN_BASE_SEPOLIA)));

        // Reserve outflow (as attempt() would) then prove success.
        let op_ref = OpRef::Release(e.transfer_id);
        state::apply_transition(Transition::OutflowUnknown { amount: op.escrow_outflow() }, AuditCtx::note("test")).unwrap();
        let mut sent = op.clone();
        sent.outflow_reserved = true;
        sent.attempts = 1;
        ledger_ops::store_op(&op_ref, sent.clone());
        ledger_ops::apply_resolution(&op_ref, ledger_ops::resolution_for(&sent, &CallOutcome::Accepted { block_index: 44 }));
        let r = state::get_release(&e.transfer_id).unwrap();
        assert_eq!(r.state, ReleaseState::Released);
        let b = state::read_core(|c| c.buckets);
        assert_eq!(b.pending_release, 0);
        assert_eq!(b.unknown_outflow, 0);
        assert_eq!(b.fees, (E8 / 100) as u128);
    }

    #[test]
    fn release_window_defers_without_loss() {
        setup();
        state::mutate_core(|c| c.config.limits.release_window_limit_e8s = 150 * E8);
        let (e1, l1) = ev(1, 100 * E8, user().as_slice().to_vec());
        let (e2, l2) = ev(2, 100 * E8, user().as_slice().to_vec());
        record_finalized_bridge_out(&e1, &l1).unwrap();
        record_finalized_bridge_out(&e2, &l2).unwrap();
        assert!(prepare_release_op(&e1.transfer_id));
        assert!(!prepare_release_op(&e2.transfer_id));
        assert_eq!(state::get_release(&e2.transfer_id).unwrap().state, ReleaseState::Pending);
        assert_eq!(state::read_core(|c| c.buckets.pending_release), (200 * E8) as u128);
    }

    #[test]
    fn hints_are_bounded_deduped_and_authenticated() {
        setup();
        crate::env::test_env::set_caller(Principal::anonymous());
        assert_eq!(record_hint(format!("0x{}", hex::encode([1u8; 32]))), Err(BridgeError::Anonymous));
        crate::env::test_env::set_caller(user());
        assert!(record_hint("0x1234".into()).is_err());
        for i in 0..MAX_HINTS_PER_CALLER {
            let mut h = [0u8; 32];
            h[..8].copy_from_slice(&(i as u64).to_be_bytes());
            record_hint(ids::hex0x(&h)).unwrap();
        }
        // Re-submitting a known hint is a no-op, a new one over the per-caller cap is refused.
        record_hint(ids::hex0x(&[0u8; 32])).unwrap();
        assert_eq!(record_hint(ids::hex0x(&[0xffu8; 32])), Err(BridgeError::TooManyActiveOperations));
        // The global bound still holds across many callers.
        for i in 0..(state::MAX_HINTS + 10) {
            crate::env::test_env::set_caller(Principal::from_slice(&(i as u32).to_be_bytes()));
            let mut h = [0x77u8; 32];
            h[..8].copy_from_slice(&(i as u64).to_be_bytes());
            record_hint(ids::hex0x(&h)).unwrap();
        }
        assert_eq!(state::read_core(|c| c.hints.len()), state::MAX_HINTS);
        assert!(!crate::timer::has_pending_work(), "hints alone never force fast polling");
    }
}
