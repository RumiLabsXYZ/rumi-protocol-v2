//! Stable records of the nonce lane (BB-48): the nonce pool slots and the operations whose
//! attempts the lane signs, broadcasts and reconciles.
//!
//! Records are CBOR (see `state.rs`), so a field added later with `#[serde(default)]` decodes
//! from older bytes; `records::tests` pin an image of each record so a rename or retype is
//! caught before it can wipe state at an upgrade. Signatures are `Sig` (a byte string, because
//! serde has no impl for `[u8; 64]`); keys, hashes and ids are `[u8; 32]`.
//!
//! Vocabulary (note section 4):
//!
//! * a **slot** is one durable-nonce account of the pool; it is *reserved* by one operation from
//!   the moment the operation's message is persisted until its attempt is proven consumed;
//! * an **attempt** is one persisted unsigned message bound to one nonce value: the Original, and
//!   at most one Cancel (an advance-only message) per generation;
//! * a **generation** starts when the same operation id is re-issued on a fresh nonce value after
//!   a proven non-delivery.

use crate::ids::Hash32;
use crate::tx::builders::RefundDestination;
use crate::tx::{Pubkey, Signature};
use candid::CandidType;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A 64-byte signature as a CBOR byte string.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Sig(pub Signature);

impl std::fmt::Debug for Sig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sig({})", crate::tx::signature_to_base58(&self.0))
    }
}

impl Serialize for Sig {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for Sig {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let bytes = serde_bytes::ByteBuf::deserialize(d)?;
        Signature::try_from(bytes.as_slice())
            .map(Sig)
            .map_err(|_| serde::de::Error::invalid_length(bytes.len(), &"64 signature bytes"))
    }
}

/// The two durable-nonce lanes. A burn (and a stray refund) uses the burn lane, so a mint can
/// never starve a burn of nonces and a halt in one lane leaves the other readable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, CandidType)]
pub enum Lane {
    Mint,
    Burn,
}

impl Lane {
    pub const ALL: [Lane; 2] = [Lane::Mint, Lane::Burn];

    /// One letter used in the nonce account seed.
    pub fn seed_letter(&self) -> char {
        match self {
            Lane::Mint => 'm',
            Lane::Burn => 'b',
        }
    }
}

// ---------------------------------------------------------------------------
// Pool slots
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotState {
    /// Registered, not yet read back as an initialized nonce account owned by System with the
    /// fee payer as authority.
    Unverified,
    Free,
    Reserved { op_id: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonceSlot {
    pub lane: Lane,
    pub index: u8,
    /// `rumi-nonce-<m|b>-<index>`; the address is `create_with_seed(fee-payer, seed, System)`.
    pub seed: String,
    pub address: Pubkey,
    pub state: SlotState,
    /// The last durable nonce value read from the account (verified at `finalized`).
    #[serde(default)]
    pub last_value: Option<Hash32>,
    /// The last transaction known to have written this account (the creation, or the consumer
    /// of the previous value). Discovery lists only newer signatures (`until`).
    #[serde(default)]
    pub anchor: Option<Sig>,
    /// Bumped on every reservation, release and verification. A preparation read while the slot
    /// was `Free` is committed only if the slot is still `Free` AT THE SAME generation: a slot that
    /// was reserved, consumed and released in between is `Free` again but holds a new value, and
    /// a message prepared on the old value must not be admitted on it (free-slot ABA).
    #[serde(default)]
    pub generation: u64,
}

/// `lane << 8 | index`, the key of the slot map.
pub fn slot_key(lane: Lane, index: u8) -> u16 {
    ((lane as u16) << 8) | index as u16
}

// ---------------------------------------------------------------------------
// Operations and attempts
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, CandidType)]
pub enum OpKind {
    Mint,
    Burn,
    Refund,
}

impl OpKind {
    /// Mints use the mint lane; burns and stray refunds the burn lane.
    pub fn lane(&self) -> Lane {
        match self {
            OpKind::Mint => Lane::Mint,
            OpKind::Burn | OpKind::Refund => Lane::Burn,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpPayload {
    /// `recipient_wallet` and the profile's `per_operation_cap_e8s` at admission.
    Mint { recipient_wallet: Pubkey, per_operation_cap: u64 },
    Burn,
    Refund { destination: RefundDestination },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttemptKind {
    Original,
    /// The advance-only message on the same nonce (reconcile step 2).
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureRecord {
    pub signer: Pubkey,
    pub signature: Sig,
    pub at_ns: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub kind: AttemptKind,
    pub generation: u32,
    pub nonce_account: Pubkey,
    /// The durable nonce value the message is bound to (its `recent_blockhash`).
    pub nonce_value: Hash32,
    /// The exact unsigned message bytes. The identity of the attempt: signed as is, rebroadcast
    /// as is, and the only bytes a landed transaction may be adopted against.
    #[serde(with = "serde_bytes")]
    pub message: Vec<u8>,
    /// Required signers in signature order.
    pub signers: Vec<Pubkey>,
    /// EVERY signature ever received for these bytes, in arrival order (a re-sign may return a
    /// different signature if threshold signing is not deterministic).
    #[serde(default)]
    pub signatures: Vec<SignatureRecord>,
    /// The transaction id (first signature) of every wire transaction assembled for broadcast,
    /// recorded BEFORE the send. Landed lookups check all of them.
    #[serde(default)]
    pub tx_ids: Vec<Sig>,
    pub created_at_ns: u64,
    #[serde(default)]
    pub sign_failures: u32,
    #[serde(default)]
    pub last_sign_error: Option<String>,
    #[serde(default)]
    pub broadcast_count: u32,
    #[serde(default)]
    pub last_broadcast_ns: Option<u64>,
    #[serde(default)]
    pub last_broadcast_note: Option<String>,
}

impl Attempt {
    pub fn has_signature_from(&self, signer: &Pubkey) -> bool {
        self.signatures.iter().any(|r| &r.signer == signer)
    }

    /// Every required signer has at least one recorded signature.
    pub fn fully_signed(&self) -> bool {
        self.signers.iter().all(|s| self.has_signature_from(s))
    }

    /// The newest signature of each required signer, in signer order, or `None` if one is missing.
    pub fn latest_signatures(&self) -> Option<Vec<Signature>> {
        self.signers
            .iter()
            .map(|s| self.signatures.iter().rev().find(|r| &r.signer == s).map(|r| r.signature.0))
            .collect()
    }
}

/// Why the nonce was consumed without delivery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotDeliveredReason {
    /// Our advance-only cancel landed with `err == null`: the original can never land.
    CancelLanded { signature: Sig },
    /// One of our attempts landed with `err != null` (the fee is lost, the nonce is consumed).
    LandedWithError { signature: Sig },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpState {
    /// A current-generation attempt exists; reconciled every tick.
    Active,
    /// The original (or an exact stored message) landed with `err == null`. Terminal.
    /// `adopted` is true when it was found by discovery rather than by a recorded signature.
    Delivered { signature: Sig, slot: u64, adopted: bool },
    /// The nonce is proven consumed without delivery; the same id waits for a fresh nonce value.
    NeedsReissue { reason: NotDeliveredReason },
    /// Unverifiable or foreign consumer, re-issue limit, or history horizon: no replacement and
    /// no refund is invented. Resolved by hand.
    Held { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolOp {
    pub id: u64,
    pub kind: OpKind,
    pub payload: OpPayload,
    /// `mint_id`, `burn_id` or `refund_id`: the idempotency key and the memo content.
    pub external_id: Hash32,
    pub amount: u64,
    pub state: OpState,
    pub lane: Lane,
    /// The reserved slot of the current generation.
    #[serde(default)]
    pub slot_index: Option<u8>,
    pub generation: u32,
    /// Every attempt of every generation, oldest first.
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    /// The nonce value seen on the first tick that found the account advanced (generation
    /// scoped); with `advance_seen_at_ns` it drives the history-horizon halt.
    #[serde(default)]
    pub observed_value: Option<Hash32>,
    #[serde(default)]
    pub advance_seen_at_ns: Option<u64>,
    /// Pagination cursor of the discovery sweep (`before` of the next page).
    #[serde(default)]
    pub discovery_cursor: Option<Sig>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
    /// Set by the saga once the outcome was applied to the accounting buckets.
    #[serde(default)]
    pub outcome_applied: bool,
    #[serde(default)]
    pub alarm_raised_at_ns: Option<u64>,
    #[serde(default)]
    pub note: Option<String>,
}

impl SolOp {
    pub fn is_terminal(&self) -> bool {
        matches!(self.state, OpState::Delivered { .. })
    }

    /// Attempts of the current generation.
    pub fn current_attempts(&self) -> impl Iterator<Item = (usize, &Attempt)> {
        let generation = self.generation;
        self.attempts.iter().enumerate().filter(move |(_, a)| a.generation == generation)
    }
}

// ---------------------------------------------------------------------------
// Bootstrap (49a): the mint, the return ATA and the pool accounts
// ---------------------------------------------------------------------------

/// What a bootstrap transaction creates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootstrapTarget {
    /// The escrow mint account (at the address of the `mint-account` key).
    Mint,
    /// The return associated token account.
    ReturnAta,
    /// One nonce pool account.
    PoolSlot { lane: Lane, index: u8 },
}

impl BootstrapTarget {
    /// Key of the stable map: unique per target.
    pub fn key(&self) -> u32 {
        match self {
            BootstrapTarget::Mint => 1,
            BootstrapTarget::ReturnAta => 2,
            BootstrapTarget::PoolSlot { lane, index } => 0x1000 | slot_key(*lane, *index) as u32,
        }
    }
}

/// One bootstrap transaction for one target. Persisted BEFORE it is signed, like a lane attempt:
/// the exact bytes, then every signature, then the transaction ids. These transactions use a real
/// recent blockhash (no nonce exists yet) and are idempotent in effect (the target account either
/// exists or not; the driver reads it before every send), so an expired one is replaced by a fresh
/// one rather than reconciled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapTx {
    pub target: BootstrapTarget,
    pub recent_blockhash: Hash32,
    #[serde(with = "serde_bytes")]
    pub message: Vec<u8>,
    pub signers: Vec<Pubkey>,
    #[serde(default)]
    pub signatures: Vec<SignatureRecord>,
    #[serde(default)]
    pub tx_ids: Vec<Sig>,
    pub created_at_ns: u64,
    #[serde(default)]
    pub sign_failures: u32,
    #[serde(default)]
    pub last_sign_error: Option<String>,
    #[serde(default)]
    pub broadcast_count: u32,
    #[serde(default)]
    pub last_broadcast_ns: Option<u64>,
    #[serde(default)]
    pub last_broadcast_note: Option<String>,
}

impl BootstrapTx {
    pub fn fully_signed(&self) -> bool {
        self.signers.iter().all(|s| self.signatures.iter().any(|r| &r.signer == s))
    }

    pub fn latest_signatures(&self) -> Option<Vec<Signature>> {
        self.signers
            .iter()
            .map(|s| self.signatures.iter().rev().find(|r| &r.signer == s).map(|r| r.signature.0))
            .collect()
    }
}

/// The claim on the escrow mint (note section 8): `None -> DeployedUnbound -> Bound`. Only `Bound`
/// lets the lane build mints, burns and refunds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, CandidType)]
pub enum MintState {
    #[default]
    None,
    /// The mint account exists and passed its checks; the return account is not verified yet.
    DeployedUnbound,
    /// Every bind check passed at `finalized`.
    Bound,
}

/// Lane-level halt (note section 4: "a lane halts when ...").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneHalt {
    pub reason: String,
    pub at_ns: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneHalts {
    #[serde(default)]
    pub mint: Option<LaneHalt>,
    #[serde(default)]
    pub burn: Option<LaneHalt>,
}

impl LaneHalts {
    pub fn get(&self, lane: Lane) -> Option<&LaneHalt> {
        match lane {
            Lane::Mint => self.mint.as_ref(),
            Lane::Burn => self.burn.as_ref(),
        }
    }

    pub fn set(&mut self, lane: Lane, halt: Option<LaneHalt>) {
        match lane {
            Lane::Mint => self.mint = halt,
            Lane::Burn => self.burn = halt,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{cbor_encode, try_cbor_decode};
    use ic_stable_structures::Storable;

    fn sample_attempt(kind: AttemptKind, generation: u32, byte: u8) -> Attempt {
        Attempt {
            kind,
            generation,
            nonce_account: [0x55; 32],
            nonce_value: [byte; 32],
            message: vec![1, 2, 3, byte],
            signers: vec![[0x11; 32], [0x22; 32]],
            signatures: vec![
                SignatureRecord { signer: [0x11; 32], signature: Sig([0xa1; 64]), at_ns: 10 },
                SignatureRecord { signer: [0x22; 32], signature: Sig([0xb2; 64]), at_ns: 11 },
            ],
            tx_ids: vec![Sig([0xa1; 64])],
            created_at_ns: 9,
            sign_failures: 2,
            last_sign_error: Some("SYS_UNKNOWN".into()),
            broadcast_count: 3,
            last_broadcast_ns: Some(12),
            last_broadcast_note: Some("sent".into()),
        }
    }

    pub(crate) fn sample_op() -> SolOp {
        SolOp {
            id: 7,
            kind: OpKind::Mint,
            payload: OpPayload::Mint { recipient_wallet: [0x77; 32], per_operation_cap: 1_000_000 },
            external_id: [0xa1; 32],
            amount: 123_456_789,
            state: OpState::NeedsReissue { reason: NotDeliveredReason::CancelLanded { signature: Sig([0xc3; 64]) } },
            lane: Lane::Mint,
            slot_index: Some(2),
            generation: 1,
            attempts: vec![sample_attempt(AttemptKind::Original, 0, 0x66), sample_attempt(AttemptKind::Cancel, 0, 0x66)],
            observed_value: Some([0x67; 32]),
            advance_seen_at_ns: Some(50),
            discovery_cursor: Some(Sig([0xd4; 64])),
            created_at_ns: 5,
            updated_at_ns: 60,
            outcome_applied: false,
            alarm_raised_at_ns: Some(70),
            note: Some("n".into()),
        }
    }

    pub(crate) fn sample_slot() -> NonceSlot {
        NonceSlot {
            lane: Lane::Burn,
            index: 3,
            seed: "rumi-nonce-b-3".into(),
            address: [0xac; 32],
            state: SlotState::Reserved { op_id: 7 },
            last_value: Some([0x66; 32]),
            anchor: Some(Sig([0xe5; 64])),
            generation: 4,
        }
    }

    #[test]
    fn records_round_trip_through_cbor_and_the_storable_impl() {
        let op = sample_op();
        assert_eq!(try_cbor_decode::<SolOp>(&cbor_encode(&op)).unwrap(), op);
        assert_eq!(SolOp::from_bytes(op.to_bytes()), op);
        let slot = sample_slot();
        assert_eq!(try_cbor_decode::<NonceSlot>(&cbor_encode(&slot)).unwrap(), slot);
        assert_eq!(NonceSlot::from_bytes(slot.to_bytes()), slot);
        for state in [
            OpState::Active,
            OpState::Delivered { signature: Sig([1; 64]), slot: 9, adopted: true },
            OpState::NeedsReissue { reason: NotDeliveredReason::LandedWithError { signature: Sig([2; 64]) } },
            OpState::Held { reason: "x".into() },
        ] {
            let mut o = sample_op();
            o.state = state;
            assert_eq!(try_cbor_decode::<SolOp>(&cbor_encode(&o)).unwrap(), o);
        }
        for payload in [OpPayload::Burn, OpPayload::Refund { destination: RefundDestination::CreateAta { owner: [9; 32] } }, OpPayload::Refund { destination: RefundDestination::ExistingAccount { token_account: [8; 32] } }] {
            let mut o = sample_op();
            o.payload = payload;
            assert_eq!(try_cbor_decode::<SolOp>(&cbor_encode(&o)).unwrap(), o);
        }
    }

    /// The stable layout of the new records: pinned CBOR images decode to exactly the sample, and
    /// the sample encodes to exactly the image. A renamed or retyped field fails here before it
    /// can wipe an operation at an upgrade.
    #[test]
    fn pinned_images_decode_to_the_samples() {
        let op_image = hex::decode(PINNED_OP_V1).expect("hex");
        assert_eq!(try_cbor_decode::<SolOp>(&op_image).unwrap(), sample_op());
        assert_eq!(cbor_encode(&sample_op()), op_image);
        let slot_image = hex::decode(PINNED_SLOT_V2).expect("hex");
        assert_eq!(try_cbor_decode::<NonceSlot>(&slot_image).unwrap(), sample_slot());
        assert_eq!(cbor_encode(&sample_slot()), slot_image);
        // The slot as first written by BB-48 (before `generation` existed) still decodes, at
        // generation 0.
        let old_slot = hex::decode(PINNED_SLOT_V1).expect("hex");
        assert_eq!(try_cbor_decode::<NonceSlot>(&old_slot).unwrap(), NonceSlot { generation: 0, ..sample_slot() });
    }

    #[test]
    fn older_images_without_appended_fields_decode_with_defaults() {
        use ciborium::Value;
        let strip = |v: &mut Value, keys: &[&str]| {
            if let Value::Map(entries) = v {
                entries.retain(|(k, _)| !matches!(k, Value::Text(t) if keys.contains(&t.as_str())));
            }
        };
        let mut v: Value = ciborium::de::from_reader(cbor_encode(&sample_op()).as_slice()).unwrap();
        strip(&mut v, &["slot_index", "attempts", "observed_value", "advance_seen_at_ns", "discovery_cursor", "outcome_applied", "alarm_raised_at_ns", "note"]);
        let mut bytes = vec![];
        ciborium::ser::into_writer(&v, &mut bytes).unwrap();
        let op = try_cbor_decode::<SolOp>(&bytes).unwrap();
        assert_eq!((op.slot_index, op.attempts.len(), op.observed_value, op.advance_seen_at_ns, op.discovery_cursor, op.outcome_applied, op.alarm_raised_at_ns, op.note), (None, 0, None, None, None, false, None, None));
        // An attempt written before the counters existed.
        let mut a: Value = ciborium::de::from_reader(cbor_encode(&sample_attempt(AttemptKind::Original, 0, 1)).as_slice()).unwrap();
        strip(&mut a, &["signatures", "tx_ids", "sign_failures", "last_sign_error", "broadcast_count", "last_broadcast_ns", "last_broadcast_note"]);
        let mut bytes = vec![];
        ciborium::ser::into_writer(&a, &mut bytes).unwrap();
        let attempt = try_cbor_decode::<Attempt>(&bytes).unwrap();
        assert!(attempt.signatures.is_empty() && attempt.tx_ids.is_empty() && !attempt.fully_signed());
        assert_eq!((attempt.sign_failures, attempt.broadcast_count, attempt.last_broadcast_ns), (0, 0, None));
        // A slot written before the value and the anchor were tracked.
        let mut sl: Value = ciborium::de::from_reader(cbor_encode(&sample_slot()).as_slice()).unwrap();
        strip(&mut sl, &["last_value", "anchor", "generation"]);
        let mut bytes = vec![];
        ciborium::ser::into_writer(&sl, &mut bytes).unwrap();
        let slot = try_cbor_decode::<NonceSlot>(&bytes).unwrap();
        assert_eq!((slot.last_value, slot.anchor, slot.generation), (None, None, 0));
    }

    #[test]
    fn a_signature_is_a_byte_string_of_exactly_64_bytes() {
        let ok = cbor_encode(&Sig([5; 64]));
        assert_eq!(ok.len(), 2 + 64, "major type 2 with a one-byte length, then 64 bytes");
        assert_eq!(try_cbor_decode::<Sig>(&ok).unwrap(), Sig([5; 64]));
        let short = cbor_encode(&serde_bytes::ByteBuf::from(vec![1u8; 63]));
        assert!(try_cbor_decode::<Sig>(&short).is_err());
        let long = cbor_encode(&serde_bytes::ByteBuf::from(vec![1u8; 65]));
        assert!(try_cbor_decode::<Sig>(&long).is_err());
    }

    #[test]
    fn attempt_signature_helpers() {
        let mut a = sample_attempt(AttemptKind::Original, 0, 1);
        assert!(a.fully_signed());
        assert_eq!(a.latest_signatures(), Some(vec![[0xa1; 64], [0xb2; 64]]));
        // A second signature from the first signer wins as the newest.
        a.signatures.push(SignatureRecord { signer: [0x11; 32], signature: Sig([0xa9; 64]), at_ns: 20 });
        assert_eq!(a.latest_signatures(), Some(vec![[0xa9; 64], [0xb2; 64]]));
        a.signatures.retain(|r| r.signer != [0x22; 32]);
        assert!(!a.fully_signed());
        assert_eq!(a.latest_signatures(), None);
    }

    #[test]
    fn lanes_map_op_kinds_and_have_distinct_slot_keys() {
        assert_eq!(OpKind::Mint.lane(), Lane::Mint);
        assert_eq!(OpKind::Burn.lane(), Lane::Burn);
        assert_eq!(OpKind::Refund.lane(), Lane::Burn);
        assert_ne!(slot_key(Lane::Mint, 3), slot_key(Lane::Burn, 3));
        let mut halts = LaneHalts::default();
        halts.set(Lane::Burn, Some(LaneHalt { reason: "r".into(), at_ns: 1 }));
        assert!(halts.get(Lane::Mint).is_none() && halts.get(Lane::Burn).is_some());
    }

    #[test]
    #[ignore = "writes /tmp/records_v1.txt to regenerate the pinned images"]
    fn zz_dump_images() {
        std::fs::write("/tmp/records_v1.txt", format!("{}\n{}\n", hex::encode(cbor_encode(&sample_op())), hex::encode(cbor_encode(&sample_slot())))).unwrap();
    }

    const PINNED_OP_V1: &str = concat!(
        "b262696407646b696e64644d696e74677061796c6f6164a1644d696e74a270726563697069656e745f77616c6c657498",
        "201877187718771877187718771877187718771877187718771877187718771877187718771877187718771877187718",
        "7718771877187718771877187718771877717065725f6f7065726174696f6e5f6361701a000f42406b65787465726e61",
        "6c5f6964982018a118a118a118a118a118a118a118a118a118a118a118a118a118a118a118a118a118a118a118a118a1",
        "18a118a118a118a118a118a118a118a118a118a118a166616d6f756e741a075bcd15657374617465a16c4e6565647352",
        "656973737565a166726561736f6ea16c43616e63656c4c616e646564a1697369676e61747572655840c3c3c3c3c3c3c3",
        "c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3",
        "c3c3c3c3c3c3c3c3c3646c616e65644d696e746a736c6f745f696e646578026a67656e65726174696f6e016861747465",
        "6d70747382ae646b696e64684f726967696e616c6a67656e65726174696f6e006d6e6f6e63655f6163636f756e749820",
        "185518551855185518551855185518551855185518551855185518551855185518551855185518551855185518551855",
        "185518551855185518551855185518556b6e6f6e63655f76616c75659820186618661866186618661866186618661866",
        "18661866186618661866186618661866186618661866186618661866186618661866186618661866186618661866676d",
        "6573736167654401020366677369676e6572738298201111111111111111111111111111111111111111111111111111",
        "111111111111982018221822182218221822182218221822182218221822182218221822182218221822182218221822",
        "1822182218221822182218221822182218221822182218226a7369676e61747572657382a3667369676e657298201111",
        "111111111111111111111111111111111111111111111111111111111111697369676e61747572655840a1a1a1a1a1a1",
        "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
        "a1a1a1a1a1a1a1a1a1a16561745f6e730aa3667369676e65729820182218221822182218221822182218221822182218",
        "22182218221822182218221822182218221822182218221822182218221822182218221822182218221822697369676e",
        "61747572655840b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2",
        "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b26561745f6e730b6674785f696473815840a1a1a1a1a1a1a1a1",
        "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
        "a1a1a1a1a1a1a1a16d637265617465645f61745f6e73096d7369676e5f6661696c75726573026f6c6173745f7369676e",
        "5f6572726f726b5359535f554e4b4e4f574e6f62726f6164636173745f636f756e7403716c6173745f62726f61646361",
        "73745f6e730c736c6173745f62726f6164636173745f6e6f74656473656e74ae646b696e646643616e63656c6a67656e",
        "65726174696f6e006d6e6f6e63655f6163636f756e749820185518551855185518551855185518551855185518551855",
        "185518551855185518551855185518551855185518551855185518551855185518551855185518556b6e6f6e63655f76",
        "616c75659820186618661866186618661866186618661866186618661866186618661866186618661866186618661866",
        "18661866186618661866186618661866186618661866676d6573736167654401020366677369676e6572738298201111",
        "111111111111111111111111111111111111111111111111111111111111982018221822182218221822182218221822",
        "182218221822182218221822182218221822182218221822182218221822182218221822182218221822182218221822",
        "6a7369676e61747572657382a3667369676e657298201111111111111111111111111111111111111111111111111111",
        "111111111111697369676e61747572655840a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
        "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a16561745f6e730aa3667369676e65",
        "729820182218221822182218221822182218221822182218221822182218221822182218221822182218221822182218",
        "22182218221822182218221822182218221822697369676e61747572655840b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2",
        "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b265",
        "61745f6e730b6674785f696473815840a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
        "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a16d637265617465645f61745f6e73096d",
        "7369676e5f6661696c75726573026f6c6173745f7369676e5f6572726f726b5359535f554e4b4e4f574e6f62726f6164",
        "636173745f636f756e7403716c6173745f62726f6164636173745f6e730c736c6173745f62726f6164636173745f6e6f",
        "74656473656e746e6f627365727665645f76616c75659820186718671867186718671867186718671867186718671867",
        "1867186718671867186718671867186718671867186718671867186718671867186718671867186772616476616e6365",
        "5f7365656e5f61745f6e73183270646973636f766572795f637572736f725840d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4",
        "d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4",
        "6d637265617465645f61745f6e73056d757064617465645f61745f6e73183c6f6f7574636f6d655f6170706c696564f4",
        "72616c61726d5f7261697365645f61745f6e731846646e6f7465616e",
    );
    /// The slot image as the first BB-48 commit wrote it (no `generation`). NEVER regenerate.
    const PINNED_SLOT_V1: &str = concat!(
        "a7646c616e65644275726e65696e6465780364736565646e72756d692d6e6f6e63652d622d3367616464726573739820",
        "18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac",
        "18ac18ac18ac18ac18ac18ac18ac18ac657374617465a1685265736572766564a1656f705f6964076a6c6173745f7661",
        "6c7565982018661866186618661866186618661866186618661866186618661866186618661866186618661866186618",
        "66186618661866186618661866186618661866186666616e63686f725840e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5",
        "e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5",
    );

    /// The current slot image (schema with `generation`).
    const PINNED_SLOT_V2: &str = concat!(
        "a8646c616e65644275726e65696e6465780364736565646e72756d692d6e6f6e63652d622d3367616464726573739820",
        "18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac18ac",
        "18ac18ac18ac18ac18ac18ac18ac18ac657374617465a1685265736572766564a1656f705f6964076a6c6173745f7661",
        "6c7565982018661866186618661866186618661866186618661866186618661866186618661866186618661866186618",
        "66186618661866186618661866186618661866186666616e63686f725840e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5",
        "e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e56a67",
        "656e65726174696f6e04",
    );
}
