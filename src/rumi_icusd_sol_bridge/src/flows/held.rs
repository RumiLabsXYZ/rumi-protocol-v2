//! Admin resolution of held operations (49b). "Resolved by hand" (note section 4) means a human
//! checked the chain and decided; this module makes that decision explicit, bounded and audited,
//! and never lets it manufacture evidence:
//!
//! * [`HeldOpResolution::Recheck`] puts the operation back under reconcile against the SAME stored
//!   bytes. Adoption still needs finalized success and the exact message.
//! * [`HeldOpResolution::Abandon`] is an attestation, available for a held MINT only: the operation
//!   becomes terminal, its nonce slot is re-read from the chain before reuse, and its deposit is
//!   never minted again (the owner may ask for the ICP refund). A reason is required and stored.
//!
//! Attesting that a held operation DID deliver is deliberately not offered here: delivery is
//! credited only by the strict verifier (49c), never by an admin's say-so.

use crate::admin;
use crate::env;
use crate::lane;
use crate::state::{self, AuditCtx, DepositState};
use crate::types::*;
use crate::views;

const MAX_REASON_BYTES: usize = 512;

pub fn resolve_held_op(op_id: u64, resolution: HeldOpResolution) -> BridgeResult<OpView> {
    let caller = admin::require_admin()?;
    let now = env::now_ns();
    match resolution {
        HeldOpResolution::Recheck => {
            lane::recheck_held(op_id, now).map_err(BridgeError::InvalidState)?;
            state::append_audit(None, AuditCtx::op(op_id, format!("recheck requested by {caller}")));
            crate::timer::schedule_soon();
        }
        HeldOpResolution::Abandon { reason } => {
            let reason = reason.trim().to_string();
            if reason.is_empty() || reason.len() > MAX_REASON_BYTES {
                return Err(BridgeError::InvalidArgument(format!("a reason of 1 to {MAX_REASON_BYTES} bytes is required")));
            }
            state::get_op(op_id).ok_or(BridgeError::NotFound)?;
            // Find the deposit first: an abandoned operation nobody owns would leave funds in
            // limbo, so refuse before changing anything.
            let deposit = state::deposits_in_state(|d| d.mint_op == Some(op_id), 1)
                .into_iter()
                .next()
                .ok_or_else(|| BridgeError::InvalidState(format!("no active deposit is waiting on operation {op_id}")))?;
            lane::abandon_held_mint(op_id, &reason, now).map_err(BridgeError::InvalidState)?;
            let mut d = state::get_deposit(deposit.id).ok_or(BridgeError::NotFound)?;
            d.mint_op = None;
            d.abandoned_mint_ops.push(op_id);
            d.state = DepositState::MintHeld { reason: format!("mint operation {op_id} abandoned by an admin: {reason}") };
            d.hold_retry_at_ns = None;
            d.updated_at_ns = now;
            state::put_deposit(&d);
            state::append_audit(
                None,
                AuditCtx::deposit(d.id, format!("mint operation {op_id} abandoned by {caller}; refund only: {reason}")),
            );
        }
    }
    state::get_op(op_id).map(|o| views::op_view(&o)).ok_or(BridgeError::NotFound)
}
