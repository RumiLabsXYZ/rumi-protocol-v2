//! Authorization and synchronous admin configuration.
//!
//! Admins can pause, tune limits/fees/gas/observer settings within hard bounds, and
//! replace RPC providers. They cannot move escrow (only the release, refund and fee
//! sweep paths can) and cannot change the ledger, chain id or key. Controllers manage
//! the admin list.

use crate::env;
use crate::state::{self, validate_fees, validate_limits, validate_rpc_services, AuditCtx, GasSettings, ObserverSettings};
use crate::types::*;
use candid::{Nat, Principal};
use num_traits::ToPrimitive;

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

/// Pausing is always allowed. Unpausing deposits needs a bound contract and no halt.
pub fn set_pauses(p: Pauses) -> BridgeResult<()> {
    let caller = require_admin()?;
    let (bound, halted) = state::read_core(|c| (c.config.contract.is_some(), c.halted.clone()));
    if !p.deposits && !bound {
        return Err(BridgeError::NotConfigured("bind the Base contract before enabling deposits".into()));
    }
    if let Some(h) = halted {
        let current = state::read_core(|c| c.config.pauses.clone());
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
    validate_limits(&l).map_err(BridgeError::InvalidArgument)?;
    let (fees, binding) = state::read_core(|c| (c.config.fees.clone(), c.config.contract.clone()));
    validate_fees(&fees, &l).map_err(BridgeError::InvalidArgument)?;
    if let Some(b) = binding {
        check_limits_within_contract(&l, &b).map_err(BridgeError::InvalidArgument)?;
    }
    state::mutate_core(|c| c.config.limits = l.clone());
    state::append_audit(None, AuditCtx::note(format!("limits set by {caller}: {l:?}")));
    Ok(())
}

/// Canister limits must never let more through than the contract allows, so the
/// contract's checks are a second line of defense and not the first to trip.
pub fn check_limits_within_contract(l: &BridgeLimits, b: &state::ContractBinding) -> Result<(), String> {
    if l.max_outstanding_e8s as u128 > b.supply_cap {
        return Err(format!("max_outstanding_e8s {} exceeds contract supplyCap {}", l.max_outstanding_e8s, b.supply_cap));
    }
    if l.mint_window_limit_e8s as u128 > b.mint_window_limit {
        return Err(format!(
            "mint_window_limit_e8s {} exceeds contract mintWindowLimit {}",
            l.mint_window_limit_e8s, b.mint_window_limit
        ));
    }
    if l.window_seconds < b.mint_window_seconds {
        return Err(format!(
            "window_seconds {} is shorter than contract mintWindowSeconds {}",
            l.window_seconds, b.mint_window_seconds
        ));
    }
    if (l.min_deposit_e8s as u128) < b.min_bridge_out {
        // Not a safety issue for minting, but a deposit below the contract minimum could
        // never be bridged back in one piece. Keep the rule simple and explicit.
        return Err(format!(
            "min_deposit_e8s {} is below contract minBridgeOut {}",
            l.min_deposit_e8s, b.min_bridge_out
        ));
    }
    Ok(())
}

pub fn set_fees(f: BridgeFees) -> BridgeResult<()> {
    let caller = require_admin()?;
    let limits = state::read_core(|c| c.config.limits.clone());
    validate_fees(&f, &limits).map_err(BridgeError::InvalidArgument)?;
    state::mutate_core(|c| c.config.fees = f.clone());
    state::append_audit(None, AuditCtx::note(format!("fees set by {caller}: {f:?}")));
    Ok(())
}

fn nat_u128(n: &Nat, field: &str) -> BridgeResult<u128> {
    n.0.to_u128().ok_or_else(|| BridgeError::InvalidArgument(format!("{field} does not fit in u128")))
}

pub fn set_gas(g: GasConfig) -> BridgeResult<()> {
    let caller = require_admin()?;
    let s = GasSettings {
        max_fee_per_gas_wei: nat_u128(&g.max_fee_per_gas_wei, "max_fee_per_gas_wei")?,
        max_priority_fee_per_gas_wei: nat_u128(&g.max_priority_fee_per_gas_wei, "max_priority_fee_per_gas_wei")?,
        mint_gas_limit: g.mint_gas_limit,
        admin_gas_limit: g.admin_gas_limit,
        min_gas_reserve_wei: nat_u128(&g.min_gas_reserve_wei, "min_gas_reserve_wei")?,
        replace_after_secs: g.replace_after_secs,
    };
    validate_gas(&s).map_err(BridgeError::InvalidArgument)?;
    state::mutate_core(|c| c.config.gas = s.clone());
    state::append_audit(None, AuditCtx::note(format!("gas set by {caller}: {s:?}")));
    Ok(())
}

pub fn validate_gas(s: &GasSettings) -> Result<(), String> {
    if s.max_fee_per_gas_wei == 0 || s.max_priority_fee_per_gas_wei > s.max_fee_per_gas_wei {
        return Err("need 0 < max_priority_fee <= max_fee".into());
    }
    // 1000 gwei hard ceiling.
    if s.max_fee_per_gas_wei > 1_000_000_000_000 {
        return Err("max_fee_per_gas above hard ceiling".into());
    }
    for (name, v) in [("mint_gas_limit", s.mint_gas_limit), ("admin_gas_limit", s.admin_gas_limit)] {
        if !(21_000..=1_000_000).contains(&v) {
            return Err(format!("{name} must be in 21000..=1000000"));
        }
    }
    if s.replace_after_secs < 10 {
        return Err("replace_after_secs must be >= 10".into());
    }
    Ok(())
}

pub fn set_observer_config(o: ObserverConfig) -> BridgeResult<()> {
    let caller = require_admin()?;
    let s = ObserverSettings {
        logs_chunk_blocks: o.logs_chunk_blocks,
        logs_max_response_bytes: o.logs_max_response_bytes,
        default_max_response_bytes: o.default_max_response_bytes,
        idle_interval_secs: o.idle_interval_secs,
        active_interval_secs: o.active_interval_secs,
        cycles_per_rpc_call: nat_u128(&o.cycles_per_rpc_call, "cycles_per_rpc_call")?,
    };
    validate_observer(&s).map_err(BridgeError::InvalidArgument)?;
    state::mutate_core(|c| c.config.observer = s.clone());
    state::append_audit(None, AuditCtx::note(format!("observer config set by {caller}: {s:?}")));
    Ok(())
}

pub fn validate_observer(s: &ObserverSettings) -> Result<(), String> {
    if !(1..=10_000).contains(&s.logs_chunk_blocks) {
        return Err("logs_chunk_blocks must be in 1..=10000".into());
    }
    if !(1_024..=2_000_000).contains(&s.logs_max_response_bytes) || !(1_024..=2_000_000).contains(&s.default_max_response_bytes) {
        return Err("response caps must be in 1024..=2000000".into());
    }
    if s.active_interval_secs < 5 || s.idle_interval_secs < s.active_interval_secs || s.idle_interval_secs > 86_400 {
        return Err("need 5 <= active_interval <= idle_interval <= 86400".into());
    }
    if s.cycles_per_rpc_call == 0 || s.cycles_per_rpc_call > 100_000_000_000 {
        return Err("cycles_per_rpc_call must be in 1..=100B".into());
    }
    Ok(())
}

pub fn set_rpc_services(services: Vec<RpcServiceConfig>, min_quorum: u8) -> BridgeResult<()> {
    let caller = require_admin()?;
    let chain = state::read_core(|c| c.config.chain_id);
    validate_rpc_services(chain, &services, min_quorum).map_err(BridgeError::InvalidArgument)?;
    state::mutate_core(|c| {
        c.config.rpc_services = services.clone();
        c.config.min_quorum = min_quorum;
    });
    state::append_audit(None, AuditCtx::note(format!("rpc services set by {caller}: {} services, quorum {min_quorum}", services.len())));
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

    fn setup() {
        state::reset_for_tests();
        state::mutate_core(|c| {
            c.initialized = true;
            c.config.admins = vec![admin()];
            c.config.chain_id = state::CHAIN_BASE_SEPOLIA;
        });
    }

    #[test]
    fn anonymous_and_non_admin_are_rejected() {
        setup();
        test_env::set_caller(Principal::anonymous());
        assert_eq!(set_fees(BridgeFees { deposit_fee_e8s: 0, release_fee_e8s: 0 }), Err(BridgeError::Anonymous));
        test_env::set_caller(Principal::from_slice(&[4; 10]));
        assert_eq!(set_fees(BridgeFees { deposit_fee_e8s: 0, release_fee_e8s: 0 }), Err(BridgeError::Unauthorized));
        assert_eq!(set_admins(vec![admin()]), Err(BridgeError::Unauthorized));
    }

    #[test]
    fn deposits_cannot_be_unpaused_before_binding_or_while_halted() {
        setup();
        test_env::set_caller(admin());
        let open = Pauses { deposits: false, mints: false, releases: false, observer: false };
        assert!(matches!(set_pauses(open.clone()), Err(BridgeError::NotConfigured(_))));
        state::mutate_core(|c| {
            c.config.contract = Some(state::ContractBinding {
                address: [1; 20],
                bound_at_ns: 0,
                start_block: 0,
                supply_cap: u128::MAX,
                mint_window_limit: u128::MAX,
                mint_window_seconds: 1,
                min_bridge_out: 1,
            })
        });
        state::halt("test");
        assert!(matches!(set_pauses(open.clone()), Err(BridgeError::Halted(_))));
        // Pausing everything is still allowed while halted.
        let all = Pauses { deposits: true, mints: true, releases: true, observer: true };
        assert!(set_pauses(all).is_ok());
    }

    #[test]
    fn limits_must_fit_inside_contract_limits() {
        let b = state::ContractBinding {
            address: [1; 20],
            bound_at_ns: 0,
            start_block: 0,
            supply_cap: 1_000,
            mint_window_limit: 500,
            mint_window_seconds: 3_600,
            min_bridge_out: 10,
        };
        let mut l = state::default_limits();
        l.max_outstanding_e8s = 1_000;
        l.max_deposit_e8s = 100;
        l.min_deposit_e8s = 10;
        l.mint_window_limit_e8s = 500;
        l.window_seconds = 3_600;
        assert!(check_limits_within_contract(&l, &b).is_ok());
        let mut x = l.clone();
        x.max_outstanding_e8s = 1_001;
        assert!(check_limits_within_contract(&x, &b).is_err());
        let mut x = l.clone();
        x.mint_window_limit_e8s = 501;
        assert!(check_limits_within_contract(&x, &b).is_err());
        let mut x = l.clone();
        x.window_seconds = 3_599;
        assert!(check_limits_within_contract(&x, &b).is_err());
        let mut x = l;
        x.min_deposit_e8s = 9;
        assert!(check_limits_within_contract(&x, &b).is_err());
    }

    #[test]
    fn gas_and_observer_bounds() {
        assert!(validate_gas(&GasSettings::default()).is_ok());
        let mut g = GasSettings::default();
        g.max_priority_fee_per_gas_wei = g.max_fee_per_gas_wei + 1;
        assert!(validate_gas(&g).is_err());
        let mut g = GasSettings::default();
        g.mint_gas_limit = 20_000;
        assert!(validate_gas(&g).is_err());
        assert!(validate_observer(&ObserverSettings::default()).is_ok());
        let mut o = ObserverSettings::default();
        o.active_interval_secs = 1;
        assert!(validate_observer(&o).is_err());
    }

    #[test]
    fn controllers_set_admins() {
        setup();
        let ctl = Principal::from_slice(&[7; 10]);
        test_env::set_controllers(vec![ctl]);
        test_env::set_caller(ctl);
        assert!(set_admins(vec![]).is_err());
        assert!(set_admins(vec![Principal::anonymous()]).is_err());
        assert!(set_admins(vec![admin(), ctl]).is_ok());
        assert!(state::read_core(|c| c.is_admin(&ctl)));
    }
}
