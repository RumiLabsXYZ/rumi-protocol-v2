//! The BCS `TransactionData` builder for the one transaction this canister signs on Sui, a
//! sponsored `rumi_icusd_gate::mint` call, with BOTH digests, the serialized wire signatures and
//! a strict decoder used as the shape guard before any signature is produced.
//!
//! # Sources (current pinned official, 2026-10-31)
//!
//! * **Layout.** `MystenLabs/sui` at `mainnet-v1.82.3`, `crates/sui-types/src/transaction.rs`:
//!   `enum TransactionData { V1(TransactionDataV1) }`, `TransactionDataV1 { kind, sender,
//!   gas_data, expiration }`, `TransactionKind::ProgrammableTransaction` is variant 0,
//!   `ProgrammableTransaction { inputs: Vec<CallArg>, commands: Vec<Command> }`,
//!   `CallArg { Pure(Vec<u8>) = 0, Object(ObjectArg) = 1, FundsWithdrawal = 2 }`,
//!   `ObjectArg { ImmOrOwnedObject(ObjectRef) = 0, SharedObject { id, initial_shared_version,
//!   mutability } = 1, Receiving = 2 }`, `SharedObjectMutability { Immutable = 0, Mutable = 1,
//!   NonExclusiveWrite = 2 }`, `Command::MoveCall(Box<ProgrammableMoveCall>)` is variant 0 and
//!   `ProgrammableMoveCall { package, module, function, type_arguments, arguments }`,
//!   `Argument { GasCoin = 0, Input(u16) = 1, Result(u16) = 2, NestedResult(u16, u16) = 3 }`,
//!   `GasData { payment: Vec<ObjectRef>, owner, price, budget }`,
//!   `TransactionExpiration { None = 0, Epoch(u64) = 1, ValidDuring = 2, Validity = 3 }`.
//!   The official TypeScript SDK (`MystenLabs/ts-sdks`, `packages/sui/src/bcs/bcs.ts`) declares
//!   the same orders. An `ObjectRef` is `(ObjectID, SequenceNumber, ObjectDigest)` and the digest
//!   is length-prefixed in BCS, 33 bytes in all (`sui-sdk-types` 0.3.2 `digest.rs`).
//! * **Two different hashes.** `sui-sdk-types` 0.3.2 `hash.rs`: `Transaction::signing_digest` is
//!   BLAKE2b-256 over the intent bytes `[scope=TransactionData(0), version=V0(0), app=Sui(0)]`
//!   followed by the BCS; `Transaction::digest` (the id GraphQL `transaction(digest:)` takes) is
//!   BLAKE2b-256 over the salt `TransactionData::` followed by the BCS. `sui-types` `crypto.rs`
//!   says the same in `Signature::new_secure` ("different from the transaction digest itself that
//!   computes the BCS hash of the Rust type prefix and `struct TransactionData`"). Ed25519 signs
//!   the 32-byte SIGNING digest, never the typed digest.
//! * **Wire signature.** `flag(0x00 Ed25519) || signature(64) || public_key(32)`, base64
//!   (`sui-types` `crypto.rs`, the `SuiSignature` byte layout).
//!
//! Every byte here is also produced by the independent Python reference
//! (`reference/sui_golden_reference.py`) and compared in the tests.

use crate::bcs::{BcsError, Reader, Writer};
use crate::gate::{ConfiguredGate, MINT_FUNCTION, MINT_ID_LEN, MODULE};
use crate::hash::blake2b256_parts;
use crate::sui::{address_from_ed25519, format_digest_base58, Bytes32, ObjectRef, SuiAddress, ED25519_FLAG};
use base64::{engine::general_purpose::STANDARD, Engine as _};

const TX_DATA_V1: u32 = 0;
const KIND_PROGRAMMABLE_TRANSACTION: u32 = 0;
const CALL_ARG_PURE: u32 = 0;
const CALL_ARG_OBJECT: u32 = 1;
const OBJECT_ARG_IMM_OR_OWNED: u32 = 0;
const OBJECT_ARG_SHARED: u32 = 1;
const MUTABILITY_MUTABLE: u32 = 1;
const COMMAND_MOVE_CALL: u32 = 0;
const ARGUMENT_INPUT: u32 = 1;
const EXPIRATION_EPOCH: u32 = 1;
/// `Intent { scope: TransactionData, version: V0, app_id: Sui }`.
pub const INTENT_PREFIX: [u8; 3] = [0, 0, 0];
/// The type-name salt of the typed transaction digest.
pub const TYPED_DIGEST_SALT: &[u8] = b"TransactionData::";
/// Gas budget bounds in MIST (chain-sui.md section 3: 2,000 MIST to 50 SUI).
pub const MIN_GAS_BUDGET: u64 = 2_000;
pub const MAX_GAS_BUDGET: u64 = 50_000_000_000;
pub const WIRE_SIGNATURE_LEN: usize = 1 + 64 + 32;
/// The mint call's `Input` count: gate, mint cap, mint id, recipient, amount.
const MINT_INPUTS: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxError {
    /// The configured gate or the role addresses are not usable.
    Config(String),
    /// A request field breaks a rule (identity, amount, recipient, gas, epoch).
    Request(String),
    /// Bytes that are not exactly one mint transaction of this shape.
    Shape(String),
    /// A signature or key does not belong to the role it was offered for.
    Signature(String),
}

impl core::fmt::Display for TxError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl From<BcsError> for TxError {
    fn from(e: BcsError) -> Self {
        TxError::Shape(format!("{e}"))
    }
}

fn request<T>(m: impl Into<String>) -> Result<T, TxError> {
    Err(TxError::Request(m.into()))
}

fn shape<T>(m: impl Into<String>) -> Result<T, TxError> {
    Err(TxError::Shape(m.into()))
}

/// The three role addresses. Distinct and non-zero by construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoleAddresses {
    pub treasury: SuiAddress,
    pub gas: SuiAddress,
    pub upgrade_custody: SuiAddress,
}

impl RoleAddresses {
    pub fn validate(&self) -> Result<(), TxError> {
        let all = [("treasury", self.treasury), ("gas", self.gas), ("upgrade-custody", self.upgrade_custody)];
        for (name, a) in all {
            if a == [0u8; 32] {
                return Err(TxError::Config(format!("the {name} address is zero")));
            }
        }
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                if all[i].1 == all[j].1 {
                    return Err(TxError::Config(format!("the {} and {} roles share an address", all[i].0, all[j].0)));
                }
            }
        }
        Ok(())
    }
}

/// What one mint attempt needs beyond the configured identities. `gate_initial_shared_version`,
/// `mint_cap` and `gas_payment` are OBSERVED facts (their versions and digests change); the
/// builder only checks that they refer to the configured objects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MintRequest {
    pub gate_initial_shared_version: u64,
    pub mint_cap: ObjectRef,
    pub mint_id: Bytes32,
    pub recipient: SuiAddress,
    pub amount: u64,
    pub gas_payment: ObjectRef,
    pub gas_price: u64,
    pub gas_budget: u64,
    /// The epoch the transaction is bound to: `TransactionExpiration::Epoch(epoch)`. This bounds
    /// when validators will sign it; it is NOT proof of non-execution after the epoch ends.
    pub epoch: u64,
}

/// The unsigned transaction and both of its digests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltMint {
    pub bcs: Vec<u8>,
    pub signing_digest: Bytes32,
    pub transaction_digest: Bytes32,
}

impl BuiltMint {
    pub fn transaction_digest_base58(&self) -> String {
        format_digest_base58(&self.transaction_digest)
    }
}

/// What a decoded mint transaction contains.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedMint {
    pub package: Bytes32,
    pub gate: Bytes32,
    pub gate_initial_shared_version: u64,
    pub mint_cap: ObjectRef,
    pub mint_id: Bytes32,
    pub recipient: SuiAddress,
    pub amount: u64,
    pub sender: SuiAddress,
    pub gas_payment: Vec<ObjectRef>,
    pub gas_owner: SuiAddress,
    pub gas_price: u64,
    pub gas_budget: u64,
    pub epoch: u64,
}

pub fn signing_digest(tx_bcs: &[u8]) -> Bytes32 {
    blake2b256_parts(&[&INTENT_PREFIX, tx_bcs])
}

pub fn transaction_digest(tx_bcs: &[u8]) -> Bytes32 {
    blake2b256_parts(&[TYPED_DIGEST_SALT, tx_bcs])
}

fn write_object_ref(w: &mut Writer, r: &ObjectRef) {
    w.raw(&r.id).u64(r.version).bytes(&r.digest);
}

fn read_object_ref(r: &mut Reader) -> Result<ObjectRef, BcsError> {
    let id = r.raw::<32>()?;
    let version = r.u64()?;
    let digest_bytes = r.bytes(32)?;
    let digest = <Bytes32>::try_from(digest_bytes.as_slice()).map_err(|_| BcsError::Eof)?;
    Ok(ObjectRef { id, version, digest })
}

/// Build the sponsored mint transaction for the configured gate.
///
/// The sender is the `treasury` role (it owns the `MintCap`) and the gas owner is the `gas`
/// role. Refuses anything that is not the configured package, gate or mint cap, a zero or
/// gate-bound recipient, a zero amount, and gas or epoch values outside the sourced bounds.
pub fn build_mint_transaction(cfg: &ConfiguredGate, roles: &RoleAddresses, req: &MintRequest) -> Result<BuiltMint, TxError> {
    cfg.validate().map_err(TxError::Config)?;
    roles.validate()?;
    if req.mint_cap.id != cfg.mint_cap {
        return request("the mint cap is not the configured mint cap");
    }
    if req.gas_payment.id == cfg.mint_cap || req.gas_payment.id == cfg.gate || req.gas_payment.id == cfg.admin_cap {
        return request("the gas coin cannot be a gate capability or the gate");
    }
    if req.gate_initial_shared_version == 0 {
        return request("the gate's initial shared version is zero");
    }
    if req.amount == 0 {
        return request("the mint amount is zero");
    }
    if req.recipient == [0u8; 32] || req.recipient == cfg.gate {
        return request("the recipient is the zero address or the gate");
    }
    if req.gas_price == 0 {
        return request("the gas price is zero");
    }
    if !(MIN_GAS_BUDGET..=MAX_GAS_BUDGET).contains(&req.gas_budget) {
        return request("the gas budget is outside 2,000 MIST to 50 SUI");
    }
    if req.epoch == 0 {
        return request("the expiration epoch is zero");
    }

    let mut w = Writer::new();
    w.uleb(TX_DATA_V1);
    // TransactionKind::ProgrammableTransaction
    w.uleb(KIND_PROGRAMMABLE_TRANSACTION);
    // inputs: [gate (shared, mutable), mint cap (owned), mint id, recipient, amount]
    w.uleb(MINT_INPUTS as u32);
    w.uleb(CALL_ARG_OBJECT).uleb(OBJECT_ARG_SHARED).raw(&cfg.gate).u64(req.gate_initial_shared_version).uleb(MUTABILITY_MUTABLE);
    w.uleb(CALL_ARG_OBJECT).uleb(OBJECT_ARG_IMM_OR_OWNED);
    write_object_ref(&mut w, &req.mint_cap);
    // Pure arguments carry the BCS of the Move argument as a length-prefixed byte string.
    let mut mint_id_arg = Writer::new();
    mint_id_arg.bytes(&req.mint_id); // Move `vector<u8>`
    w.uleb(CALL_ARG_PURE).bytes(&mint_id_arg.into_bytes());
    w.uleb(CALL_ARG_PURE).bytes(&req.recipient); // Move `address`
    w.uleb(CALL_ARG_PURE).bytes(&req.amount.to_le_bytes()); // Move `u64`
    // commands: [MoveCall]
    w.uleb(1);
    w.uleb(COMMAND_MOVE_CALL).raw(&cfg.package).string(MODULE).string(MINT_FUNCTION);
    w.uleb(0); // no type arguments
    w.uleb(MINT_INPUTS as u32);
    for i in 0..MINT_INPUTS as u16 {
        w.uleb(ARGUMENT_INPUT).u16(i);
    }
    // sender, gas data, expiration
    w.raw(&roles.treasury);
    w.uleb(1);
    write_object_ref(&mut w, &req.gas_payment);
    w.raw(&roles.gas).u64(req.gas_price).u64(req.gas_budget);
    w.uleb(EXPIRATION_EPOCH).u64(req.epoch);

    let bcs = w.into_bytes();
    // The decoder is the shape guard: what we built must decode to exactly what was asked.
    let parsed = check_mint_transaction(&bcs, cfg, roles)?;
    let expected = ParsedMint {
        package: cfg.package,
        gate: cfg.gate,
        gate_initial_shared_version: req.gate_initial_shared_version,
        mint_cap: req.mint_cap,
        mint_id: req.mint_id,
        recipient: req.recipient,
        amount: req.amount,
        sender: roles.treasury,
        gas_payment: vec![req.gas_payment],
        gas_owner: roles.gas,
        gas_price: req.gas_price,
        gas_budget: req.gas_budget,
        epoch: req.epoch,
    };
    if parsed != expected {
        return shape("the built transaction does not decode to the request (builder bug)");
    }
    Ok(BuiltMint { signing_digest: signing_digest(&bcs), transaction_digest: transaction_digest(&bcs), bcs })
}

/// Strictly decode a mint transaction. Accepts exactly the shape [`build_mint_transaction`]
/// writes and nothing else: one programmable transaction, the five inputs in order, one
/// `MoveCall` with no type arguments and the five `Input` arguments, one gas coin, an `Epoch`
/// expiration, and no trailing bytes.
pub fn parse_mint_transaction(bytes: &[u8]) -> Result<ParsedMint, TxError> {
    let mut r = Reader::new(bytes);
    if r.uleb()? != TX_DATA_V1 {
        return shape("not TransactionData::V1");
    }
    if r.uleb()? != KIND_PROGRAMMABLE_TRANSACTION {
        return shape("not a programmable transaction");
    }
    if r.uleb()? as usize != MINT_INPUTS {
        return shape("the mint transaction has five inputs");
    }
    // input 0: the shared gate, mutable
    if r.uleb()? != CALL_ARG_OBJECT || r.uleb()? != OBJECT_ARG_SHARED {
        return shape("input 0 is not a shared object");
    }
    let gate = r.raw::<32>()?;
    let gate_initial_shared_version = r.u64()?;
    if r.uleb()? != MUTABILITY_MUTABLE {
        return shape("the gate must be taken mutably");
    }
    // input 1: the owned mint cap
    if r.uleb()? != CALL_ARG_OBJECT || r.uleb()? != OBJECT_ARG_IMM_OR_OWNED {
        return shape("input 1 is not an owned object");
    }
    let mint_cap = read_object_ref(&mut r)?;
    // inputs 2 to 4: pure arguments
    if r.uleb()? != CALL_ARG_PURE {
        return shape("input 2 is not a pure value");
    }
    let mint_id_arg = r.bytes(64)?;
    let mut ir = Reader::new(&mint_id_arg);
    let mint_id_vec = ir.bytes(MINT_ID_LEN)?;
    ir.finish()?;
    let mint_id = <Bytes32>::try_from(mint_id_vec.as_slice()).map_err(|_| TxError::Shape("the mint id is not 32 bytes".into()))?;
    if r.uleb()? != CALL_ARG_PURE {
        return shape("input 3 is not a pure value");
    }
    let recipient_arg = r.bytes(32)?;
    let recipient = <SuiAddress>::try_from(recipient_arg.as_slice()).map_err(|_| TxError::Shape("the recipient is not 32 bytes".into()))?;
    if r.uleb()? != CALL_ARG_PURE {
        return shape("input 4 is not a pure value");
    }
    let amount_arg = r.bytes(8)?;
    let amount = u64::from_le_bytes(<[u8; 8]>::try_from(amount_arg.as_slice()).map_err(|_| TxError::Shape("the amount is not a u64".into()))?);
    // commands
    if r.uleb()? != 1 {
        return shape("the mint transaction has exactly one command");
    }
    if r.uleb()? != COMMAND_MOVE_CALL {
        return shape("the command is not a Move call");
    }
    let package = r.raw::<32>()?;
    if r.string(64)? != MODULE || r.string(64)? != MINT_FUNCTION {
        return shape("the call is not rumi_icusd_gate::mint");
    }
    if r.uleb()? != 0 {
        return shape("the mint call takes no type arguments");
    }
    if r.uleb()? as usize != MINT_INPUTS {
        return shape("the mint call takes five arguments");
    }
    for i in 0..MINT_INPUTS as u16 {
        if r.uleb()? != ARGUMENT_INPUT || r.u16()? != i {
            return shape("the call arguments are not Input(0) to Input(4) in order");
        }
    }
    let sender = r.raw::<32>()?;
    let payments = r.uleb()? as usize;
    if payments != 1 {
        return shape("the mint transaction pays gas with exactly one coin");
    }
    let gas_payment = vec![read_object_ref(&mut r)?];
    let gas_owner = r.raw::<32>()?;
    let gas_price = r.u64()?;
    let gas_budget = r.u64()?;
    if r.uleb()? != EXPIRATION_EPOCH {
        return shape("the expiration is not an epoch");
    }
    let epoch = r.u64()?;
    r.finish()?;
    Ok(ParsedMint {
        package,
        gate,
        gate_initial_shared_version,
        mint_cap,
        mint_id,
        recipient,
        amount,
        sender,
        gas_payment,
        gas_owner,
        gas_price,
        gas_budget,
        epoch,
    })
}

/// The shape guard to run on a stored attempt before EVERY signature (including a re-sign of
/// the same bytes): the bytes decode as a mint, and name exactly the configured package, gate
/// and mint cap, with the treasury as sender and the gas role as gas owner.
pub fn check_mint_transaction(bytes: &[u8], cfg: &ConfiguredGate, roles: &RoleAddresses) -> Result<ParsedMint, TxError> {
    let p = parse_mint_transaction(bytes)?;
    if p.package != cfg.package {
        return shape("the call targets a package that is not the configured package");
    }
    if p.gate != cfg.gate {
        return shape("the shared object is not the configured gate");
    }
    if p.mint_cap.id != cfg.mint_cap {
        return shape("the owned object is not the configured mint cap");
    }
    if p.sender != roles.treasury {
        return shape("the sender is not the treasury role");
    }
    if p.gas_owner != roles.gas {
        return shape("the gas owner is not the gas role");
    }
    if p.gas_payment[0].id == cfg.mint_cap || p.gas_payment[0].id == cfg.gate {
        return shape("the gas coin is a gate object");
    }
    if p.amount == 0 || p.recipient == [0u8; 32] || p.recipient == cfg.gate {
        return shape("the amount or recipient is invalid");
    }
    Ok(p)
}

/// `0x00 || signature || public key`.
pub fn wire_signature(signature: &[u8; 64], public_key: &Bytes32) -> [u8; WIRE_SIGNATURE_LEN] {
    let mut out = [0u8; WIRE_SIGNATURE_LEN];
    out[0] = ED25519_FLAG;
    out[1..65].copy_from_slice(signature);
    out[65..].copy_from_slice(public_key);
    out
}

pub fn wire_signature_base64(signature: &[u8; 64], public_key: &Bytes32) -> String {
    STANDARD.encode(wire_signature(signature, public_key))
}

pub fn tx_bytes_base64(bcs: &[u8]) -> String {
    STANDARD.encode(bcs)
}

/// A signed mint, ready for a later broadcast step (not performed here).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedMint {
    pub tx_bytes_base64: String,
    /// Sender (treasury) first, then gas owner. Sui accepts either order.
    pub signatures_base64: [String; 2],
    pub transaction_digest_base58: String,
}

/// Attach the two raw signatures. Each key must be the public key of the role address it signs
/// for (`address_from_ed25519(pk) == role address`); a signature made by another key is refused.
/// This checks the keys, not the signatures: verifying an Ed25519 signature needs a crypto
/// crate this foundation does not ship (listed in the omissions; tests verify with dalek).
pub fn assemble_signed_mint(
    built: &BuiltMint,
    roles: &RoleAddresses,
    treasury: (&[u8; 64], &Bytes32),
    gas: (&[u8; 64], &Bytes32),
) -> Result<SignedMint, TxError> {
    if address_from_ed25519(treasury.1) != roles.treasury {
        return Err(TxError::Signature("the treasury signature's key is not the treasury address".into()));
    }
    if address_from_ed25519(gas.1) != roles.gas {
        return Err(TxError::Signature("the gas signature's key is not the gas address".into()));
    }
    if treasury.0 == gas.0 {
        return Err(TxError::Signature("the two signatures are identical".into()));
    }
    Ok(SignedMint {
        tx_bytes_base64: tx_bytes_base64(&built.bcs),
        signatures_base64: [wire_signature_base64(treasury.0, treasury.1), wire_signature_base64(gas.0, gas.1)],
        transaction_digest_base58: built.transaction_digest_base58(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vectors::*;
    use ed25519_dalek::{Signer as _, SigningKey, Verifier as _, VerifyingKey};

    fn keys() -> (SigningKey, SigningKey, SigningKey) {
        (SigningKey::from_bytes(&SEED_TREASURY), SigningKey::from_bytes(&SEED_GAS), SigningKey::from_bytes(&SEED_CUSTODY))
    }

    pub fn sample_request() -> MintRequest {
        MintRequest {
            gate_initial_shared_version: GATE_INITIAL_SHARED_VERSION,
            mint_cap: ObjectRef { id: MINT_CAP_ID, version: MINT_CAP_VERSION, digest: MINT_CAP_DIGEST },
            mint_id: mint_id_bytes(),
            recipient: RECIPIENT,
            amount: AMOUNT,
            gas_payment: ObjectRef { id: GAS_COIN_ID, version: GAS_COIN_VERSION, digest: GAS_COIN_DIGEST },
            gas_price: GAS_PRICE,
            gas_budget: GAS_BUDGET,
            epoch: EPOCH,
        }
    }

    fn built() -> BuiltMint {
        build_mint_transaction(&configured(), &roles(), &sample_request()).expect("build")
    }

    #[test]
    fn test_seeds_give_the_reference_keys_and_addresses() {
        let (t, g, c) = keys();
        assert_eq!(hex::encode(t.verifying_key().to_bytes()), PUB_TREASURY);
        assert_eq!(hex::encode(g.verifying_key().to_bytes()), PUB_GAS);
        assert_eq!(hex::encode(c.verifying_key().to_bytes()), PUB_CUSTODY);
        // Derived here from the seeds, not read from the shared constants.
        let derived = RoleAddresses {
            treasury: address_from_ed25519(&t.verifying_key().to_bytes()),
            gas: address_from_ed25519(&g.verifying_key().to_bytes()),
            upgrade_custody: address_from_ed25519(&c.verifying_key().to_bytes()),
        };
        assert_eq!(hex::encode(derived.treasury), ADDR_TREASURY);
        assert_eq!(hex::encode(derived.gas), ADDR_GAS);
        assert_eq!(hex::encode(derived.upgrade_custody), ADDR_CUSTODY);
        assert_eq!(derived, roles());
        derived.validate().unwrap();
    }

    #[test]
    fn the_transaction_bytes_and_both_digests_match_the_independent_reference() {
        let b = built();
        assert_eq!(hex::encode(&b.bcs), TX_BCS);
        assert_eq!(hex::encode(b.signing_digest), SIGNING_DIGEST);
        assert_eq!(hex::encode(b.transaction_digest), TRANSACTION_DIGEST);
        assert_eq!(b.transaction_digest_base58(), TRANSACTION_DIGEST_B58);
        assert_ne!(b.signing_digest, b.transaction_digest, "the two hashes are different by design");
        assert_eq!(b.signing_digest, signing_digest(&b.bcs));
        assert_eq!(b.transaction_digest, transaction_digest(&b.bcs));
    }

    #[test]
    fn the_digests_use_their_own_prefixes() {
        let bytes = [1u8, 2, 3, 4];
        let intent = crate::hash::blake2b256(&[&INTENT_PREFIX[..], &bytes[..]].concat());
        let typed = crate::hash::blake2b256(&[TYPED_DIGEST_SALT, &bytes[..]].concat());
        assert_eq!(signing_digest(&bytes), intent);
        assert_eq!(transaction_digest(&bytes), typed);
        assert_eq!(INTENT_PREFIX, [0, 0, 0]);
        assert_eq!(TYPED_DIGEST_SALT, b"TransactionData::");
    }

    #[test]
    fn wire_signatures_match_the_reference_and_verify() {
        let (t, g, _) = keys();
        let b = built();
        // Ed25519 is deterministic (RFC 8032), so signing the signing digest reproduces the
        // reference signatures exactly.
        let sig_t = t.sign(&b.signing_digest).to_bytes();
        let sig_g = g.sign(&b.signing_digest).to_bytes();
        assert_eq!(hex::encode(sig_t), SIG_TREASURY);
        assert_eq!(hex::encode(sig_g), SIG_GAS);
        let pk_t = t.verifying_key().to_bytes();
        let pk_g = g.verifying_key().to_bytes();
        assert_eq!(wire_signature_base64(&sig_t, &pk_t), WIRE_TREASURY_B64);
        assert_eq!(wire_signature_base64(&sig_g, &pk_g), WIRE_GAS_B64);
        let wire = wire_signature(&sig_t, &pk_t);
        assert_eq!(wire.len(), 97);
        assert_eq!(wire[0], 0x00);
        assert_eq!(&wire[1..65], &sig_t);
        assert_eq!(&wire[65..], &pk_t);
        // The signature verifies over the signing digest and NOT over the typed digest.
        let vk = VerifyingKey::from_bytes(&pk_t).unwrap();
        let s = ed25519_dalek::Signature::from_bytes(&sig_t);
        assert!(vk.verify(&b.signing_digest, &s).is_ok());
        assert!(vk.verify(&b.transaction_digest, &s).is_err());
    }

    #[test]
    fn assembling_binds_each_key_to_its_role() {
        let (t, g, c) = keys();
        let b = built();
        let r = roles();
        let st = t.sign(&b.signing_digest).to_bytes();
        let sg = g.sign(&b.signing_digest).to_bytes();
        let sc = c.sign(&b.signing_digest).to_bytes();
        let (pt, pg, pc) = (t.verifying_key().to_bytes(), g.verifying_key().to_bytes(), c.verifying_key().to_bytes());
        let signed = assemble_signed_mint(&b, &r, (&st, &pt), (&sg, &pg)).unwrap();
        assert_eq!(signed.signatures_base64[0], WIRE_TREASURY_B64);
        assert_eq!(signed.signatures_base64[1], WIRE_GAS_B64);
        assert_eq!(signed.transaction_digest_base58, TRANSACTION_DIGEST_B58);
        assert_eq!(STANDARD.decode(&signed.tx_bytes_base64).unwrap(), b.bcs);
        // The upgrade-custody key cannot stand in for either role, and roles cannot be swapped.
        assert!(matches!(assemble_signed_mint(&b, &r, (&sc, &pc), (&sg, &pg)), Err(TxError::Signature(_))));
        assert!(matches!(assemble_signed_mint(&b, &r, (&st, &pt), (&sc, &pc)), Err(TxError::Signature(_))));
        assert!(matches!(assemble_signed_mint(&b, &r, (&sg, &pg), (&st, &pt)), Err(TxError::Signature(_))));
        assert!(matches!(assemble_signed_mint(&b, &r, (&st, &pt), (&st, &pg)), Err(TxError::Signature(_))), "identical signatures");
    }

    #[test]
    fn the_builder_rejects_wrong_identities_and_bad_values() {
        let cfg = configured();
        let r = roles();
        let ok = sample_request();
        let build = |m: MintRequest| build_mint_transaction(&cfg, &r, &m);
        assert!(build(ok).is_ok());

        let mut m = ok;
        m.mint_cap.id = [0x01; 32];
        assert!(matches!(build(m), Err(TxError::Request(_))), "a mint cap that is not the configured one");
        let mut m = ok;
        m.mint_cap.id = cfg.admin_cap;
        assert!(build(m).is_err(), "the admin cap is not the mint cap");
        for bad_gas in [cfg.mint_cap, cfg.gate, cfg.admin_cap] {
            let mut m = ok;
            m.gas_payment.id = bad_gas;
            assert!(build(m).is_err());
        }
        let mut m = ok;
        m.amount = 0;
        assert!(build(m).is_err());
        let mut m = ok;
        m.recipient = [0; 32];
        assert!(build(m).is_err());
        let mut m = ok;
        m.recipient = cfg.gate;
        assert!(build(m).is_err());
        let mut m = ok;
        m.gas_price = 0;
        assert!(build(m).is_err());
        for budget in [0, MIN_GAS_BUDGET - 1, MAX_GAS_BUDGET + 1] {
            let mut m = ok;
            m.gas_budget = budget;
            assert!(build(m).is_err(), "budget {budget}");
        }
        for budget in [MIN_GAS_BUDGET, MAX_GAS_BUDGET] {
            let mut m = ok;
            m.gas_budget = budget;
            assert!(build(m).is_ok(), "budget {budget}");
        }
        let mut m = ok;
        m.epoch = 0;
        assert!(build(m).is_err());
        let mut m = ok;
        m.gate_initial_shared_version = 0;
        assert!(build(m).is_err());

        // Role and configuration problems.
        let same = RoleAddresses { gas: r.treasury, ..r };
        assert!(matches!(build_mint_transaction(&cfg, &same, &ok), Err(TxError::Config(_))), "sponsor and sender roles must differ");
        let same = RoleAddresses { upgrade_custody: r.gas, ..r };
        assert!(build_mint_transaction(&cfg, &same, &ok).is_err());
        assert!(build_mint_transaction(&ConfiguredGate { gate: [0; 32], ..cfg }, &r, &ok).is_err());
    }

    #[test]
    fn the_shape_guard_accepts_only_the_exact_mint_for_the_configured_identities() {
        let cfg = configured();
        let r = roles();
        let b = built();
        let p = check_mint_transaction(&b.bcs, &cfg, &r).unwrap();
        assert_eq!(p.amount, AMOUNT);
        assert_eq!(p.mint_id, mint_id_bytes());
        assert_eq!(p.epoch, EPOCH);
        assert_eq!(p.gas_payment.len(), 1);

        // Another package, gate, mint cap, sender or gas owner is refused by the same bytes.
        for other in [
            ConfiguredGate { package: [0x71; 32], ..cfg },
            ConfiguredGate { gate: [0x6b; 32], ..cfg },
            ConfiguredGate { mint_cap: [0x6c; 32], ..cfg },
        ] {
            assert!(check_mint_transaction(&b.bcs, &other, &r).is_err());
        }
        assert!(check_mint_transaction(&b.bcs, &cfg, &RoleAddresses { treasury: [9; 32], ..r }).is_err());
        assert!(check_mint_transaction(&b.bcs, &cfg, &RoleAddresses { gas: [9; 32], ..r }).is_err());

        // Truncation, trailing bytes and single-byte tampering of the structural tags.
        for cut in [0usize, 1, 10, b.bcs.len() - 1] {
            assert!(parse_mint_transaction(&b.bcs[..cut]).is_err(), "cut {cut}");
        }
        let mut longer = b.bcs.clone();
        longer.push(0);
        assert!(parse_mint_transaction(&longer).is_err());
        // Byte 0 is the TransactionData variant, byte 1 the kind, byte 2 the input count.
        for (index, value) in [(0usize, 1u8), (1, 1), (2, 4), (3, 0), (4, 0)] {
            let mut t = b.bcs.clone();
            t[index] = value;
            assert!(parse_mint_transaction(&t).is_err(), "tamper at {index}");
        }
        // Shared object mutability and the expiration tag are structural too.
        let mut t = b.bcs.clone();
        let mutability_at = 5 + 32 + 8; // after the 5 leading bytes, the gate id and its version
        t[mutability_at] = 0;
        assert!(parse_mint_transaction(&t).is_err(), "an immutable gate");
        let mut t = b.bcs.clone();
        let n = t.len();
        t[n - 9] = 0; // expiration None instead of Epoch
        assert!(parse_mint_transaction(&t).is_err());
    }

    #[test]
    fn changing_any_request_field_changes_both_digests() {
        let base = built();
        let r = roles();
        let cfg = configured();
        let mut seen = vec![base.signing_digest];
        let mut variants: Vec<MintRequest> = vec![];
        let mut m = sample_request();
        m.amount += 1;
        variants.push(m);
        let mut m = sample_request();
        m.recipient[0] ^= 1;
        variants.push(m);
        let mut m = sample_request();
        m.mint_id[0] ^= 1;
        variants.push(m);
        let mut m = sample_request();
        m.epoch += 1;
        variants.push(m);
        let mut m = sample_request();
        m.mint_cap.version += 1;
        variants.push(m);
        let mut m = sample_request();
        m.gas_payment.version += 1;
        variants.push(m);
        let mut m = sample_request();
        m.gas_budget += 1;
        variants.push(m);
        for v in variants {
            let b = build_mint_transaction(&cfg, &r, &v).unwrap();
            assert_ne!(b.transaction_digest, base.transaction_digest);
            assert!(!seen.contains(&b.signing_digest));
            seen.push(b.signing_digest);
        }
    }
}
