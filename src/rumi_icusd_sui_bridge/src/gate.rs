//! The configured identities of the Move gate (`move/rumi_icusd_gate` at 23f75ef1) and the
//! decoders for its durable state: the shared `Gate` object, the `mints` table entries and the
//! `returns` table entries.
//!
//! Everything here is a *configured identity or a decoder*, never an observed fact: the package,
//! gate, cap ids, currency type and decimals are what the operator says the deployment is. A
//! provider reading is accepted only when it matches them exactly; see `graphql.rs`.
//!
//! The decoders follow the Move struct layouts field for field (BCS has no field names):
//!
//! ```text
//! Gate        = id:UID(32) version:u64 treasury:TreasuryCap{id:UID(32) total_supply:Supply{value:u64}}
//!               chain_identifier:vector<u8> supply_cap:u64 min_bridge_out:u64
//!               minted_total:u128 burned_total:u128 mint_count:u64 bridge_out_count:u64
//!               mints:Table{id:UID(32) size:u64} returns:Table{id:UID(32) size:u64}
//! MintRecord   = recipient:address(32) amount:u64 sequence:u64
//! ReturnRecord = sender:address(32) amount:u64 sequence:u64 icp_principal:vector<u8>
//!                icp_subaccount:vector<u8> client_nonce:u64
//! ```
//!
//! A `UID` is a struct wrapping an `ID` wrapping an address: 32 raw bytes. `TreasuryCap` and
//! `Supply` are the framework's (`sui::coin`, `sui::balance`); `Table` is `sui::table`'s
//! `{id, size}`. The table keys are `vector<u8>` (32 bytes) and the values the records above,
//! stored as dynamic fields of the table's UID.

use crate::bcs::{BcsError, Reader};
use crate::sui::{parse_address_padded, Bytes32, ObjectId};

/// The Move module that holds the gate (`module rumi_icusd_gate::rumi_icusd_gate`).
pub const MODULE: &str = "rumi_icusd_gate";
pub const GATE_STRUCT: &str = "Gate";
/// The one-time-witness struct, which is also the coin type's name.
pub const COIN_STRUCT: &str = "RUMI_ICUSD_GATE";
pub const MINT_FUNCTION: &str = "mint";
/// Decimals of the currency (`DECIMALS` in the Move module).
pub const DECIMALS: u8 = 8;
/// The gate version this build understands (`VERSION` in the Move module).
pub const GATE_VERSION: u64 = 1;
/// `MAX_SUPPLY_CAP` in the Move module: one below `u64::MAX`.
pub const MAX_SUPPLY_CAP: u64 = u64::MAX - 1;
/// The Move table key type string used to address a dynamic field of `mints` / `returns`.
pub const TABLE_KEY_TYPE: &str = "vector<u8>";
pub const MAX_PRINCIPAL_LEN: usize = 29;
pub const SUBACCOUNT_LEN: usize = 32;
pub const MINT_ID_LEN: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateError {
    Decode(BcsError),
    /// A field disagrees with the configured identity or a Move invariant.
    Mismatch(String),
}

impl core::fmt::Display for GateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl From<BcsError> for GateError {
    fn from(e: BcsError) -> Self {
        GateError::Decode(e)
    }
}

fn mismatch<T>(what: impl Into<String>) -> Result<T, GateError> {
    Err(GateError::Mismatch(what.into()))
}

/// The exact identities the operator configured. Caps are objects whose version and digest
/// change on every use, so only their ids are configured; versions are observed per attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfiguredGate {
    pub package: ObjectId,
    pub gate: ObjectId,
    pub mint_cap: ObjectId,
    pub admin_cap: ObjectId,
}

impl ConfiguredGate {
    /// All four ids must be distinct and non-zero.
    pub fn validate(&self) -> Result<(), String> {
        let ids = [("package", self.package), ("gate", self.gate), ("mint_cap", self.mint_cap), ("admin_cap", self.admin_cap)];
        for (name, id) in ids {
            if id == [0u8; 32] {
                return Err(format!("{name} id is zero"));
            }
        }
        for i in 0..ids.len() {
            for j in i + 1..ids.len() {
                if ids[i].1 == ids[j].1 {
                    return Err(format!("{} and {} are the same id", ids[i].0, ids[j].0));
                }
            }
        }
        Ok(())
    }

    /// `0x<package>::rumi_icusd_gate::Gate` in the canonical long form.
    pub fn gate_type(&self) -> String {
        format!("0x{}::{MODULE}::{GATE_STRUCT}", hex::encode(self.package))
    }

    /// `0x<package>::rumi_icusd_gate::<name>`: the fully qualified type of one of this package's own
    /// non-generic structs (an event type filter, an object type).
    pub fn struct_type(&self, name: &str) -> String {
        format!("0x{}::{MODULE}::{name}", hex::encode(self.package))
    }

    /// `0x<package>::rumi_icusd_gate::RUMI_ICUSD_GATE`, the currency's coin type.
    pub fn coin_type(&self) -> String {
        format!("0x{}::{MODULE}::{COIN_STRUCT}", hex::encode(self.package))
    }

    /// Whether a provider's `repr` is exactly this package's non-generic `struct_name`,
    /// accepting a short or long address form for the package.
    pub fn type_repr_matches(&self, repr: &str, struct_name: &str) -> bool {
        if repr.contains('<') {
            return false;
        }
        let parts: Vec<&str> = repr.split("::").collect();
        if parts.len() != 3 {
            return false;
        }
        match parse_address_padded(parts[0]) {
            Ok(package) => package == self.package && parts[1] == MODULE && parts[2] == struct_name,
            Err(_) => false,
        }
    }
}

/// The decoded shared `Gate` object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateObject {
    pub id: ObjectId,
    pub version: u64,
    pub treasury_cap_id: ObjectId,
    pub total_supply: u64,
    pub chain_identifier: Vec<u8>,
    pub supply_cap: u64,
    pub min_bridge_out: u64,
    pub minted_total: u128,
    pub burned_total: u128,
    pub mint_count: u64,
    pub bridge_out_count: u64,
    pub mints_table: ObjectId,
    pub mints_size: u64,
    pub returns_table: ObjectId,
    pub returns_size: u64,
}

/// Decode the BCS `contents` of a `Gate` object. Refuses trailing bytes.
pub fn decode_gate(bytes: &[u8]) -> Result<GateObject, GateError> {
    let mut r = Reader::new(bytes);
    let gate = GateObject {
        id: r.raw::<32>()?,
        version: r.u64()?,
        treasury_cap_id: r.raw::<32>()?,
        total_supply: r.u64()?,
        chain_identifier: r.bytes(64)?,
        supply_cap: r.u64()?,
        min_bridge_out: r.u64()?,
        minted_total: r.u128()?,
        burned_total: r.u128()?,
        mint_count: r.u64()?,
        bridge_out_count: r.u64()?,
        mints_table: r.raw::<32>()?,
        mints_size: r.u64()?,
        returns_table: r.raw::<32>()?,
        returns_size: r.u64()?,
    };
    r.finish()?;
    Ok(gate)
}

impl GateObject {
    /// The bind check against the configured identities and the Move invariants. Any failure is
    /// a mismatch the caller must treat as a hold, never as zero or success.
    ///
    /// * the object id is the configured gate and the code version is the one this build knows;
    /// * the chain identifier stored in the gate is the configured network's (32 bytes);
    /// * `supply_cap <= MAX_SUPPLY_CAP` and `min_bridge_out >= 1` (Move `set_limits` bounds);
    /// * `minted_total - burned_total == total_supply` (the supply identity), without underflow;
    /// * the table sizes equal the counters (`mints` grows once per mint, `returns` once per
    ///   return; entries are never removed).
    pub fn check_against(&self, cfg: &ConfiguredGate, chain_identifier: &Bytes32) -> Result<(), GateError> {
        if self.id != cfg.gate {
            return mismatch("the gate object id is not the configured gate");
        }
        if self.version != GATE_VERSION {
            return mismatch(format!("gate version {} is not the supported version {GATE_VERSION}", self.version));
        }
        if self.chain_identifier.as_slice() != chain_identifier.as_slice() {
            return mismatch("the gate was created for a different chain identifier");
        }
        if self.supply_cap > MAX_SUPPLY_CAP {
            return mismatch("supply_cap is above the Move maximum");
        }
        if self.min_bridge_out == 0 {
            return mismatch("min_bridge_out is zero");
        }
        match self.minted_total.checked_sub(self.burned_total) {
            Some(outstanding) if outstanding == u128::from(self.total_supply) => {}
            _ => return mismatch("minted_total - burned_total does not equal the treasury supply"),
        }
        if self.mints_size != self.mint_count {
            return mismatch("the mints table size differs from mint_count");
        }
        if self.returns_size != self.bridge_out_count {
            return mismatch("the returns table size differs from bridge_out_count");
        }
        if self.mints_table == self.returns_table || self.mints_table == self.id || self.returns_table == self.id {
            return mismatch("the table ids are not distinct");
        }
        Ok(())
    }
}

/// `mints[mint_id]`: who got what, as mint number `sequence`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintRecord {
    pub recipient: Bytes32,
    pub amount: u64,
    pub sequence: u64,
}

pub fn decode_mint_record(bytes: &[u8]) -> Result<MintRecord, GateError> {
    let mut r = Reader::new(bytes);
    let rec = MintRecord { recipient: r.raw::<32>()?, amount: r.u64()?, sequence: r.u64()? };
    r.finish()?;
    if rec.recipient == [0u8; 32] {
        return mismatch("a mint record with the zero recipient cannot exist");
    }
    if rec.amount == 0 || rec.sequence == 0 {
        return mismatch("a mint record has a zero amount or sequence");
    }
    Ok(rec)
}

/// `returns[transfer_id]`: the burn, with the ICP destination saved on chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReturnRecord {
    pub sender: Bytes32,
    pub amount: u64,
    pub sequence: u64,
    pub icp_principal: Vec<u8>,
    pub icp_subaccount: Vec<u8>,
    pub client_nonce: u64,
}

pub fn decode_return_record(bytes: &[u8]) -> Result<ReturnRecord, GateError> {
    let mut r = Reader::new(bytes);
    let rec = ReturnRecord {
        sender: r.raw::<32>()?,
        amount: r.u64()?,
        sequence: r.u64()?,
        icp_principal: r.bytes(64)?,
        icp_subaccount: r.bytes(64)?,
        client_nonce: r.u64()?,
    };
    r.finish()?;
    // The Move `bridge_out` checks, re-applied so a record the gate could not have written is
    // never believed.
    if rec.amount == 0 || rec.sequence == 0 {
        return mismatch("a return record has a zero amount or sequence");
    }
    if rec.icp_principal.is_empty() || rec.icp_principal.len() > MAX_PRINCIPAL_LEN || rec.icp_principal == [0x04] {
        return mismatch("a return record has an invalid ICP principal");
    }
    if rec.icp_subaccount.len() != SUBACCOUNT_LEN {
        return mismatch("a return record has an invalid ICP subaccount length");
    }
    Ok(rec)
}

/// The `BridgeOut` event (`emit` in `bridge_out`). It can DISCOVER a return; it is never proof of one: the
/// durable `returns` record is. Layout (Move struct order):
/// `transfer_id: vector<u8>, sender: address, amount: u64, sequence: u64, icp_principal: vector<u8>,
/// icp_subaccount: vector<u8>, client_nonce: u64, burned_total: u128, supply: u64`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeOutEvent {
    pub transfer_id: Vec<u8>,
    pub sender: Bytes32,
    pub amount: u64,
    pub sequence: u64,
    pub icp_principal: Vec<u8>,
    pub icp_subaccount: Vec<u8>,
    pub client_nonce: u64,
    pub burned_total: u128,
    pub supply: u64,
}

pub fn decode_bridge_out_event(bytes: &[u8]) -> Result<BridgeOutEvent, GateError> {
    let mut r = Reader::new(bytes);
    let e = BridgeOutEvent {
        transfer_id: r.bytes(64)?,
        sender: r.raw::<32>()?,
        amount: r.u64()?,
        sequence: r.u64()?,
        icp_principal: r.bytes(64)?,
        icp_subaccount: r.bytes(64)?,
        client_nonce: r.u64()?,
        burned_total: r.u128()?,
        supply: r.u64()?,
    };
    r.finish()?;
    if e.transfer_id.len() != MINT_ID_LEN || e.amount == 0 || e.sequence == 0 {
        return mismatch("a BridgeOut event has a malformed transfer id, a zero amount or a zero sequence");
    }
    Ok(e)
}

/// The `Minted` event: `mint_id: vector<u8>, recipient: address, amount: u64, sequence: u64,
/// minted_total: u128, supply: u64`. Discovery only, like [`BridgeOutEvent`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintedEvent {
    pub mint_id: Vec<u8>,
    pub recipient: Bytes32,
    pub amount: u64,
    pub sequence: u64,
    pub minted_total: u128,
    pub supply: u64,
}

pub fn decode_minted_event(bytes: &[u8]) -> Result<MintedEvent, GateError> {
    let mut r = Reader::new(bytes);
    let e = MintedEvent {
        mint_id: r.bytes(64)?,
        recipient: r.raw::<32>()?,
        amount: r.u64()?,
        sequence: r.u64()?,
        minted_total: r.u128()?,
        supply: r.u64()?,
    };
    r.finish()?;
    if e.mint_id.len() != MINT_ID_LEN || e.amount == 0 || e.sequence == 0 {
        return mismatch("a Minted event has a malformed mint id, a zero amount or a zero sequence");
    }
    Ok(e)
}

/// The BCS of a table key (`vector<u8>`): the value the GraphQL `DynamicFieldName.bcs` carries.
pub fn table_key_bcs(key: &Bytes32) -> Vec<u8> {
    let mut w = crate::bcs::Writer::new();
    w.bytes(key);
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vectors::*;

    fn cfg() -> ConfiguredGate {
        configured()
    }

    #[test]
    fn the_event_decoders_follow_the_move_struct_order_and_the_gates_own_bounds() {
        let mut w = crate::bcs::Writer::new();
        w.bytes(&[0xab; 32]).raw(&[0x44; 32]).u64(7 * 100_000_000).u64(3).bytes(&[1, 2, 3]).bytes(&[0u8; 32]).u64(99).u128(10).u64(5);
        let e = decode_bridge_out_event(&w.into_bytes()).unwrap();
        assert_eq!((e.transfer_id.clone(), e.sender, e.amount, e.sequence, e.client_nonce, e.burned_total, e.supply), (vec![0xab; 32], [0x44; 32], 700_000_000, 3, 99, 10, 5));
        assert_eq!((e.icp_principal, e.icp_subaccount), (vec![1, 2, 3], vec![0u8; 32]));
        let bad = |f: &dyn Fn(&mut crate::bcs::Writer)| {
            let mut w = crate::bcs::Writer::new();
            f(&mut w);
            decode_bridge_out_event(&w.into_bytes()).is_err()
        };
        assert!(bad(&|w| {
            w.bytes(&[0xab; 31]).raw(&[0x44; 32]).u64(1).u64(1).bytes(&[1]).bytes(&[0u8; 32]).u64(0).u128(0).u64(0);
        }), "a 31-byte transfer id");
        assert!(bad(&|w| {
            w.bytes(&[0xab; 32]).raw(&[0x44; 32]).u64(0).u64(1).bytes(&[1]).bytes(&[0u8; 32]).u64(0).u128(0).u64(0);
        }), "a zero amount");
        assert!(bad(&|w| {
            w.bytes(&[0xab; 32]).raw(&[0x44; 32]).u64(1).u64(0).bytes(&[1]).bytes(&[0u8; 32]).u64(0).u128(0).u64(0);
        }), "a zero sequence");
        assert!(bad(&|w| {
            w.bytes(&[0xab; 32]).raw(&[0x44; 32]).u64(1).u64(1).bytes(&[1]);
        }), "truncated");
        let mut t = crate::bcs::Writer::new();
        t.bytes(&[0xab; 32]).raw(&[0x44; 32]).u64(1).u64(1).bytes(&[1]).bytes(&[0u8; 32]).u64(0).u128(0).u64(0).u8(9);
        assert!(decode_bridge_out_event(&t.into_bytes()).is_err(), "trailing bytes");
        let mut m = crate::bcs::Writer::new();
        m.bytes(&[0xcd; 32]).raw(&[0x55; 32]).u64(8).u64(2).u128(100).u64(40);
        let me = decode_minted_event(&m.into_bytes()).unwrap();
        assert_eq!((me.mint_id, me.recipient, me.amount, me.sequence, me.minted_total, me.supply), (vec![0xcd; 32], [0x55; 32], 8, 2, 100, 40));
        let mut z = crate::bcs::Writer::new();
        z.bytes(&[0xcd; 32]).raw(&[0x55; 32]).u64(0).u64(2).u128(100).u64(40);
        assert!(decode_minted_event(&z.into_bytes()).is_err());
        assert_eq!(cfg().struct_type("BridgeOut"), format!("0x{}::rumi_icusd_gate::BridgeOut", "70".repeat(32)));
    }

    #[test]
    fn decodes_the_reference_gate() {
        let g = decode_gate(&hex::decode(GATE_BCS).unwrap()).unwrap();
        assert_eq!(g.id, [0x6a; 32]);
        assert_eq!(g.version, 1);
        assert_eq!(g.treasury_cap_id, [0xdd; 32]);
        assert_eq!(g.total_supply, 400 * 100_000_000);
        assert_eq!(g.chain_identifier, crate::ids::TESTNET_CHAIN_IDENTIFIER.to_vec());
        assert_eq!(g.supply_cap, 1000 * 100_000_000);
        assert_eq!(g.min_bridge_out, 1);
        assert_eq!(g.minted_total, 650 * 100_000_000);
        assert_eq!(g.burned_total, 250 * 100_000_000);
        assert_eq!((g.mint_count, g.bridge_out_count), (4, 3));
        assert_eq!((g.mints_table, g.mints_size), ([0xe1; 32], 4));
        assert_eq!((g.returns_table, g.returns_size), ([0xe2; 32], 3));
        g.check_against(&cfg(), &crate::ids::TESTNET_CHAIN_IDENTIFIER).unwrap();
    }

    #[test]
    fn decoding_is_exact() {
        let bytes = hex::decode(GATE_BCS).unwrap();
        for cut in [0, 1, 40, bytes.len() - 1] {
            assert!(decode_gate(&bytes[..cut]).is_err(), "truncated at {cut}");
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert_eq!(decode_gate(&extra), Err(GateError::Decode(BcsError::Trailing(1))));
    }

    #[test]
    fn the_bind_check_rejects_every_wrong_identity_and_invariant() {
        let good = decode_gate(&hex::decode(GATE_BCS).unwrap()).unwrap();
        let check = |g: &GateObject| g.check_against(&cfg(), &crate::ids::TESTNET_CHAIN_IDENTIFIER);
        assert!(check(&good).is_ok());

        let mut g = good.clone();
        g.id = [0x6b; 32];
        assert!(check(&g).is_err(), "another gate object");

        // The reference gate at version 2 decodes but is refused by this build.
        let v2 = decode_gate(&hex::decode(GATE_BCS_VERSION_2).unwrap()).unwrap();
        assert!(matches!(check(&v2), Err(GateError::Mismatch(m)) if m.contains("version")));

        assert!(good.check_against(&cfg(), &crate::ids::MAINNET_CHAIN_IDENTIFIER).is_err(), "wrong network");

        let mut g = good.clone();
        g.supply_cap = u64::MAX;
        assert!(check(&g).is_err());
        let mut g = good.clone();
        g.min_bridge_out = 0;
        assert!(check(&g).is_err());
        let mut g = good.clone();
        g.burned_total += 1;
        assert!(check(&g).is_err(), "supply identity");
        let mut g = good.clone();
        g.burned_total = g.minted_total + 1;
        assert!(check(&g).is_err(), "burned above minted never underflows into success");
        let mut g = good.clone();
        g.mints_size -= 1;
        assert!(check(&g).is_err());
        let mut g = good.clone();
        g.returns_size += 1;
        assert!(check(&g).is_err());
        let mut g = good.clone();
        g.returns_table = g.mints_table;
        assert!(check(&g).is_err());
    }

    #[test]
    fn records_decode_and_enforce_the_move_checks() {
        let m = decode_mint_record(&hex::decode(MINT_RECORD_BCS).unwrap()).unwrap();
        assert_eq!(m, MintRecord { recipient: [0x8b; 32], amount: 250 * 100_000_000, sequence: 4 });
        let r = decode_return_record(&hex::decode(RETURN_RECORD_BCS).unwrap()).unwrap();
        assert_eq!(r.sender, [0xbb; 32]);
        assert_eq!((r.amount, r.sequence, r.client_nonce), (250 * 100_000_000, 1, 42));
        assert_eq!(r.icp_principal, (1..=29).collect::<Vec<u8>>());
        assert_eq!(r.icp_subaccount, (100..=131).collect::<Vec<u8>>());

        let mut bad = hex::decode(MINT_RECORD_BCS).unwrap();
        bad[..32].fill(0);
        assert!(decode_mint_record(&bad).is_err(), "zero recipient");
        let mut w = crate::bcs::Writer::new();
        w.raw(&[1; 32]).u64(5).u64(1).bytes(&[0x04]).bytes(&[0; 32]).u64(0);
        assert!(decode_return_record(&w.into_bytes()).is_err(), "anonymous principal");
        let mut w = crate::bcs::Writer::new();
        w.raw(&[1; 32]).u64(5).u64(1).bytes(&[1, 2]).bytes(&[0; 31]).u64(0);
        assert!(decode_return_record(&w.into_bytes()).is_err(), "short subaccount");
    }

    #[test]
    fn type_names_match_exactly_and_ignore_the_address_form() {
        let c = cfg();
        assert_eq!(c.gate_type(), format!("0x{}::rumi_icusd_gate::Gate", "70".repeat(32)));
        assert_eq!(c.coin_type(), format!("0x{}::rumi_icusd_gate::RUMI_ICUSD_GATE", "70".repeat(32)));
        assert!(c.type_repr_matches(&c.gate_type(), GATE_STRUCT));
        assert!(!c.type_repr_matches(&c.gate_type(), COIN_STRUCT));
        assert!(!c.type_repr_matches(&format!("0x{}::rumi_icusd_gate::Gate", "71".repeat(32)), GATE_STRUCT), "another package");
        assert!(!c.type_repr_matches(&format!("0x{}::other::Gate", "70".repeat(32)), GATE_STRUCT));
        assert!(!c.type_repr_matches(&format!("0x{}::rumi_icusd_gate::Gate<0x2::sui::SUI>", "70".repeat(32)), GATE_STRUCT));
        assert!(!c.type_repr_matches("0x70::rumi_icusd_gate::Gate", GATE_STRUCT), "a short form is a different address");
        let short = ConfiguredGate { package: { let mut p = [0u8; 32]; p[31] = 0x42; p }, ..cfg() };
        assert!(short.type_repr_matches("0x42::rumi_icusd_gate::Gate", GATE_STRUCT));
    }

    #[test]
    fn configured_ids_are_validated() {
        assert!(cfg().validate().is_ok());
        assert!(ConfiguredGate { gate: [0; 32], ..cfg() }.validate().is_err());
        assert!(ConfiguredGate { mint_cap: cfg().admin_cap, ..cfg() }.validate().is_err());
        assert!(ConfiguredGate { gate: cfg().package, ..cfg() }.validate().is_err());
    }

    #[test]
    fn table_keys_are_length_prefixed() {
        let k = table_key_bcs(&[0x80; 32]);
        assert_eq!(k.len(), 33);
        assert_eq!(k[0], 32);
    }
}
