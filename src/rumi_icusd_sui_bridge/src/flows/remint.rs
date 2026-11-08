//! The held-release re-mint driver (53b).
//!
//! A return the ICP side cannot pay (its saved destination is the anonymous principal, this
//! canister or the ledger's minting account, or the amount does not cover the fees) opens `Held`
//! (53a): the coin is already burned on Sui, the backing is still in the escrow bucket `H`. This
//! driver gives the value back where it came from, to the Sui address that sent the return, as a
//! re-mint deposit that the ordinary mint lane then drives like any other mint:
//!
//! 1. in ONE synchronous step it applies `RemintStarted` (`H -> PM`), creates the re-mint deposit
//!    (kind `Remint { return_id }`, owned by the canister, no pull, no fee, `Escrowed`) with a mint id
//!    in its own domain (`ids::remint_id`) and moves the release to `Reminting { deposit_id }`;
//! 2. the lane mints it (the same persist-before-sign, same-attempt, receipt-or-record rules);
//! 3. delivery applies `RemintDelivered` (`PM -> O`) and moves the release to `Reminted`.
//!
//! The value is never paid to ICP and never silently dropped. A re-mint is NOT refundable to ICP
//! (the destination is the problem), so a re-mint whose operation fails stays `MintHeld` until an
//! admin retries it after the lane's own records prove every earlier attempt dead.

use crate::accounting::Transition;
use crate::env;
use crate::flows::deposit;
use crate::ids::{self, Hash32};
use crate::state::{self, AuditCtx, DepositKind, DepositRecord, DepositState, Queue, ReleaseState};

pub const MAX_REMINTS_PER_TICK: usize = 5;

/// Start re-mints for held releases, bounded per tick. Synchronous; makes no call.
pub fn process_remints() {
    let blocked = state::read_runtime(|c| c.halted.is_some() || c.mint_lane_halted.is_some() || c.config.pauses.mints);
    if blocked || deposit::deployment_not_ready().is_some() {
        return;
    }
    let held = state::next_releases(Queue::Remints, MAX_REMINTS_PER_TICK, |r| matches!(r.state, ReleaseState::Held { .. }));
    for r in held {
        let _ = start_remint(&r.return_id);
    }
}

/// Turn one held release into a re-mint deposit. All or nothing.
pub fn start_remint(return_id: &Hash32) -> Result<u64, String> {
    let mut r = state::get_release(return_id).ok_or("no such release")?;
    if !matches!(r.state, ReleaseState::Held { .. }) {
        return Err("the release is not held".into());
    }
    let (chain, binding, salt) = state::read_core(|c| (c.chain_identifier, c.binding.clone(), c.instance_salt));
    let b = binding.ok_or("no gate binding")?;
    if r.sui_sender == [0u8; 32] || r.sui_sender == b.gate {
        return Err("the return's sender cannot receive a mint".into());
    }
    if r.amount_e8s == 0 {
        return Err("the release amount is zero".into());
    }
    let mint_id = ids::remint_id(&chain, &b.package, &b.gate, &salt, return_id, &r.sui_sender, r.amount_e8s);
    state::apply_transition(Transition::RemintStarted { amount: r.amount_e8s }, AuditCtx::release(*return_id, "held release moved to a re-mint"))
        .map_err(|e| e.to_string())?;
    let now = env::now_ns();
    let id = state::mutate_runtime(|c| {
        let n = c.next_deposit_id;
        c.next_deposit_id += 1;
        n
    });
    let d = DepositRecord {
        id,
        kind: DepositKind::Remint { return_id: *return_id },
        owner: env::canister_id(),
        from: None,
        client_request_id: return_id.to_vec(),
        sui_recipient: r.sui_sender,
        amount_e8s: r.amount_e8s,
        deposit_fee_e8s: 0,
        mint_amount_e8s: r.amount_e8s,
        mint_id: Some(mint_id),
        state: DepositState::Escrowed,
        pull: None,
        refund: None,
        mint_op: None,
        abandoned_mint_ops: vec![],
        delivered: None,
        hold_retry_at_ns: None,
        created_at_ns: now,
        updated_at_ns: now,
    };
    state::put_deposit(&d);
    r.state = ReleaseState::Reminting { deposit_id: id };
    r.updated_at_ns = now;
    state::put_release(&r);
    state::append_audit(None, AuditCtx { note: format!("release re-minted by deposit {id}"), deposit_id: Some(id), return_id: Some(*return_id), op_id: None });
    Ok(id)
}
