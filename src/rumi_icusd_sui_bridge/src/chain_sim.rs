//! A simulated Sui chain for the 53b tests (host unit tests only): the configured Gate with its
//! `mints` and `returns` tables, the `MintCap` and a gas coin as owned objects with versions, epochs,
//! checkpoints, executed transactions, and a set of simulated providers that answer the exact GraphQL
//! query shapes of `reads.rs` and `graphql.rs` as JSON. The whole stack from the lane down to the JSON
//! parsers runs against it; nothing here touches a network.
//!
//! What the simulation assumes (stated so a test failure is not mistaken for a Sui fact):
//!
//! * a transaction that executes bumps the version of every owned input (`MintCap`, gas coin), as the
//!   pinned source says for a successful one. Whether an ABORTED one does is not read from source;
//!   [`ChainSim::bump_on_abort`] makes it a switch so the lane is tested both ways;
//! * validators refuse a transaction whose owned inputs are not at their current versions, whose
//!   expiration epoch is below the current epoch, or whose signatures do not verify (checked with
//!   Ed25519 against the two role keys);
//! * an accepted transaction is executed in the next checkpoint (or never, in [`ExecMode::Drop`]);
//! * the Gate's `mint` checks mirror the Move: unconsumed id, non-zero amount, supply cap.
//!
//! Provider behaviors ([`Provider`]): lagging heads, being down, reporting another network, forging a
//! number, answering an exact-version query with the wrong version.

use crate::bcs::Writer;
use crate::gate::ConfiguredGate;
use crate::graphql::{self, ProviderConfig};
use crate::mirror::{CallKind, SuiTransport};
use crate::reads;
use crate::sui::{address_from_ed25519, format_address, format_digest_base58, Bytes32};
use crate::tx;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

pub const SIM_PACKAGE: Bytes32 = [0x70; 32];
pub const SIM_GATE: Bytes32 = [0x6a; 32];
pub const SIM_MINT_CAP: Bytes32 = [0x6d; 32];
pub const SIM_ADMIN_CAP: Bytes32 = [0x6e; 32];
pub const SIM_MINTS_TABLE: Bytes32 = [0x61; 32];
pub const SIM_RETURNS_TABLE: Bytes32 = [0x62; 32];
pub const SIM_TREASURY_CAP: Bytes32 = [0x63; 32];
pub const SIM_GAS_COIN: Bytes32 = [0x42; 32];
pub const SIM_SHARED_VERSION: u64 = 42;
pub const REFERENCE_GAS_PRICE: u64 = 750;

pub fn sim_cfg() -> ConfiguredGate {
    ConfiguredGate { package: SIM_PACKAGE, gate: SIM_GATE, mint_cap: SIM_MINT_CAP, admin_cap: SIM_ADMIN_CAP }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimOwner {
    Shared(u64),
    Address(Bytes32),
}

#[derive(Clone, Debug)]
struct ObjVersion {
    from_cp: u64,
    version: u64,
    digest: Bytes32,
    owner: SimOwner,
    type_repr: String,
    contents: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimMint {
    pub recipient: Bytes32,
    pub amount: u64,
    pub sequence: u64,
    pub from_cp: u64,
}

#[derive(Clone, Debug)]
pub struct SimGate {
    pub version: u64,
    pub supply_cap: u64,
    pub min_bridge_out: u64,
    pub total_supply: u64,
    pub minted_total: u128,
    pub burned_total: u128,
    pub mint_count: u64,
    pub bridge_out_count: u64,
    pub mints: BTreeMap<Bytes32, SimMint>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimStatus {
    Success,
    Failure(String),
}

#[derive(Clone, Debug)]
pub struct SimTx {
    pub status: SimStatus,
    pub checkpoint: u64,
    pub epoch: u64,
}

/// What the network does with a transaction it accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecMode {
    /// Execute it in the next checkpoint (the Move checks decide success or abort).
    Execute,
    /// Execute it and abort with this message, whatever the Move checks say.
    Abort(String),
    /// Accept it and never execute it (an accepted but lost or pending transaction).
    Drop,
}

#[derive(Clone, Debug, Default)]
pub struct Provider {
    pub down: bool,
    pub wrong_network: bool,
    /// The provider's head is this many checkpoints behind the truth.
    pub lag: u64,
    /// Report this total supply in the gate contents instead of the truth.
    pub forge_supply: Option<u64>,
    /// Answer an exact-version object query with version + 1 (right object, wrong version).
    pub wrong_exact_version: bool,
    /// Answer every transaction lookup with null (an indexer that lost it).
    pub no_transactions: bool,
    /// Forward `executeTransaction` to the network but fail the reply (the transaction may have run).
    pub lose_send_reply: bool,
}

pub struct ChainSim {
    pub genesis: Bytes32,
    pub head: Cell<u64>,
    /// `(first checkpoint, epoch)`.
    epochs: RefCell<Vec<(u64, u64)>>,
    objects: RefCell<BTreeMap<Bytes32, Vec<ObjVersion>>>,
    pub gate: RefCell<SimGate>,
    txs: RefCell<BTreeMap<Bytes32, SimTx>>,
    /// Digests that reached the network (accepted), in order.
    pub accepted: RefCell<Vec<Bytes32>>,
    pub mode: RefCell<ExecMode>,
    pub bump_on_abort: Cell<bool>,
    pub treasury_key: Bytes32,
    pub gas_key: Bytes32,
    pub custody_key: Bytes32,
    pub providers: RefCell<BTreeMap<String, Provider>>,
    pub coin_metadata: RefCell<Option<(u8, String)>>,
    /// Number of requests served, by query text prefix (to assert what the lane asked).
    pub served: RefCell<Vec<String>>,
    next_lamport: Cell<u64>,
    pub rejections: RefCell<Vec<String>>,
}

fn digest_of(tag: &str, id: &Bytes32, version: u64) -> Bytes32 {
    let mut h = Sha256::new();
    h.update(tag.as_bytes());
    h.update(id);
    h.update(version.to_be_bytes());
    h.finalize().into()
}

impl ChainSim {
    /// A testnet chain at checkpoint 1,000 in epoch 5 with a pristine gate (`supply_cap` 0), a mint cap
    /// owned by the treasury role and a gas coin owned by the gas role holding 1 SUI.
    pub fn new(treasury_key: Bytes32, gas_key: Bytes32, custody_key: Bytes32) -> ChainSim {
        let sim = ChainSim {
            genesis: crate::ids::TESTNET_CHAIN_IDENTIFIER,
            head: Cell::new(1_000),
            epochs: RefCell::new(vec![(0, 5)]),
            objects: RefCell::new(BTreeMap::new()),
            gate: RefCell::new(SimGate {
                version: 1,
                supply_cap: 0,
                min_bridge_out: 1,
                total_supply: 0,
                minted_total: 0,
                burned_total: 0,
                mint_count: 0,
                bridge_out_count: 0,
                mints: BTreeMap::new(),
            }),
            txs: RefCell::new(BTreeMap::new()),
            accepted: RefCell::new(vec![]),
            mode: RefCell::new(ExecMode::Execute),
            bump_on_abort: Cell::new(true),
            treasury_key,
            gas_key,
            custody_key,
            providers: RefCell::new(BTreeMap::new()),
            coin_metadata: RefCell::new(Some((8, "icUSD".to_string()))),
            served: RefCell::new(vec![]),
            next_lamport: Cell::new(100),
            rejections: RefCell::new(vec![]),
        };
        let cfg = sim_cfg();
        let treasury = address_from_ed25519(&treasury_key);
        let gas = address_from_ed25519(&gas_key);
        let cap = |id: Bytes32| [id.as_slice(), SIM_GATE.as_slice()].concat();
        sim.put_object(SIM_MINT_CAP, SimOwner::Address(treasury), format!("{}::rumi_icusd_gate::MintCap", pkg()), cap(SIM_MINT_CAP), 0);
        sim.put_object(SIM_ADMIN_CAP, SimOwner::Address(address_from_ed25519(&custody_key)), format!("{}::rumi_icusd_gate::AdminCap", pkg()), cap(SIM_ADMIN_CAP), 0);
        sim.put_object(SIM_GAS_COIN, SimOwner::Address(gas), "0x2::coin::Coin<0x2::sui::SUI>".to_string(), coin_contents(&SIM_GAS_COIN, 1_000_000_000), 0);
        sim.sync_gate(0);
        let _ = cfg;
        sim
    }

    fn lamport(&self) -> u64 {
        let v = self.next_lamport.get() + 1;
        self.next_lamport.set(v);
        v
    }

    pub fn epoch_at(&self, cp: u64) -> u64 {
        self.epochs.borrow().iter().rev().find(|(start, _)| *start <= cp).map(|(_, e)| *e).unwrap_or(0)
    }

    pub fn epoch(&self) -> u64 {
        self.epoch_at(self.head.get())
    }

    fn put_object(&self, id: Bytes32, owner: SimOwner, type_repr: String, contents: Vec<u8>, from_cp: u64) {
        let version = self.lamport();
        let digest = digest_of("obj", &id, version);
        self.objects.borrow_mut().entry(id).or_default().push(ObjVersion { from_cp, version, digest, owner, type_repr, contents });
    }

    fn bump(&self, id: &Bytes32, from_cp: u64) {
        let mut objs = self.objects.borrow_mut();
        let latest = objs.get(id).and_then(|v| v.last()).cloned().expect("object");
        let version = self.lamport();
        objs.get_mut(id).unwrap().push(ObjVersion { from_cp, version, digest: digest_of("obj", id, version), ..latest });
    }

    pub fn current_ref(&self, id: &Bytes32) -> reads::ObservedRef {
        let o = self.objects.borrow();
        let v = o.get(id).and_then(|v| v.last()).expect("object");
        reads::ObservedRef { id: *id, version: v.version, digest: v.digest }
    }

    /// Re-record the gate object at checkpoint `cp` from the simulated state.
    fn sync_gate(&self, cp: u64) {
        let g = self.gate.borrow().clone();
        let contents = gate_contents(&g);
        let owner = SimOwner::Shared(SIM_SHARED_VERSION);
        let type_repr = format!("{}::rumi_icusd_gate::Gate", pkg());
        let mut objs = self.objects.borrow_mut();
        let version = self.lamport();
        objs.entry(SIM_GATE).or_default().push(ObjVersion { from_cp: cp, version, digest: digest_of("obj", &SIM_GATE, version), owner, type_repr, contents });
    }

    // ---- scenario controls -------------------------------------------------

    pub fn set_provider(&self, id: &str, p: Provider) {
        self.providers.borrow_mut().insert(id.to_string(), p);
    }

    pub fn advance(&self, n: u64) {
        self.head.set(self.head.get() + n);
    }

    pub fn next_epoch(&self) {
        let cp = self.head.get() + 1;
        let next = self.epoch_at(cp - 1) + 1;
        self.epochs.borrow_mut().push((cp, next));
        self.head.set(cp);
    }

    /// An admin raised the supply cap (a `set_limits` transaction by the `AdminCap` holder).
    pub fn set_supply_cap(&self, cap: u64) {
        let cp = self.head.get() + 1;
        self.head.set(cp);
        self.gate.borrow_mut().supply_cap = cap;
        self.sync_gate(cp);
    }

    pub fn set_gas_balance(&self, balance: u64) {
        let cp = self.head.get() + 1;
        self.head.set(cp);
        let latest = self.objects.borrow().get(&SIM_GAS_COIN).and_then(|v| v.last()).cloned().expect("gas coin");
        let version = self.lamport();
        self.objects.borrow_mut().get_mut(&SIM_GAS_COIN).unwrap().push(ObjVersion {
            from_cp: cp,
            version,
            digest: digest_of("obj", &SIM_GAS_COIN, version),
            contents: coin_contents(&SIM_GAS_COIN, balance),
            ..latest
        });
    }

    /// A transaction we did not build consumes the mint cap (its version moves, no receipt of ours).
    pub fn foreign_consume_cap(&self) {
        let cp = self.head.get() + 1;
        self.head.set(cp);
        self.bump(&SIM_MINT_CAP, cp);
        let foreign = digest_of("foreign", &SIM_MINT_CAP, cp);
        self.txs.borrow_mut().insert(foreign, SimTx { status: SimStatus::Success, checkpoint: cp, epoch: self.epoch() });
    }

    /// Someone mints with this id (a foreign mint, or a lost mint of ours that landed).
    pub fn foreign_mint(&self, mint_id: Bytes32, recipient: Bytes32, amount: u64) {
        let cp = self.head.get() + 1;
        self.head.set(cp);
        let mut g = self.gate.borrow_mut();
        g.mint_count += 1;
        g.minted_total += amount as u128;
        g.total_supply += amount;
        let sequence = g.mint_count;
        g.mints.insert(mint_id, SimMint { recipient, amount, sequence, from_cp: cp });
        drop(g);
        self.sync_gate(cp);
        self.bump(&SIM_MINT_CAP, cp);
    }

    /// A transaction a validator quorum had certified executes now, whatever the epoch (the case the
    /// design cannot rule out after an epoch ends). Executes the accepted-but-unexecuted `digest`.
    pub fn execute_accepted(&self, tx_bcs: &[u8]) {
        let parsed = tx::parse_mint_transaction(tx_bcs).expect("a mint transaction");
        let digest = tx::transaction_digest(tx_bcs);
        assert!(self.accepted.borrow().contains(&digest), "only an accepted transaction can execute");
        let cap = self.current_ref(&SIM_MINT_CAP);
        assert_eq!(parsed.mint_cap.version, cap.version, "its owned inputs must still be current");
        let cp = self.head.get() + 1;
        self.head.set(cp);
        let mut g = self.gate.borrow_mut();
        g.mint_count += 1;
        g.minted_total += parsed.amount as u128;
        g.total_supply += parsed.amount;
        let sequence = g.mint_count;
        g.mints.insert(parsed.mint_id, SimMint { recipient: parsed.recipient, amount: parsed.amount, sequence, from_cp: cp });
        drop(g);
        self.sync_gate(cp);
        self.bump(&SIM_MINT_CAP, cp);
        self.bump(&SIM_GAS_COIN, cp);
        self.txs.borrow_mut().insert(digest, SimTx { status: SimStatus::Success, checkpoint: cp, epoch: self.epoch() });
    }

    /// The chain the providers serve has no such object (it was never published here, or is not served).
    pub fn forget_object(&self, id: &Bytes32) {
        self.objects.borrow_mut().remove(id);
    }

    /// An object is transferred to another address (a new version at the next checkpoint).
    pub fn set_owner(&self, id: &Bytes32, owner: Bytes32) {
        let cp = self.head.get() + 1;
        self.head.set(cp);
        let latest = self.objects.borrow().get(id).and_then(|v| v.last()).cloned().expect("object");
        let version = self.lamport();
        self.objects.borrow_mut().get_mut(id).unwrap().push(ObjVersion { from_cp: cp, version, digest: digest_of("obj", id, version), owner: SimOwner::Address(owner), ..latest });
    }

    pub fn tx(&self, digest: &Bytes32) -> Option<SimTx> {
        self.txs.borrow().get(digest).cloned()
    }

    pub fn mint_record(&self, mint_id: &Bytes32) -> Option<SimMint> {
        self.gate.borrow().mints.get(mint_id).cloned()
    }

    pub fn executed_count(&self) -> usize {
        self.txs.borrow().values().filter(|t| t.status == SimStatus::Success).count()
    }

    // ---- the network -------------------------------------------------------

    /// What validators do with a signed transaction a provider forwards.
    pub fn submit(&self, tx_bcs: &[u8], signatures: &[Vec<u8>]) -> Result<(), String> {
        let parsed = tx::parse_mint_transaction(tx_bcs).map_err(|e| format!("not a transaction this chain knows: {e}"))?;
        let signing = tx::signing_digest(tx_bcs);
        let digest = tx::transaction_digest(tx_bcs);
        // Two signatures, one per signer address, each valid over the signing digest.
        let mut signers = vec![];
        for s in signatures {
            if s.len() != tx::WIRE_SIGNATURE_LEN || s[0] != crate::sui::ED25519_FLAG {
                return Err("bad signature encoding".into());
            }
            let sig: [u8; 64] = s[1..65].try_into().unwrap();
            let key: Bytes32 = s[65..].try_into().unwrap();
            if !crate::schnorr::verify_signature(&key, &signing, &sig) {
                return Err("signature does not verify".into());
            }
            signers.push(address_from_ed25519(&key));
        }
        if !signers.contains(&parsed.sender) || !signers.contains(&parsed.gas_owner) {
            return Err("missing a required signature".into());
        }
        if parsed.epoch < self.epoch() {
            return Err(format!("transaction expired: epoch {} < current {}", parsed.epoch, self.epoch()));
        }
        let cap = self.current_ref(&SIM_MINT_CAP);
        let gas = self.current_ref(&SIM_GAS_COIN);
        if parsed.mint_cap.version != cap.version || parsed.mint_cap.digest != cap.digest {
            return Err("object version unavailable for mint cap".into());
        }
        if parsed.gas_payment[0].version != gas.version || parsed.gas_payment[0].digest != gas.digest {
            return Err("object version unavailable for gas coin".into());
        }
        if self.txs.borrow().contains_key(&digest) {
            return Ok(()); // a duplicate submission of an executed transaction
        }
        if !self.accepted.borrow().contains(&digest) {
            self.accepted.borrow_mut().push(digest);
        }
        let mode = self.mode.borrow().clone();
        match mode {
            ExecMode::Drop => Ok(()),
            ExecMode::Execute | ExecMode::Abort(_) => {
                let cp = self.head.get() + 1;
                self.head.set(cp);
                let mut failure = if let ExecMode::Abort(m) = mode { Some(m) } else { None };
                if failure.is_none() {
                    let g = self.gate.borrow();
                    if g.mints.contains_key(&parsed.mint_id) {
                        failure = Some("MoveAbort EMintConsumed (1)".into());
                    } else if parsed.amount == 0 {
                        failure = Some("MoveAbort EZeroAmount (3)".into());
                    } else if (g.total_supply as u128) + (parsed.amount as u128) > g.supply_cap as u128 {
                        failure = Some("MoveAbort ESupplyCapExceeded (5)".into());
                    }
                }
                match failure {
                    None => {
                        let mut g = self.gate.borrow_mut();
                        g.mint_count += 1;
                        g.minted_total += parsed.amount as u128;
                        g.total_supply += parsed.amount;
                        let sequence = g.mint_count;
                        g.mints.insert(parsed.mint_id, SimMint { recipient: parsed.recipient, amount: parsed.amount, sequence, from_cp: cp });
                        drop(g);
                        self.sync_gate(cp);
                        self.bump(&SIM_MINT_CAP, cp);
                        self.bump(&SIM_GAS_COIN, cp);
                        self.txs.borrow_mut().insert(digest, SimTx { status: SimStatus::Success, checkpoint: cp, epoch: self.epoch() });
                    }
                    Some(msg) => {
                        if self.bump_on_abort.get() {
                            self.bump(&SIM_MINT_CAP, cp);
                            self.bump(&SIM_GAS_COIN, cp);
                        } else {
                            // The gas coin is still charged; the cap is untouched.
                            self.bump(&SIM_GAS_COIN, cp);
                        }
                        self.txs.borrow_mut().insert(digest, SimTx { status: SimStatus::Failure(msg), checkpoint: cp, epoch: self.epoch() });
                    }
                }
                Ok(())
            }
        }
    }

    // ---- JSON ----------------------------------------------------------------

    fn chain_text(&self, p: &Provider) -> String {
        if p.wrong_network {
            format_digest_base58(&crate::ids::MAINNET_CHAIN_IDENTIFIER)
        } else {
            format_digest_base58(&self.genesis)
        }
    }

    fn cp_digest(n: u64) -> Bytes32 {
        digest_of("checkpoint", &[0u8; 32], n)
    }

    fn object_json(&self, p: &Provider, id: &Bytes32, at: Option<u64>, exact_version: Option<u64>, with_owner_address: bool) -> Value {
        let objs = self.objects.borrow();
        let Some(history) = objs.get(id) else { return Value::Null };
        let found = match (at, exact_version) {
            (_, Some(v)) => history.iter().find(|o| o.version == v),
            (Some(cp), None) => {
                if cp > self.head.get().saturating_sub(p.lag) {
                    return Value::Null;
                }
                history.iter().rev().find(|o| o.from_cp <= cp)
            }
            (None, None) => history.last(),
        };
        let Some(o) = found else { return Value::Null };
        let mut contents = o.contents.clone();
        if *id == SIM_GATE {
            if let Some(forged) = p.forge_supply {
                let mut g = self.gate.borrow().clone();
                g.total_supply = forged;
                contents = gate_contents(&g);
            }
        }
        let owner = match &o.owner {
            SimOwner::Shared(v) => json!({ "__typename": "Shared", "initialSharedVersion": v }),
            SimOwner::Address(a) if with_owner_address => json!({ "__typename": "AddressOwner", "address": { "address": format_address(a) } }),
            SimOwner::Address(_) => json!({ "__typename": "AddressOwner" }),
        };
        let version = if p.wrong_exact_version && exact_version.is_some() { o.version + 1 } else { o.version };
        json!({
            "address": format_address(id),
            "version": version,
            "digest": format_digest_base58(&o.digest),
            "owner": owner,
            "asMoveObject": { "contents": { "type": { "repr": o.type_repr }, "bcs": STANDARD.encode(&contents) } }
        })
    }

    fn handle(&self, name: &str, p: &Provider, query: &str, vars: &Value) -> Result<Value, String> {
        let chain = self.chain_text(p);
        let head = self.head.get().saturating_sub(p.lag);
        let mut data = serde_json::Map::new();
        data.insert("chainIdentifier".into(), json!(chain));
        let id_of = |key: &str| -> Result<Bytes32, String> {
            crate::sui::parse_address_padded(vars[key].as_str().ok_or("missing variable")?).map_err(|e| format!("{e}"))
        };
        match query {
            q if q == graphql::QUERY_CHAIN => {}
            q if q == reads::QUERY_HEAD => {
                data.insert("checkpoint".into(), json!({ "sequenceNumber": head }));
            }
            q if q == reads::QUERY_CHECKPOINT_GAS || q == graphql::QUERY_CHECKPOINT => {
                let s = vars["s"].as_u64().ok_or("missing s")?;
                let with_gas = q == reads::QUERY_CHECKPOINT_GAS;
                let cp = if s > head {
                    Value::Null
                } else if with_gas {
                    json!({ "sequenceNumber": s, "digest": format_digest_base58(&Self::cp_digest(s)), "epoch": { "epochId": self.epoch_at(s), "referenceGasPrice": REFERENCE_GAS_PRICE.to_string() } })
                } else {
                    json!({ "sequenceNumber": s, "digest": format_digest_base58(&Self::cp_digest(s)), "epoch": { "epochId": self.epoch_at(s) } })
                };
                data.insert("checkpoint".into(), cp);
            }
            q if q == graphql::QUERY_OBJECT_AT_CHECKPOINT || q == reads::QUERY_OWNED_OBJECT => {
                let id = id_of("a")?;
                let c = vars["c"].as_u64().ok_or("missing c")?;
                data.insert("object".into(), self.object_json(p, &id, Some(c), None, q == reads::QUERY_OWNED_OBJECT));
            }
            q if q == graphql::QUERY_OBJECT_AT_VERSION => {
                let id = id_of("a")?;
                let v = vars["v"].as_u64().ok_or("missing v")?;
                data.insert("object".into(), self.object_json(p, &id, None, Some(v), false));
            }
            q if q == graphql::QUERY_TABLE_ENTRY => {
                let table = id_of("t")?;
                let c = vars["c"].as_u64().ok_or("missing c")?;
                if c > head || table != SIM_MINTS_TABLE {
                    data.insert("address".into(), json!({ "dynamicField": Value::Null }));
                } else {
                    let name_b64 = vars["n"]["bcs"].as_str().ok_or("missing name")?;
                    let name_bcs = STANDARD.decode(name_b64).map_err(|e| e.to_string())?;
                    let key: Bytes32 = name_bcs[name_bcs.len() - 32..].try_into().map_err(|_| "bad key".to_string())?;
                    let entry = self.gate.borrow().mints.get(&key).filter(|m| m.from_cp <= c).cloned();
                    match entry {
                        None => {
                            data.insert("address".into(), json!({ "dynamicField": Value::Null }));
                        }
                        Some(m) => {
                            let mut w = Writer::new();
                            w.raw(&m.recipient).u64(m.amount).u64(m.sequence);
                            data.insert(
                                "address".into(),
                                json!({ "dynamicField": {
                                    "name": { "bcs": name_b64 },
                                    "value": { "__typename": "MoveValue", "type": { "repr": format!("{}::rumi_icusd_gate::MintRecord", pkg()) }, "bcs": STANDARD.encode(w.into_bytes()) }
                                } }),
                            );
                        }
                    }
                }
            }
            q if q == reads::QUERY_COIN_METADATA => {
                let md = self.coin_metadata.borrow().clone();
                data.insert("coinMetadata".into(), md.map(|(d, s)| json!({ "decimals": d, "symbol": s })).unwrap_or(Value::Null));
            }
            q if q == reads::QUERY_TRANSACTION => {
                let d = crate::sui::parse_digest_base58(vars["d"].as_str().ok_or("missing d")?).map_err(|e| format!("{e}"))?;
                let found = if p.no_transactions { None } else { self.txs.borrow().get(&d).filter(|t| t.checkpoint <= head).cloned() };
                data.insert(
                    "transaction".into(),
                    match found {
                        None => Value::Null,
                        Some(t) => json!({ "digest": format_digest_base58(&d), "effects": {
                            "status": if t.status == SimStatus::Success { "SUCCESS" } else { "FAILURE" },
                            "executionError": match &t.status { SimStatus::Failure(m) => json!({ "abortCode": "1", "message": m }), SimStatus::Success => Value::Null },
                            "checkpoint": { "sequenceNumber": t.checkpoint },
                            "epoch": { "epochId": t.epoch },
                        } }),
                    },
                );
            }
            q if q == reads::MUTATION_EXECUTE => {
                let tx = STANDARD.decode(vars["t"].as_str().ok_or("missing t")?).map_err(|e| e.to_string())?;
                let sigs: Vec<Vec<u8>> = vars["s"]
                    .as_array()
                    .ok_or("missing s")?
                    .iter()
                    .map(|s| STANDARD.decode(s.as_str().unwrap_or("")).map_err(|e| e.to_string()))
                    .collect::<Result<_, _>>()?;
                return match self.submit(&tx, &sigs) {
                    Ok(()) => Ok(json!({ "data": { "executeTransaction": { "effects": { "status": "SUCCESS" } } } })),
                    Err(e) => {
                        self.rejections.borrow_mut().push(format!("{name}: {e}"));
                        Ok(json!({ "errors": [{ "message": e }] }))
                    }
                };
            }
            other => return Err(format!("the simulated provider does not know the query {other:?}")),
        }
        Ok(json!({ "data": Value::Object(data) }))
    }
}

fn pkg() -> String {
    format!("0x{}", hex::encode(SIM_PACKAGE))
}

fn coin_contents(id: &Bytes32, balance: u64) -> Vec<u8> {
    [id.as_slice(), balance.to_le_bytes().as_slice()].concat()
}

/// The BCS of the gate object, field for field as `gate::decode_gate` reads it.
pub fn gate_contents(g: &SimGate) -> Vec<u8> {
    let mut w = Writer::new();
    w.raw(&SIM_GATE).u64(g.version).raw(&SIM_TREASURY_CAP).u64(g.total_supply);
    w.bytes(&crate::ids::TESTNET_CHAIN_IDENTIFIER);
    w.u64(g.supply_cap).u64(g.min_bridge_out).u128(g.minted_total).u128(g.burned_total).u64(g.mint_count).u64(g.bridge_out_count);
    w.raw(&SIM_MINTS_TABLE).u64(g.mints.len() as u64);
    w.raw(&SIM_RETURNS_TABLE).u64(g.bridge_out_count);
    w.into_bytes()
}

/// The transport the mirror runs on in the tests: each simulated provider answers as the GraphQL
/// service would, and the response goes through the same transform the canister installs.
pub struct SimTransport<'a> {
    pub sim: &'a ChainSim,
}

impl SuiTransport for SimTransport<'_> {
    async fn post(&self, provider: &ProviderConfig, kind: CallKind, body: Vec<u8>, max_response_bytes: u32) -> Result<Vec<u8>, String> {
        let p = self.sim.providers.borrow().get(&provider.id).cloned().unwrap_or_default();
        if p.down {
            return Err(format!("{}: connection refused", provider.id));
        }
        let req: Value = serde_json::from_slice(&body).map_err(|e| format!("bad request: {e}"))?;
        let query = req["query"].as_str().ok_or("no query")?.to_string();
        self.sim.served.borrow_mut().push(query.chars().take(24).collect());
        let response = self.sim.handle(&provider.id, &p, &query, &req["variables"])?;
        if kind == CallKind::Send && p.lose_send_reply {
            return Err(format!("{}: timeout waiting for the response", provider.id));
        }
        let bytes = serde_json::to_vec(&response).unwrap();
        match kind {
            CallKind::Read => graphql::canonicalize_response(&bytes, max_response_bytes as usize),
            CallKind::Head => reads::canonicalize_head(&bytes, max_response_bytes as usize),
            CallKind::Send => Ok(reads::SEND_ACCEPTED_BODY.to_vec()),
        }
    }
}
