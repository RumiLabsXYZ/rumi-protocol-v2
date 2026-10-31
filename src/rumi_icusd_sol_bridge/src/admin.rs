//! Authorization guards and the controller-only admin list. Copied from the Base instance's
//! `admin.rs` (the guards and `set_admins`); the pause, limit, fee, gas, observer and RPC
//! setters arrive with the endpoints that use them (BB-48, BB-49).
//!
//! Admins can (later) pause and tune limits within hard bounds. They cannot move escrow (only
//! the release, refund and fee-sweep paths can) and cannot change the ledger, cluster or key.
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
