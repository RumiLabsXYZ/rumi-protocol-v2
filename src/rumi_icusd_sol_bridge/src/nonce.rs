//! The durable-nonce pool (`chain-solana.md` section 4): account layout, the checks run on EVERY
//! read, slot registration and verification, and slot reservation.
//!
//! A pool is N nonce accounts per lane (config `nonce_pool_size`, start 4). Each is created with
//! `CreateAccountWithSeed` from the fee payer (`tx::builders::build_nonce_pool_account_message`),
//! so its address is `create_with_seed(fee-payer, "rumi-nonce-<m|b>-<index>", System)` and one
//! signature suffices. A slot is reserved by one operation from the moment the operation's
//! message is persisted until the attempt is proven consumed; it is then reusable once its stored
//! value has changed.
//!
//! Every read of a nonce account is checked (`check_snapshot`): owner System, version Current,
//! state Initialized, authority equal to the fee payer. A failed check is a lane halt (the
//! "nonce owner or authority changed" trigger); the lane never signs against an account it
//! cannot verify.
//!
//! Account data layout (80 bytes, read from `solana-nonce` 2.2.1 `Versions`/`State`/`Data`,
//! bincode): `[version u32][state u32][authority 32][durable nonce 32][lamports/signature u64]`,
//! version 1 = `Current` (a `Legacy` nonce is invalid for durable transactions), state 1 =
//! `Initialized`.

use crate::ids::Hash32;
use crate::records::{slot_key, Lane, NonceSlot, SlotState};
use crate::sol_rpc::{AccountSnapshot, RpcFailure, SolRpc};
use crate::state;
use crate::tx::{create_with_seed, Pubkey, TxError, NONCE_ACCOUNT_SPACE, SYSTEM_PROGRAM_ID};

/// The parsed, initialized content of a nonce account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NonceState {
    pub authority: Pubkey,
    /// The stored durable nonce value: the `recent_blockhash` of the next durable transaction.
    pub value: Hash32,
    pub lamports_per_signature: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NonceProblem {
    WrongOwner(Pubkey),
    WrongAuthority(Pubkey),
    NotInitialized,
    /// `Versions::Legacy`: not valid for durable transactions.
    LegacyVersion,
    BadData(String),
}

impl std::fmt::Display for NonceProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NonceProblem::WrongOwner(o) => write!(f, "nonce account owner changed to {}", crate::tx::pubkey_to_base58(o)),
            NonceProblem::WrongAuthority(a) => write!(f, "nonce authority changed to {}", crate::tx::pubkey_to_base58(a)),
            NonceProblem::NotInitialized => write!(f, "nonce account is not initialized"),
            NonceProblem::LegacyVersion => write!(f, "nonce account is a legacy version"),
            NonceProblem::BadData(m) => write!(f, "nonce account data is malformed: {m}"),
        }
    }
}

/// Parse the 80-byte account data. Pure.
pub fn parse_nonce_data(data: &[u8]) -> Result<NonceState, NonceProblem> {
    let data: &[u8; 80] = data.try_into().map_err(|_| NonceProblem::BadData(format!("expected {NONCE_ACCOUNT_SPACE} bytes, got {}", data.len())))?;
    let word = |i: usize| u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
    match word(0) {
        1 => {}
        0 => return Err(NonceProblem::LegacyVersion),
        other => return Err(NonceProblem::BadData(format!("unknown version {other}"))),
    }
    match word(4) {
        1 => {}
        0 => return Err(NonceProblem::NotInitialized),
        other => return Err(NonceProblem::BadData(format!("unknown state {other}"))),
    }
    let mut authority = [0u8; 32];
    authority.copy_from_slice(&data[8..40]);
    let mut value = [0u8; 32];
    value.copy_from_slice(&data[40..72]);
    let mut fee = [0u8; 8];
    fee.copy_from_slice(&data[72..80]);
    Ok(NonceState { authority, value, lamports_per_signature: u64::from_le_bytes(fee) })
}

/// The check run on EVERY read: owner System, then the data, then authority = fee payer.
pub fn check_snapshot(snapshot: &AccountSnapshot, fee_payer: &Pubkey) -> Result<NonceState, NonceProblem> {
    if snapshot.owner != SYSTEM_PROGRAM_ID {
        return Err(NonceProblem::WrongOwner(snapshot.owner));
    }
    let nonce = parse_nonce_data(&snapshot.data)?;
    if &nonce.authority != fee_payer {
        return Err(NonceProblem::WrongAuthority(nonce.authority));
    }
    Ok(nonce)
}

/// The seed of pool slot `index` of `lane`: `rumi-nonce-<m|b>-<index>` (at most 32 bytes).
pub fn slot_seed(lane: Lane, index: u8) -> String {
    format!("rumi-nonce-{}-{index}", lane.seed_letter())
}

/// `(seed, address)` of a pool slot for this fee payer.
pub fn slot_address(fee_payer: &Pubkey, lane: Lane, index: u8) -> Result<(String, Pubkey), TxError> {
    let seed = slot_seed(lane, index);
    let address = create_with_seed(fee_payer, &seed, &SYSTEM_PROGRAM_ID)?;
    Ok((seed, address))
}

/// Why registering or reserving failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PoolError {
    NotConfigured(String),
    IndexOutOfRange { index: u8, pool_size: u8 },
    AlreadyRegistered,
    Tx(TxError),
}

/// Register slot `index` of `lane` (state `Unverified`). The account must be created on chain and
/// then verified with [`verify_slot`] before it is used. Idempotent registration is refused so a
/// slot's address never changes under a reservation.
pub fn register_slot(lane: Lane, index: u8) -> Result<NonceSlot, PoolError> {
    let (fee_payer, pool_size) = state::read_core(|c| (c.keys.as_ref().map(|k| k.fee_payer), c.config.lane.nonce_pool_size));
    let fee_payer = fee_payer.ok_or_else(|| PoolError::NotConfigured("role keys are not derived yet".into()))?;
    if index >= pool_size {
        return Err(PoolError::IndexOutOfRange { index, pool_size });
    }
    if state::get_slot(lane, index).is_some() {
        return Err(PoolError::AlreadyRegistered);
    }
    let (seed, address) = slot_address(&fee_payer, lane, index).map_err(PoolError::Tx)?;
    let slot = NonceSlot { lane, index, seed, address, state: SlotState::Unverified, last_value: None, anchor: None };
    state::put_slot(&slot);
    Ok(slot)
}

/// The result of verifying one slot against the chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// Initialized, owned by System, authority fee payer: the slot is `Free` with this value.
    Verified(Hash32),
    /// The account does not exist at `finalized` (yet).
    NotCreated,
    /// A check failed. The slot stays `Unverified` (or `Free`); nothing is reserved against it.
    Problem(NonceProblem),
    /// Inconsistent or failed read: try again.
    Unavailable(RpcFailure),
}

/// Read a registered slot and, if it passes every check, mark an `Unverified` slot `Free` with its
/// current value. A `Free` slot's `last_value` is refreshed. A `Reserved` slot is not touched.
pub async fn verify_slot<R: SolRpc>(rpc: &R, lane: Lane, index: u8) -> Result<VerifyOutcome, PoolError> {
    let slot = state::get_slot(lane, index).ok_or_else(|| PoolError::NotConfigured("slot is not registered".into()))?;
    let fee_payer = state::read_core(|c| c.keys.as_ref().map(|k| k.fee_payer)).ok_or_else(|| PoolError::NotConfigured("role keys are not derived yet".into()))?;
    let snapshot = match rpc.get_account(&slot.address).await {
        Ok(Some(s)) => s,
        Ok(None) => return Ok(VerifyOutcome::NotCreated),
        Err(e) => return Ok(VerifyOutcome::Unavailable(e)),
    };
    let nonce = match check_snapshot(&snapshot, &fee_payer) {
        Ok(n) => n,
        Err(p) => return Ok(VerifyOutcome::Problem(p)),
    };
    // Re-read the slot: it may have been reserved while the read was in flight.
    if let Some(mut current) = state::get_slot(lane, index) {
        if matches!(current.state, SlotState::Unverified | SlotState::Free) {
            current.state = SlotState::Free;
            current.last_value = Some(nonce.value);
            state::put_slot(&current);
        }
    }
    Ok(VerifyOutcome::Verified(nonce.value))
}

/// Reserve slot `index` for `op_id`, only if it is `Free` right now. Atomic (one synchronous
/// block), which is what lets two concurrent submissions never share a slot.
pub fn reserve_slot(lane: Lane, index: u8, op_id: u64) -> bool {
    match state::get_slot(lane, index) {
        Some(mut s) if s.state == SlotState::Free => {
            s.state = SlotState::Reserved { op_id };
            state::put_slot(&s);
            true
        }
        _ => false,
    }
}

/// Release a slot reserved by `op_id` after its attempt is proven consumed: the slot is `Free`
/// again with the value now stored on chain and the signature of the consumer as the discovery
/// anchor. Does nothing if the slot is reserved by another operation.
pub fn release_slot(lane: Lane, index: u8, op_id: u64, new_value: Hash32, anchor: crate::records::Sig) {
    if let Some(mut s) = state::get_slot(lane, index) {
        if s.state == (SlotState::Reserved { op_id }) {
            s.state = SlotState::Free;
            s.last_value = Some(new_value);
            s.anchor = Some(anchor);
            state::put_slot(&s);
        }
    }
}

/// The lowest-index `Free` slot of a lane, if any.
pub fn first_free_slot(lane: Lane) -> Option<NonceSlot> {
    state::slots_in_lane(lane).into_iter().find(|s| s.state == SlotState::Free)
}

/// Whether a slot key is in the lane's range (used by tests of `slot_key`).
pub fn key_in_lane(key: u16, lane: Lane) -> bool {
    key >= slot_key(lane, 0) && key <= slot_key(lane, u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEE_PAYER: Pubkey = [0x11; 32];

    pub(crate) fn nonce_data(version: u32, state: u32, authority: &Pubkey, value: &Hash32, fee: u64) -> Vec<u8> {
        let mut d = Vec::with_capacity(80);
        d.extend_from_slice(&version.to_le_bytes());
        d.extend_from_slice(&state.to_le_bytes());
        d.extend_from_slice(authority);
        d.extend_from_slice(value);
        d.extend_from_slice(&fee.to_le_bytes());
        d
    }

    #[test]
    fn nonce_data_layout_is_the_documented_one() {
        let d = nonce_data(1, 1, &FEE_PAYER, &[0x66; 32], 5_000);
        assert_eq!(d.len(), 80);
        let n = parse_nonce_data(&d).unwrap();
        assert_eq!(n, NonceState { authority: FEE_PAYER, value: [0x66; 32], lamports_per_signature: 5_000 });
    }

    #[test]
    fn bad_nonce_data_is_classified() {
        assert_eq!(parse_nonce_data(&nonce_data(0, 1, &FEE_PAYER, &[1; 32], 0)), Err(NonceProblem::LegacyVersion));
        assert_eq!(parse_nonce_data(&nonce_data(1, 0, &FEE_PAYER, &[1; 32], 0)), Err(NonceProblem::NotInitialized));
        assert!(matches!(parse_nonce_data(&nonce_data(2, 1, &FEE_PAYER, &[1; 32], 0)), Err(NonceProblem::BadData(_))));
        assert!(matches!(parse_nonce_data(&nonce_data(1, 7, &FEE_PAYER, &[1; 32], 0)), Err(NonceProblem::BadData(_))));
        for len in [0usize, 79, 81, 100] {
            assert!(matches!(parse_nonce_data(&vec![1u8; len]), Err(NonceProblem::BadData(_))), "length {len}");
        }
    }

    #[test]
    fn every_read_checks_owner_then_authority() {
        let data = nonce_data(1, 1, &FEE_PAYER, &[9; 32], 0);
        let ok = AccountSnapshot { owner: SYSTEM_PROGRAM_ID, data: data.clone() };
        assert_eq!(check_snapshot(&ok, &FEE_PAYER).unwrap().value, [9; 32]);
        let other_owner = AccountSnapshot { owner: [7; 32], data: data.clone() };
        assert_eq!(check_snapshot(&other_owner, &FEE_PAYER), Err(NonceProblem::WrongOwner([7; 32])));
        assert_eq!(check_snapshot(&ok, &[0x12; 32]), Err(NonceProblem::WrongAuthority(FEE_PAYER)));
        let uninit = AccountSnapshot { owner: SYSTEM_PROGRAM_ID, data: nonce_data(1, 0, &FEE_PAYER, &[0; 32], 0) };
        assert_eq!(check_snapshot(&uninit, &FEE_PAYER), Err(NonceProblem::NotInitialized));
    }

    #[test]
    fn slot_seeds_and_addresses() {
        assert_eq!(slot_seed(Lane::Mint, 0), "rumi-nonce-m-0");
        assert_eq!(slot_seed(Lane::Burn, 15), "rumi-nonce-b-15");
        assert!(slot_seed(Lane::Burn, 255).len() <= 32);
        let (seed, a) = slot_address(&FEE_PAYER, Lane::Mint, 0).unwrap();
        assert_eq!(seed, "rumi-nonce-m-0");
        assert_eq!(a, create_with_seed(&FEE_PAYER, "rumi-nonce-m-0", &SYSTEM_PROGRAM_ID).unwrap());
        let mut seen = std::collections::BTreeSet::new();
        for lane in Lane::ALL {
            for i in 0..16u8 {
                assert!(seen.insert(slot_address(&FEE_PAYER, lane, i).unwrap().1), "address repeats");
            }
        }
        assert_ne!(slot_address(&FEE_PAYER, Lane::Mint, 0).unwrap().1, slot_address(&[0x12; 32], Lane::Mint, 0).unwrap().1, "the fee payer is part of the address");
    }

    #[test]
    fn slot_keys_never_collide_across_lanes() {
        assert!(key_in_lane(slot_key(Lane::Mint, 200), Lane::Mint));
        assert!(!key_in_lane(slot_key(Lane::Burn, 0), Lane::Mint));
        assert!(slot_key(Lane::Mint, 255) < slot_key(Lane::Burn, 0));
    }
}
