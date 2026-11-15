//! Authorization guards, the controller-only admin list, and the admin setters for pauses, limits and
//! fees. Ported from the frozen Solana instance (`admin.rs` at 4394b472).
//!
//! Admins can pause and tune limits and fees within hard bounds. They cannot move escrow (only the
//! release, refund and fee-sweep paths can), cannot change the ledger, the network or the role keys,
//! and cannot sign anything. Controllers manage the admin list and the gate binding.

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
    if state::read_runtime(|c| c.is_admin(&caller)) {
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

/// Pausing is always allowed. Unpausing deposits needs the gate binding and role keys; unpausing
/// anything needs no halt.
pub fn set_pauses(p: Pauses) -> BridgeResult<()> {
    let caller = require_admin()?;
    let (halted, current) = state::read_runtime(|c| (c.halted.clone(), c.config.pauses.clone()));
    if !p.deposits {
        if let Some(why) = crate::flows::deposit::deployment_not_ready() {
            return Err(BridgeError::NotConfigured(format!("deposits cannot be enabled: {why}")));
        }
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
    state::mutate_runtime(|c| c.config.pauses = p.clone());
    state::append_audit(None, AuditCtx::note(format!("pauses set by {caller}: {p:?}")));
    Ok(())
}

pub fn set_limits(l: BridgeLimits) -> BridgeResult<()> {
    let caller = require_admin()?;
    state::validate_limits(&l).map_err(BridgeError::InvalidArgument)?;
    let fees = state::read_runtime(|c| c.config.fees.clone());
    state::validate_fees(&fees, &l).map_err(BridgeError::InvalidArgument)?;
    state::mutate_runtime(|c| c.config.limits = l.clone());
    state::append_audit(None, AuditCtx::note(format!("limits set by {caller}: {l:?}")));
    Ok(())
}

pub fn set_fees(f: BridgeFees) -> BridgeResult<()> {
    let caller = require_admin()?;
    let limits = state::read_runtime(|c| c.config.limits.clone());
    state::validate_fees(&f, &limits).map_err(BridgeError::InvalidArgument)?;
    state::mutate_runtime(|c| c.config.fees = f.clone());
    state::append_audit(None, AuditCtx::note(format!("fees set by {caller}: {f:?}")));
    Ok(())
}

/// Clear the bridge halt after a human resolved its cause. The note is required and stored: the
/// bridge halts only for conditions the engine cannot judge (a ledger success the books refuse, an
/// unfunded release, conflicting records), so clearing one is a decision, not a retry.
pub fn clear_halt(note: &str) -> BridgeResult<()> {
    let caller = require_admin()?;
    let note = note.trim();
    if note.is_empty() || note.len() > 512 {
        return Err(BridgeError::InvalidArgument("a note of 1 to 512 bytes saying how the halt was resolved is required".into()));
    }
    if !state::clear_halt() {
        return Err(BridgeError::InvalidState("the bridge is not halted".into()));
    }
    state::append_audit(None, AuditCtx::note(format!("halt cleared by {caller}: {note}")));
    Ok(())
}

/// Admin: the gas budget and the attempt limit of the mint lane, within hard bounds.
pub fn set_lane_settings(a: LaneSettingsArg) -> BridgeResult<()> {
    let caller = require_admin()?;
    let mut lane = state::read_runtime(|c| c.config.lane.clone());
    lane.gas_budget = a.gas_budget;
    lane.max_attempts = a.max_attempts;
    state::validate_lane(&lane).map_err(BridgeError::InvalidArgument)?;
    state::mutate_runtime(|c| c.config.lane = lane.clone());
    state::append_audit(None, AuditCtx::note(format!("mint lane settings set by {caller}: gas budget {} MIST, {} attempts", lane.gas_budget, lane.max_attempts)));
    Ok(())
}

/// Clear the mint lane's own halt after a human resolved its cause; the note says how. This neither
/// replaces nor refunds anything: an operation whose evidence is still missing stays as it is.
pub fn clear_mint_lane_halt(note: &str) -> BridgeResult<()> {
    let caller = require_admin()?;
    let note = note.trim();
    if note.is_empty() || note.len() > 512 {
        return Err(BridgeError::InvalidArgument("a note of 1 to 512 bytes saying how the halt was resolved is required".into()));
    }
    let cleared = state::mutate_runtime(|c| c.mint_lane_halted.take().is_some());
    if !cleared {
        return Err(BridgeError::InvalidState("the mint lane is not halted".into()));
    }
    state::append_audit(None, AuditCtx::note(format!("mint lane halt cleared by {caller}: {note}")));
    Ok(())
}

/// Admin: abandon a mint operation whose current attempt is proven never signed. Refused for anything
/// that may be a complete signed transaction.
pub fn abandon_mint_op(op_id: u64, note: &str) -> BridgeResult<()> {
    let caller = require_admin()?;
    crate::flows::mint::abandon_never_signed(op_id, note).map_err(BridgeError::InvalidState)?;
    state::append_audit(None, AuditCtx::op(op_id, format!("mint operation abandoned by {caller}: {}", note.trim())));
    Ok(())
}

/// Controller: the one SUI coin object (owned by the gas role address) that pays for mints. It is an
/// identity like the gate binding: it cannot be one of the gate's own objects, and it cannot change
/// while a mint operation is in flight (that operation's transaction names the old coin).
pub fn set_gas_coin(id: &str) -> BridgeResult<()> {
    let caller = require_controller()?;
    let coin = crate::sui::parse_address_strict(id).map_err(|_| BridgeError::InvalidArgument("the gas coin id must be 0x followed by 64 lowercase hex digits".into()))?;
    if coin == [0u8; 32] {
        return Err(BridgeError::InvalidArgument("the gas coin id is zero".into()));
    }
    if let Some(b) = state::read_core(|c| c.binding.clone()) {
        if [b.package, b.gate, b.mint_cap, b.admin_cap].contains(&coin) {
            return Err(BridgeError::InvalidArgument("the gas coin cannot be one of the gate's own objects".into()));
        }
    }
    if state::read_runtime(|c| c.active_mint_op).is_some() {
        return Err(BridgeError::InvalidState("a mint operation is in flight".into()));
    }
    state::mutate_runtime(|c| c.config.lane.gas_coin = Some(coin));
    state::append_audit(None, AuditCtx::note(format!("gas coin set by controller {caller}: {}", crate::sui::format_address(&coin))));
    Ok(())
}

pub fn set_admins(admins: Vec<Principal>) -> BridgeResult<()> {
    let caller = require_controller()?;
    if admins.is_empty() || admins.iter().any(|p| *p == Principal::anonymous()) {
        return Err(BridgeError::InvalidArgument("admins must be non-empty and non-anonymous".into()));
    }
    state::mutate_runtime(|c| c.config.admins = admins.clone());
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
        state::mutate_runtime(|c| {
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
        assert_eq!(set_fees(BridgeFees::default()), Err(BridgeError::Anonymous));
        assert_eq!(clear_halt("x"), Err(BridgeError::Anonymous));
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
        assert_eq!(state::read_runtime(|c| c.config.admins.clone()), vec![new_admin]);
        assert!(matches!(set_admins(vec![]), Err(BridgeError::InvalidArgument(_))));
        assert!(matches!(set_admins(vec![new_admin, Principal::anonymous()]), Err(BridgeError::InvalidArgument(_))));
        assert_eq!(state::audit_len(), 1, "only the successful change is logged");
    }

    #[test]
    fn limits_and_fees_are_bounded_and_audited() {
        setup();
        test_env::set_caller(admin());
        let mut l = state::default_limits();
        l.max_deposit_e8s *= 2;
        l.max_outstanding_e8s *= 2;
        assert!(set_limits(l.clone()).is_ok());
        assert_eq!(state::read_runtime(|c| c.config.limits.clone()), l);
        let mut bad = l.clone();
        bad.min_deposit_e8s = 0;
        assert!(matches!(set_limits(bad), Err(BridgeError::InvalidArgument(_))));
        assert!(set_fees(BridgeFees { deposit_fee_e8s: 1_000, release_fee_e8s: 1_000 }).is_ok());
        assert!(matches!(set_fees(BridgeFees { deposit_fee_e8s: l.min_deposit_e8s, release_fee_e8s: 0 }), Err(BridgeError::InvalidArgument(_))));
        // Tightening the limits cannot orphan an existing fee setting.
        let mut tight = l;
        tight.min_deposit_e8s = 1_000;
        assert!(matches!(set_limits(tight), Err(BridgeError::InvalidArgument(_))));
        assert_eq!(state::audit_len(), 2, "only the two successful changes are logged");
    }

    #[test]
    fn pausing_is_always_allowed_but_enabling_deposits_needs_the_deployment() {
        setup();
        test_env::set_caller(admin());
        let paused = Pauses { deposits: true, mints: true, releases: true, observer: true };
        assert!(set_pauses(paused).is_ok());
        let open = Pauses { deposits: false, ..Pauses::default() };
        assert!(matches!(set_pauses(open.clone()), Err(BridgeError::NotConfigured(_))), "no gate binding yet");
        assert!(set_pauses(Pauses { deposits: true, ..Pauses::default() }).is_ok());
        // A halt blocks every unpause.
        state::halt("test");
        assert!(matches!(set_pauses(Pauses { deposits: true, mints: true, releases: true, observer: true }), Ok(())));
        assert!(matches!(set_pauses(Pauses { deposits: true, mints: false, releases: true, observer: true }), Err(BridgeError::Halted(_))));
    }

    #[test]
    fn clearing_a_halt_needs_a_note_and_a_halt() {
        setup();
        test_env::set_caller(admin());
        assert!(matches!(clear_halt("fixed"), Err(BridgeError::InvalidState(_))), "not halted");
        state::halt("conflict");
        assert!(matches!(clear_halt("  "), Err(BridgeError::InvalidArgument(_))));
        assert!(matches!(clear_halt(&"x".repeat(513)), Err(BridgeError::InvalidArgument(_))));
        assert!(clear_halt("checked the ledger by hand").is_ok());
        assert!(!state::is_halted());
    }
}
