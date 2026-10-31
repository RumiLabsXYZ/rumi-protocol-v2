//! Authorization guards, the controller-only admin list, and (49b) the admin setters for pauses,
//! limits and fees, ported from the Base instance's `admin.rs`.
//!
//! Admins can pause and tune limits and fees within hard bounds. They cannot move escrow (only the
//! release, refund and fee-sweep paths can) and cannot change the ledger, cluster or key.
//! Controllers manage the admin list.

use crate::env;
use crate::state::{self, AuditCtx};
use crate::types::*;
use candid::Principal;

pub fn require_authenticated() -> BridgeResult<Principal> {
    let caller = env::caller();
    if caller == Principal::anonymous() {
        return Err(BridgeError::Anonymous);
    }
    Ok(caller)
}

pub fn require_admin() -> BridgeResult<Principal> {
    let caller = require_authenticated()?;
    if state::read_core(|c| c.is_admin(&caller)) {
        Ok(caller)
    } else {
        Err(BridgeError::Unauthorized)
    }
}

pub fn require_controller() -> BridgeResult<Principal> {
    let caller = require_authenticated()?;
    if env::is_controller(&caller) {
        Ok(caller)
    } else {
        Err(BridgeError::Unauthorized)
    }
}

/// Pausing is always allowed. Unpausing deposits needs a bound mint and no halt; unpausing anything
/// needs no halt.
pub fn set_pauses(p: Pauses) -> BridgeResult<()> {
    let caller = require_admin()?;
    let (profile, halted, current) = state::read_core(|c| (c.tx_profile(), c.halted.clone(), c.config.pauses.clone()));
    if !p.deposits && profile.is_err() {
        return Err(BridgeError::NotConfigured("the Solana mint must be bound before deposits are enabled".into()));
    }
    if let Some(h) = halted {
        let unpausing = (current.deposits && !p.deposits)
            || (current.mints && !p.mints)
            || (current.releases && !p.releases)
            || (current.observer && !p.observer);
        if unpausing {
            return Err(BridgeError::Halted(h.reason));
        }
    }
    state::mutate_core(|c| c.config.pauses = p.clone());
    state::append_audit(None, AuditCtx::note(format!("pauses set by {caller}: {p:?}")));
    Ok(())
}

pub fn set_limits(l: BridgeLimits) -> BridgeResult<()> {
    let caller = require_admin()?;
    state::validate_limits(&l).map_err(BridgeError::InvalidArgument)?;
    let fees = state::read_core(|c| c.config.fees.clone());
    state::validate_fees(&fees, &l).map_err(BridgeError::InvalidArgument)?;
    state::mutate_core(|c| c.config.limits = l.clone());
    state::append_audit(None, AuditCtx::note(format!("limits set by {caller}: {l:?}")));
    Ok(())
}

pub fn set_fees(f: BridgeFees) -> BridgeResult<()> {
    let caller = require_admin()?;
    let limits = state::read_core(|c| c.config.limits.clone());
    state::validate_fees(&f, &limits).map_err(BridgeError::InvalidArgument)?;
    state::mutate_core(|c| c.config.fees = f.clone());
    state::append_audit(None, AuditCtx::note(format!("fees set by {caller}: {f:?}")));
    Ok(())
}

/// Clear a lane halt after a human resolved its cause. The reason is required and stored: the lane
/// halts only for conditions the engine cannot judge (a foreign or unverifiable consumer of a
/// nonce, a history horizon, an owner change), so clearing one is a decision, not a retry.
pub fn clear_lane_halt(lane: crate::records::Lane, note: &str) -> BridgeResult<()> {
    let caller = require_admin()?;
    let note = note.trim();
    if note.is_empty() || note.len() > 512 {
        return Err(BridgeError::InvalidArgument("a note of 1 to 512 bytes saying how the halt was resolved is required".into()));
    }
    if !state::clear_lane_halt(lane) {
        return Err(BridgeError::InvalidState("the lane is not halted".into()));
    }
    state::append_audit(None, AuditCtx::note(format!("lane {lane:?} halt cleared by {caller}: {note}")));
    Ok(())
}

pub fn set_admins(admins: Vec<Principal>) -> BridgeResult<()> {
    let caller = require_controller()?;
    if admins.is_empty() || admins.iter().any(|p| *p == Principal::anonymous()) {
        return Err(BridgeError::InvalidArgument("admins must be non-empty and non-anonymous".into()));
    }
    state::mutate_core(|c| c.config.admins = admins.clone());
    state::append_audit(None, AuditCtx::note(format!("admins set by controller {caller}: {admins:?}")));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::test_env;

    fn admin() -> Principal {
        Principal::from_slice(&[3; 10])
    }

    fn controller() -> Principal {
        Principal::from_slice(&[9; 10])
    }

    fn setup() {
        state::reset_for_tests();
        state::mutate_core(|c| {
            c.initialized = true;
            c.config.admins = vec![admin()];
        });
        test_env::set_controllers(vec![controller()]);
    }

    #[test]
    fn anonymous_is_rejected_everywhere() {
        setup();
        test_env::set_caller(Principal::anonymous());
        assert_eq!(require_authenticated(), Err(BridgeError::Anonymous));
        assert_eq!(require_admin(), Err(BridgeError::Anonymous));
        assert_eq!(require_controller(), Err(BridgeError::Anonymous));
        assert_eq!(set_admins(vec![admin()]), Err(BridgeError::Anonymous));
    }

    #[test]
    fn admin_and_controller_are_separate_roles() {
        setup();
        test_env::set_caller(admin());
        assert_eq!(require_admin(), Ok(admin()));
        assert_eq!(require_controller(), Err(BridgeError::Unauthorized), "an admin is not a controller");
        assert_eq!(set_admins(vec![admin()]), Err(BridgeError::Unauthorized));
        test_env::set_caller(controller());
        assert_eq!(require_controller(), Ok(controller()));
        assert_eq!(require_admin(), Err(BridgeError::Unauthorized), "a controller is not automatically an admin");
        test_env::set_caller(Principal::from_slice(&[4; 10]));
        assert_eq!(require_admin(), Err(BridgeError::Unauthorized));
    }

    #[test]
    fn only_a_controller_replaces_the_admin_list_and_it_must_be_valid() {
        setup();
        test_env::set_caller(controller());
        let new_admin = Principal::from_slice(&[5; 10]);
        assert!(set_admins(vec![new_admin]).is_ok());
        assert_eq!(state::read_core(|c| c.config.admins.clone()), vec![new_admin]);
        assert!(matches!(set_admins(vec![]), Err(BridgeError::InvalidArgument(_))));
        assert!(matches!(set_admins(vec![new_admin, Principal::anonymous()]), Err(BridgeError::InvalidArgument(_))));
        assert_eq!(state::audit_len(), 1, "only the successful change is logged");
    }
}
