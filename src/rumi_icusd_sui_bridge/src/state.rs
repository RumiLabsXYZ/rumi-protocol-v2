//! Stable state for the instance skeleton: one CBOR record in a `StableCell`, the same pattern
//! as the frozen Solana instance (`rumi_icusd_sol_bridge` at 23dfe841). CBOR maps fields by
//! name, so a field added later with `#[serde(default)]` decodes from an older image; the
//! schema version is stamped at upgrade by [`migrate_core`].

use crate::gate::ConfiguredGate;
use crate::graphql::{validate_settings, MirrorSettings};
use crate::ids::{network_of, SuiNetwork, SuiRole};
use crate::sui::{address_from_ed25519, parse_address_strict, Bytes32};
use crate::tx::RoleAddresses;
use crate::types::*;
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::storable::Bound;
use ic_stable_structures::{DefaultMemoryImpl, StableCell, Storable};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;

pub use crate::runtime::*;

pub(crate) type Mem = VirtualMemory<DefaultMemoryImpl>;

/// Version of the stored core record. 1 = BB-52 foundation.
pub const CORE_SCHEMA_VERSION: u32 = 1;
const MEM_CORE: u8 = 0;

pub fn cbor_encode<T: Serialize>(v: &T) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Err(e) = ciborium::ser::into_writer(v, &mut buf) {
        panic!("CBOR encode of bridge state failed: {e}");
    }
    buf
}

pub fn try_cbor_decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    ciborium::de::from_reader(bytes).map_err(|e| e.to_string())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredBinding {
    pub package: Bytes32,
    pub gate: Bytes32,
    pub mint_cap: Bytes32,
    pub admin_cap: Bytes32,
}

impl StoredBinding {
    pub fn configured(&self) -> ConfiguredGate {
        ConfiguredGate { package: self.package, gate: self.gate, mint_cap: self.mint_cap, admin_cap: self.admin_cap }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredRoleKey {
    pub role: SuiRole,
    pub public_key: Bytes32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SuiCore {
    pub schema_version: u32,
    #[serde(default)]
    pub initialized: bool,
    pub network: SuiNetwork,
    pub chain_identifier: Bytes32,
    pub instance_salt: Bytes32,
    #[serde(default)]
    pub binding: Option<StoredBinding>,
    #[serde(default)]
    pub roles: Vec<StoredRoleKey>,
    #[serde(default)]
    pub mirror: MirrorSettings,
}

impl Default for SuiCore {
    fn default() -> Self {
        SuiCore {
            schema_version: CORE_SCHEMA_VERSION,
            initialized: false,
            network: SuiNetwork::Testnet,
            chain_identifier: [0; 32],
            instance_salt: [0; 32],
            binding: None,
            roles: vec![],
            mirror: MirrorSettings::default(),
        }
    }
}

impl Storable for SuiCore {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(cbor_encode(self))
    }
    // `Storable::from_bytes` has no error channel: a core record that does not decode is
    // corrupt state, and trapping (which rolls the message or upgrade back) is the only safe answer.
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        match try_cbor_decode(bytes.as_ref()) {
            Ok(v) => v,
            Err(e) => panic!("stored bridge core does not decode: {e}"),
        }
    }
    const BOUND: Bound = Bound::Unbounded;
}

thread_local! {
    static MEMORY_MANAGER: RefCell<MemoryManager<DefaultMemoryImpl>> =
        RefCell::new(MemoryManager::init(DefaultMemoryImpl::default()));

    static CORE: RefCell<StableCell<SuiCore, Mem>> = RefCell::new(
        StableCell::init(mem(MEM_CORE), SuiCore::default()).expect("init core cell"));
}

pub(crate) fn mem(id: u8) -> Mem {
    MEMORY_MANAGER.with(|m| m.borrow().get(MemoryId::new(id)))
}

pub fn read_core<R>(f: impl FnOnce(&SuiCore) -> R) -> R {
    CORE.with(|c| f(c.borrow().get()))
}

/// Read-modify-write; the closure runs on a copy that is written back only when it returns.
pub fn mutate_core<R>(f: impl FnOnce(&mut SuiCore) -> R) -> R {
    CORE.with(|c| {
        let mut cell = c.borrow_mut();
        let mut core = cell.get().clone();
        let r = f(&mut core);
        cell.set(core).expect("write core cell");
        r
    })
}

/// Upgrade step: bring a stored core to the current schema. Returns whether it changed.
pub fn migrate_core() -> bool {
    mutate_core(|c| {
        if c.schema_version < CORE_SCHEMA_VERSION {
            c.schema_version = CORE_SCHEMA_VERSION;
            true
        } else {
            false
        }
    })
}

/// The install-time rule, also re-checked at upgrade: the chain identifier names exactly the
/// declared network, the network is one of the two accepted, and the mirror settings are valid.
pub fn validate_stored_config(c: &SuiCore) -> Result<(), String> {
    match network_of(&c.chain_identifier) {
        Some(n) if n == c.network => {}
        Some(_) => return Err("the chain identifier belongs to the other network".into()),
        None => return Err("the chain identifier is not a supported network's".into()),
    }
    validate_settings(&c.mirror)
}

pub fn parse_chain_identifier_hex(s: &str) -> Result<Bytes32, String> {
    if s.len() != 64 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err("the chain identifier must be 64 lowercase hex digits".into());
    }
    let mut out = [0u8; 32];
    hex::decode_to_slice(s, &mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Validate install arguments into the fields of a core record.
pub fn validate_init(args: &InitArgs) -> Result<(Bytes32, MirrorSettings), String> {
    let chain = parse_chain_identifier_hex(&args.chain_identifier)?;
    match network_of(&chain) {
        None => return Err("only Sui mainnet and testnet are supported; this chain identifier is neither".into()),
        Some(n) if n != args.network => return Err(format!("the chain identifier is not {:?}'s", args.network)),
        Some(_) => {}
    }
    let mirror = args.mirror.clone().unwrap_or_default();
    validate_settings(&mirror)?;
    Ok((chain, mirror))
}

/// Validate and convert a gate binding argument. All four ids must be strict `0x` plus 64
/// lowercase hex, non-zero and distinct.
pub fn validate_binding(a: &GateBindingArg) -> Result<StoredBinding, String> {
    let parse = |name: &str, v: &str| parse_address_strict(v).map_err(|_| format!("{name} must be 0x followed by 64 lowercase hex digits"));
    let b = StoredBinding {
        package: parse("package_id", &a.package_id)?,
        gate: parse("gate_id", &a.gate_id)?,
        mint_cap: parse("mint_cap_id", &a.mint_cap_id)?,
        admin_cap: parse("admin_cap_id", &a.admin_cap_id)?,
    };
    b.configured().validate()?;
    Ok(b)
}

/// The role set must be exactly the three roles, each with a distinct non-zero key and a
/// distinct address. Domain separation of the three roles is checked here, on the derived keys,
/// as well as in the derivation paths.
pub fn validate_role_keys(keys: &[(SuiRole, Bytes32)]) -> Result<RoleAddresses, String> {
    let get = |role: SuiRole| -> Result<Bytes32, String> {
        let mut found = keys.iter().filter(|(r, _)| *r == role);
        let first = found.next().ok_or_else(|| format!("missing the {role:?} key"))?;
        if found.next().is_some() {
            return Err(format!("the {role:?} key appears twice"));
        }
        Ok(first.1)
    };
    if keys.len() != SuiRole::ALL.len() {
        return Err("exactly three role keys are required".into());
    }
    let (t, g, c) = (get(SuiRole::Treasury)?, get(SuiRole::Gas)?, get(SuiRole::UpgradeCustody)?);
    for (role, key) in [(SuiRole::Treasury, t), (SuiRole::Gas, g), (SuiRole::UpgradeCustody, c)] {
        if key == [0u8; 32] {
            return Err(format!("the {role:?} key is zero"));
        }
    }
    let roles = RoleAddresses { treasury: address_from_ed25519(&t), gas: address_from_ed25519(&g), upgrade_custody: address_from_ed25519(&c) };
    if t == g || t == c || g == c {
        return Err("two roles derived the same key".into());
    }
    roles.validate().map_err(|e| format!("{e}"))?;
    Ok(roles)
}

/// Store the derived role keys, once. A second call with the same keys is a no-op; different
/// keys are a conflict (the identity of a role never changes).
pub fn set_role_keys(keys: Vec<(SuiRole, Bytes32)>) -> Result<RoleAddresses, SuiBridgeError> {
    let addresses = validate_role_keys(&keys).map_err(SuiBridgeError::InvalidArgument)?;
    let stored: Vec<StoredRoleKey> = SuiRole::ALL
        .iter()
        .map(|role| StoredRoleKey { role: *role, public_key: keys.iter().find(|(r, _)| r == role).expect("validated").1 })
        .collect();
    mutate_core(|c| {
        if !c.roles.is_empty() && c.roles != stored {
            return Err(SuiBridgeError::Conflict("role keys are already derived and differ".into()));
        }
        c.roles = stored;
        Ok(addresses)
    })
}

/// Store the gate binding, once. The same binding again is a no-op; a different one is a conflict.
pub fn set_binding(b: StoredBinding) -> Result<(), SuiBridgeError> {
    mutate_core(|c| {
        if !c.initialized {
            return Err(SuiBridgeError::NotConfigured("the instance is not initialized".into()));
        }
        match &c.binding {
            Some(existing) if *existing != b => Err(SuiBridgeError::Conflict("the gate binding is already set to different ids".into())),
            _ => {
                c.binding = Some(b);
                Ok(())
            }
        }
    })
}

#[cfg(test)]
pub fn reset_for_tests() {
    mutate_core(|c| *c = SuiCore::default());
    crate::runtime::reset_runtime_for_tests();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{MAINNET_CHAIN_IDENTIFIER, TESTNET_CHAIN_IDENTIFIER};
    use crate::vectors::*;
    use candid::Principal;

    fn init_args(network: SuiNetwork, chain: &Bytes32) -> InitArgs {
        InitArgs {
            network,
            chain_identifier: hex::encode(chain),
            mirror: None,
            icusd_ledger: Principal::from_slice(&[0x1c; 10]),
            admins: vec![Principal::from_slice(&[3; 10])],
            fee_recipient: None,
            fees: None,
            limits: None,
        }
    }

    fn sample_core() -> SuiCore {
        SuiCore {
            schema_version: CORE_SCHEMA_VERSION,
            initialized: true,
            network: SuiNetwork::Testnet,
            chain_identifier: TESTNET_CHAIN_IDENTIFIER,
            instance_salt: [9; 32],
            binding: Some(StoredBinding { package: PACKAGE, gate: GATE_ID, mint_cap: MINT_CAP_ID, admin_cap: ADMIN_CAP_ID }),
            roles: vec![
                StoredRoleKey { role: SuiRole::Treasury, public_key: h32(PUB_TREASURY) },
                StoredRoleKey { role: SuiRole::Gas, public_key: h32(PUB_GAS) },
                StoredRoleKey { role: SuiRole::UpgradeCustody, public_key: h32(PUB_CUSTODY) },
            ],
            mirror: MirrorSettings::default(),
        }
    }

    #[test]
    fn init_requires_a_chain_identifier_that_matches_the_declared_network() {
        assert!(validate_init(&init_args(SuiNetwork::Mainnet, &MAINNET_CHAIN_IDENTIFIER)).is_ok());
        assert!(validate_init(&init_args(SuiNetwork::Testnet, &TESTNET_CHAIN_IDENTIFIER)).is_ok());
        assert!(validate_init(&init_args(SuiNetwork::Mainnet, &TESTNET_CHAIN_IDENTIFIER)).is_err(), "wrong-network input is refused");
        assert!(validate_init(&init_args(SuiNetwork::Testnet, &MAINNET_CHAIN_IDENTIFIER)).is_err());
        assert!(validate_init(&init_args(SuiNetwork::Testnet, &CHAIN_A)).is_err(), "an unknown chain is refused, not guessed");
        let upper = "AB".repeat(32);
        let prefixed = format!("0x{}", "ab".repeat(31));
        for bad in ["", "ab", upper.as_str(), prefixed.as_str()] {
            let mut a = init_args(SuiNetwork::Testnet, &TESTNET_CHAIN_IDENTIFIER);
            a.chain_identifier = bad.to_string();
            assert!(validate_init(&a).is_err(), "{bad:?}");
        }
        let mut a = init_args(SuiNetwork::Testnet, &TESTNET_CHAIN_IDENTIFIER);
        a.mirror = Some(MirrorSettings { min_quorum: 1, ..MirrorSettings::default() });
        assert!(validate_init(&a).is_err(), "invalid mirror settings");
    }

    #[test]
    fn the_stored_config_is_rechecked_at_upgrade() {
        assert!(validate_stored_config(&sample_core()).is_ok());
        let mut c = sample_core();
        c.network = SuiNetwork::Mainnet;
        assert!(validate_stored_config(&c).is_err());
        let mut c = sample_core();
        c.chain_identifier = CHAIN_A;
        assert!(validate_stored_config(&c).is_err());
    }

    #[test]
    fn binding_arguments_are_strict() {
        let good = GateBindingArg {
            package_id: format!("0x{}", hex::encode(PACKAGE)),
            gate_id: format!("0x{}", hex::encode(GATE_ID)),
            mint_cap_id: format!("0x{}", hex::encode(MINT_CAP_ID)),
            admin_cap_id: format!("0x{}", hex::encode(ADMIN_CAP_ID)),
        };
        assert_eq!(validate_binding(&good).unwrap().configured(), configured());
        let with = |f: &dyn Fn(&mut GateBindingArg)| {
            let mut a = good.clone();
            f(&mut a);
            validate_binding(&a)
        };
        assert!(with(&|a| a.package_id = "0x2".into()).is_err(), "short form");
        assert!(with(&|a| a.gate_id = a.gate_id.to_uppercase()).is_err());
        assert!(with(&|a| a.mint_cap_id = a.admin_cap_id.clone()).is_err(), "caps must differ");
        assert!(with(&|a| a.gate_id = format!("0x{}", "00".repeat(32))).is_err(), "zero id");
        assert!(with(&|a| a.admin_cap_id = a.package_id.clone()).is_err());
    }

    #[test]
    fn role_keys_must_be_three_distinct_keys() {
        let t = h32(PUB_TREASURY);
        let g = h32(PUB_GAS);
        let c = h32(PUB_CUSTODY);
        let ok = vec![(SuiRole::Treasury, t), (SuiRole::Gas, g), (SuiRole::UpgradeCustody, c)];
        let roles = validate_role_keys(&ok).unwrap();
        assert_eq!(hex::encode(roles.treasury), ADDR_TREASURY);
        assert_eq!(hex::encode(roles.gas), ADDR_GAS);
        assert_eq!(hex::encode(roles.upgrade_custody), ADDR_CUSTODY);
        assert!(validate_role_keys(&ok[..2]).is_err(), "missing role");
        assert!(validate_role_keys(&[(SuiRole::Treasury, t), (SuiRole::Gas, t), (SuiRole::UpgradeCustody, c)]).is_err(), "treasury and gas share a key");
        assert!(validate_role_keys(&[(SuiRole::Treasury, t), (SuiRole::Gas, g), (SuiRole::UpgradeCustody, g)]).is_err());
        assert!(validate_role_keys(&[(SuiRole::Treasury, t), (SuiRole::Treasury, g), (SuiRole::UpgradeCustody, c)]).is_err(), "duplicate role");
        assert!(validate_role_keys(&[(SuiRole::Treasury, [0; 32]), (SuiRole::Gas, g), (SuiRole::UpgradeCustody, c)]).is_err(), "zero key");
    }

    #[test]
    fn the_core_round_trips_and_older_images_decode() {
        let core = sample_core();
        let image = cbor_encode(&core);
        assert_eq!(try_cbor_decode::<SuiCore>(&image).unwrap(), core);
        // An image written before `binding`, `roles` and `mirror` existed decodes with defaults.
        use ciborium::Value;
        let full: Value = ciborium::de::from_reader(image.as_slice()).unwrap();
        let Value::Map(entries) = full else { panic!("core is a CBOR map") };
        let old: Vec<(Value, Value)> = entries
            .into_iter()
            .filter(|(k, _)| !matches!(k, Value::Text(t) if t == "binding" || t == "roles" || t == "mirror" || t == "initialized"))
            .collect();
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&Value::Map(old), &mut bytes).unwrap();
        let decoded: SuiCore = try_cbor_decode(&bytes).unwrap();
        assert!(!decoded.initialized && decoded.binding.is_none() && decoded.roles.is_empty());
        assert_eq!(decoded.mirror, MirrorSettings::default());
        assert_eq!(decoded.chain_identifier, TESTNET_CHAIN_IDENTIFIER);
    }

    #[test]
    fn core_state_survives_a_stable_cell_round_trip_and_set_once_values_conflict() {
        reset_for_tests();
        mutate_core(|c| {
            *c = sample_core();
            c.binding = None;
            c.roles = vec![];
        });
        let b = validate_binding(&GateBindingArg {
            package_id: format!("0x{}", hex::encode(PACKAGE)),
            gate_id: format!("0x{}", hex::encode(GATE_ID)),
            mint_cap_id: format!("0x{}", hex::encode(MINT_CAP_ID)),
            admin_cap_id: format!("0x{}", hex::encode(ADMIN_CAP_ID)),
        })
        .unwrap();
        set_binding(b.clone()).unwrap();
        set_binding(b.clone()).unwrap();
        let mut other = b.clone();
        other.gate = [0x6b; 32];
        assert!(matches!(set_binding(other), Err(SuiBridgeError::Conflict(_))));
        assert_eq!(read_core(|c| c.binding.clone()), Some(b));

        let keys = vec![(SuiRole::Treasury, h32(PUB_TREASURY)), (SuiRole::Gas, h32(PUB_GAS)), (SuiRole::UpgradeCustody, h32(PUB_CUSTODY))];
        assert!(set_role_keys(keys.clone()).is_ok());
        assert!(set_role_keys(keys).is_ok(), "the same keys again are a no-op");
        let swapped = vec![(SuiRole::Treasury, h32(PUB_GAS)), (SuiRole::Gas, h32(PUB_TREASURY)), (SuiRole::UpgradeCustody, h32(PUB_CUSTODY))];
        assert!(matches!(set_role_keys(swapped), Err(SuiBridgeError::Conflict(_))), "a role's key never changes");

        assert!(!migrate_core(), "already at the current schema");
        mutate_core(|c| c.schema_version = 0);
        assert!(migrate_core());
        assert_eq!(read_core(|c| c.schema_version), CORE_SCHEMA_VERSION);
        reset_for_tests();
    }

    #[test]
    fn binding_needs_an_initialized_instance() {
        reset_for_tests();
        let b = StoredBinding { package: PACKAGE, gate: GATE_ID, mint_cap: MINT_CAP_ID, admin_cap: ADMIN_CAP_ID };
        assert!(matches!(set_binding(b), Err(SuiBridgeError::NotConfigured(_))));
    }
}
