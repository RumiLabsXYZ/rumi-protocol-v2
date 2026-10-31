//! The observed binding (53b): what the providers agree the configured deployment IS, kept apart
//! from what the controllers configured (`SuiCore.binding`).
//!
//! `observe_deployment` reads, through the strict provider quorum and at ONE finalized checkpoint,
//! the exact configured `Gate` (type, shared ownership, version, chain identifier, the supply
//! identity, the table sizes), the `MintCap` (type, bound to this gate, owned by the treasury role
//! address), the `AdminCap` (type, bound to this gate, address-owned; its owner is recorded and
//! disclosed, never required to be ours) and the coin's registered decimals. Only an agreed,
//! internally consistent observation is stored; anything else leaves the previous state alone and
//! names what failed. Deposits and the mint lane both require a stored observation.
//!
//! What this does NOT establish (carried as honest omissions, MC-DEF-13):
//!
//! * the published module bytes: no `sui move build` module digest exists to compare, so
//!   `module_digest_observed` is false;
//! * who holds the `UpgradeCap`, its policy, or that the package is the code at 23f75ef1;
//! * that the currency is not regulated (no `DenyCap` was looked for; the Move never calls
//!   `make_regulated`, and the bind check does not read the registry's regulated state);
//! * any provider's real behavior (all shapes are schema-derived, nothing was queried).
//!
//! A FIRST observation also requires a pristine gate (zero supply, counters and tables): the books of
//! this instance cannot explain activity that happened before it was bound. A refresh must keep
//! every identity and may only move the counters forward.

use crate::gate::{ConfiguredGate, DECIMALS};
use crate::graphql::Exclusion;
use crate::ids::SuiRole;
use crate::mirror::{settle, Mirror, SuiTransport};
use crate::reads::{interpret_admin_cap, interpret_mint_cap};
use crate::state::{self, AuditCtx, ObservedDeployment, StoredRef};
use crate::sui::{address_from_ed25519, format_address, Bytes32};
use crate::tx::RoleAddresses;

pub const EXPECTED_SYMBOL: &str = "icUSD";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindError {
    /// Something the observation needs is not configured (binding, role keys, enabled mirror).
    NotReady(String),
    /// The providers did not give an agreed answer; try again later.
    Unavailable(String),
    /// What the providers agree on is not the configured deployment.
    Mismatch(String),
}

/// The configured identities and the three role addresses and treasury/gas public keys.
#[derive(Clone, Copy, Debug)]
pub struct LaneIdentity {
    pub cfg: ConfiguredGate,
    pub roles: RoleAddresses,
    pub treasury_key: Bytes32,
    pub gas_key: Bytes32,
}

/// The binding and the three role keys, or what is missing.
pub fn lane_identity() -> Result<LaneIdentity, String> {
    let (binding, roles) = state::read_core(|c| (c.binding.clone(), c.roles.clone()));
    let binding = binding.ok_or("the gate binding is not configured")?;
    let key = |role: SuiRole| roles.iter().find(|k| k.role == role).map(|k| k.public_key);
    let (Some(treasury), Some(gas), Some(custody)) = (key(SuiRole::Treasury), key(SuiRole::Gas), key(SuiRole::UpgradeCustody)) else {
        return Err("the role keys are not derived".into());
    };
    let addresses = RoleAddresses {
        treasury: address_from_ed25519(&treasury),
        gas: address_from_ed25519(&gas),
        upgrade_custody: address_from_ed25519(&custody),
    };
    addresses.validate().map_err(|e| format!("{e}"))?;
    Ok(LaneIdentity { cfg: binding.configured(), roles: addresses, treasury_key: treasury, gas_key: gas })
}

pub fn exclusion_text(e: &Exclusion) -> String {
    match e {
        Exclusion::WrongNetwork(n) => format!("wrong network {n}"),
        Exclusion::Identity(m) | Exclusion::Malformed(m) | Exclusion::Transport(m) => m.clone(),
    }
}

fn role_of(addr: &Bytes32, roles: &RoleAddresses) -> Option<SuiRole> {
    if addr == &roles.treasury {
        Some(SuiRole::Treasury)
    } else if addr == &roles.gas {
        Some(SuiRole::Gas)
    } else if addr == &roles.upgrade_custody {
        Some(SuiRole::UpgradeCustody)
    } else {
        None
    }
}

/// Whether `new` may replace `prev` (or be the first observation). Pure.
///
/// A first observation needs a pristine gate. A refresh keeps the shared version and the cap ids,
/// and no counter or version may move backwards (a gate whose history rewound is not the gate that
/// was bound).
pub fn accept_observation(prev: Option<&ObservedDeployment>, new: &ObservedDeployment) -> Result<(), String> {
    match prev {
        None => {
            if new.minted_total != 0 || new.burned_total != 0 || new.mint_count != 0 || new.bridge_out_count != 0 || new.total_supply != 0 || new.mints_table_size != 0 || new.returns_table_size != 0 {
                return Err(
                    "the gate already has activity (supply, counters or table entries): the books of this instance cannot explain it, so it is not bound".into(),
                );
            }
            Ok(())
        }
        Some(p) => {
            if p.initial_shared_version != new.initial_shared_version || p.mint_cap.id != new.mint_cap.id || p.admin_cap.id != new.admin_cap.id {
                return Err("the gate or a capability changed identity since the first observation".into());
            }
            if new.gate_object_version < p.gate_object_version
                || new.mint_count < p.mint_count
                || new.bridge_out_count < p.bridge_out_count
                || new.minted_total < p.minted_total
                || new.burned_total < p.burned_total
                || new.checkpoint < p.checkpoint
            {
                return Err("a counter or version moved backwards since the previous observation".into());
            }
            Ok(())
        }
    }
}

/// Observe the deployment through the quorum and store the result when it is agreed and acceptable.
pub async fn observe_deployment<T: SuiTransport>(transport: &T) -> Result<ObservedDeployment, BindError> {
    let id = lane_identity().map_err(BindError::NotReady)?;
    let mirror = Mirror::from_state(transport).map_err(BindError::NotReady)?;
    let unavailable = BindError::Unavailable;

    let head = settle("head", mirror.head().await).map_err(unavailable)?.ok_or_else(|| BindError::Unavailable("the providers have no checkpoint".into()))?;
    let cp = settle("checkpoint", mirror.checkpoint_gas(head).await)
        .map_err(BindError::Unavailable)?
        .ok_or_else(|| BindError::Unavailable(format!("the providers do not yet serve checkpoint {head}")))?;
    let gate = settle("gate", mirror.gate(&id.cfg, head).await)
        .map_err(BindError::Unavailable)?
        .ok_or_else(|| BindError::Mismatch(format!("the configured gate does not exist as of checkpoint {head}")))?;

    let cap_obj = settle("mint cap", mirror.owned_object(&id.cfg.mint_cap, head).await)
        .map_err(BindError::Unavailable)?
        .ok_or_else(|| BindError::Mismatch("the configured mint cap does not exist".into()))?;
    let mint_cap = interpret_mint_cap(&cap_obj, &id.cfg, &id.roles.treasury).map_err(|e| BindError::Mismatch(exclusion_text(&e)))?;

    let admin_obj = settle("admin cap", mirror.owned_object(&id.cfg.admin_cap, head).await)
        .map_err(BindError::Unavailable)?
        .ok_or_else(|| BindError::Mismatch("the configured admin cap does not exist".into()))?;
    let admin = interpret_admin_cap(&admin_obj, &id.cfg).map_err(|e| BindError::Mismatch(exclusion_text(&e)))?;

    let meta = settle("coin metadata", mirror.coin_metadata(&id.cfg.coin_type()).await)
        .map_err(BindError::Unavailable)?
        .ok_or_else(|| BindError::Mismatch("no coin metadata for the configured coin type (has the currency been registered?)".into()))?;
    if meta.decimals != DECIMALS {
        return Err(BindError::Mismatch(format!("the coin has {} decimals, not {DECIMALS}", meta.decimals)));
    }
    if meta.symbol != EXPECTED_SYMBOL {
        return Err(BindError::Mismatch(format!("the coin symbol is {:?}, not {EXPECTED_SYMBOL:?}", meta.symbol)));
    }

    let g = &gate.gate;
    let observed = ObservedDeployment {
        checkpoint: head,
        checkpoint_digest: cp.digest,
        epoch: cp.epoch,
        observed_at_ns: crate::env::now_ns(),
        gate_version: g.version,
        gate_object_version: gate.object_version,
        initial_shared_version: gate.initial_shared_version,
        supply_cap: g.supply_cap,
        min_bridge_out: g.min_bridge_out,
        total_supply: g.total_supply,
        minted_total: g.minted_total,
        burned_total: g.burned_total,
        mint_count: g.mint_count,
        bridge_out_count: g.bridge_out_count,
        mints_table_size: g.mints_size,
        returns_table_size: g.returns_size,
        decimals: meta.decimals,
        symbol: meta.symbol,
        mint_cap: StoredRef::from(mint_cap),
        mint_cap_owner: id.roles.treasury,
        admin_cap: StoredRef::from(admin.reference),
        admin_cap_owner: admin.owner,
        admin_cap_role: role_of(&admin.owner, &id.roles),
        module_digest_observed: false,
    };
    let prev = state::read_runtime(|c| c.observed.clone());
    accept_observation(prev.as_ref(), &observed).map_err(BindError::Mismatch)?;
    state::mutate_runtime(|c| c.observed = Some(observed.clone()));
    state::append_audit(
        None,
        AuditCtx::note(format!(
            "deployment observed at checkpoint {head} (epoch {}): gate {} v{}, supply {}, cap {}, mint cap owner {}, admin cap owner {}",
            cp.epoch,
            format_address(&id.cfg.gate),
            g.version,
            g.total_supply,
            g.supply_cap,
            format_address(&id.roles.treasury),
            format_address(&admin.owner)
        )),
    );
    Ok(observed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs() -> ObservedDeployment {
        ObservedDeployment {
            checkpoint: 1_000,
            checkpoint_digest: [1; 32],
            epoch: 3,
            observed_at_ns: 5,
            gate_version: 1,
            gate_object_version: 10,
            initial_shared_version: 4,
            supply_cap: 0,
            min_bridge_out: 1,
            total_supply: 0,
            minted_total: 0,
            burned_total: 0,
            mint_count: 0,
            bridge_out_count: 0,
            mints_table_size: 0,
            returns_table_size: 0,
            decimals: 8,
            symbol: "icUSD".into(),
            mint_cap: StoredRef { id: [0x6d; 32], version: 2, digest: [2; 32] },
            mint_cap_owner: [0x11; 32],
            admin_cap: StoredRef { id: [0x6e; 32], version: 2, digest: [3; 32] },
            admin_cap_owner: [0x33; 32],
            admin_cap_role: None,
            module_digest_observed: false,
        }
    }

    #[test]
    fn a_first_observation_needs_a_pristine_gate() {
        assert!(accept_observation(None, &obs()).is_ok());
        for (what, mutate) in [
            ("minted", (|o: &mut ObservedDeployment| o.minted_total = 1) as fn(&mut ObservedDeployment)),
            ("burned", |o| o.burned_total = 1),
            ("mint count", |o| o.mint_count = 1),
            ("bridge out count", |o| o.bridge_out_count = 1),
            ("supply", |o| o.total_supply = 1),
            ("mints table", |o| o.mints_table_size = 1),
            ("returns table", |o| o.returns_table_size = 1),
        ] {
            let mut o = obs();
            mutate(&mut o);
            assert!(accept_observation(None, &o).is_err(), "{what}");
        }
    }

    #[test]
    fn a_refresh_keeps_identity_and_never_moves_backwards() {
        let prev = {
            let mut p = obs();
            p.mint_count = 4;
            p.minted_total = 400;
            p.total_supply = 400;
            p.gate_object_version = 20;
            p
        };
        let mut next = prev.clone();
        next.checkpoint = 2_000;
        next.gate_object_version = 25;
        next.mint_count = 5;
        next.minted_total = 500;
        next.total_supply = 500;
        assert!(accept_observation(Some(&prev), &next).is_ok());
        assert!(accept_observation(Some(&prev), &prev).is_ok(), "an identical refresh");
        let bad: Vec<(&str, ObservedDeployment)> = vec![
            ("shared version", ObservedDeployment { initial_shared_version: 5, ..next.clone() }),
            ("mint cap id", ObservedDeployment { mint_cap: StoredRef { id: [9; 32], ..next.mint_cap }, ..next.clone() }),
            ("admin cap id", ObservedDeployment { admin_cap: StoredRef { id: [9; 32], ..next.admin_cap }, ..next.clone() }),
            ("object version back", ObservedDeployment { gate_object_version: 19, ..next.clone() }),
            ("mint count back", ObservedDeployment { mint_count: 3, ..next.clone() }),
            ("bridge out back", ObservedDeployment { bridge_out_count: 0, ..prev.clone() }),
            ("minted back", ObservedDeployment { minted_total: 399, ..next.clone() }),
            ("checkpoint back", ObservedDeployment { checkpoint: 999, ..next.clone() }),
        ];
        let mut with_returns = prev.clone();
        with_returns.bridge_out_count = 2;
        with_returns.burned_total = 50;
        with_returns.total_supply = 350;
        with_returns.returns_table_size = 2;
        let mut rewound = with_returns.clone();
        rewound.burned_total = 10;
        let mut cases = bad;
        cases.push(("burned back", rewound));
        for (what, o) in cases {
            let against = if what == "burned back" || what == "bridge out back" { &with_returns } else { &prev };
            assert!(accept_observation(Some(against), &o).is_err(), "{what}");
        }
    }
}
