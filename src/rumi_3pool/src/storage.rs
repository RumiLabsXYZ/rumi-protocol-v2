// Stable-memory storage backend for rumi_3pool.
//
// This module owns the `MemoryManager` and every `StableLog`, `StableBTreeMap`
// and `StableCell` the canister persists across upgrades. It replaces the
// raw-offset-0 Candid-blob scheme in `state.rs` so that:
//
//   * Unbounded collections (LP balances, event logs, ICRC-3 blocks) never
//     have to fit into a single `Encode!`/`Decode!` call during upgrade,
//     removing the #1 upgrade-trap risk as state grows.
//   * `pre_upgrade` becomes a cheap flush of a bounded `SlimState` struct.
//   * `post_upgrade` is idempotent and detects whether it is running for the
//     first time on the new wasm (one-shot drain from the legacy blob) or
//     subsequent times (load `SlimState` from its cell).
//
// Memory ID layout (18 IDs used; 255 available):
//
//   0       SlimState cell              — bounded residual heap
//   1       lp_balances                 — BTreeMap<Principal, u128>
//   2       lp_allowances               — BTreeMap<(Principal, Principal), LpAllowance>
//   3       authorized_burn_callers     — BTreeMap<Principal, ()>
//   4,5     swap_events_v1 log          — preserved forever for auditability
//   6,7     liquidity_events_v1 log     — preserved forever
//   8,9     swap_events_v2 log
//   10,11   liquidity_events_v2 log
//   12,13   admin_events log
//   14,15   vp_snapshots log
//   16,17   icrc3_blocks log
//
// Migration semantics: the first `post_upgrade` after the Phase A deploy runs
// a one-shot drain (see `storage::migration`). All subsequent upgrades just
// load `SlimState` from MemoryId 0 and leave the collections in place.
//
// Invariant: once `storage_migrated = true`, no code in this canister writes
// raw bytes to physical stable memory offset 0 ever again.

use candid::{CandidType, Decode, Encode, Principal};
use ic_stable_structures::{
    memory_manager::{MemoryId, MemoryManager, VirtualMemory},
    storable::{Bound, Storable},
    DefaultMemoryImpl, StableBTreeMap, StableCell, StableLog,
};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;

use crate::types::{
    Icrc3Block, LiquidityEventV1, LiquidityEventV2, LpAllowance, PoolConfig, SwapEventV1,
    SwapEventV2, ThreePoolAdminEvent, VirtualPriceSnapshot,
};

pub type Memory = VirtualMemory<DefaultMemoryImpl>;

// ─── Memory IDs ──────────────────────────────────────────────────────────────

const MEM_SLIM_STATE: MemoryId = MemoryId::new(0);
const MEM_LP_BALANCES: MemoryId = MemoryId::new(1);
const MEM_LP_ALLOWANCES: MemoryId = MemoryId::new(2);
const MEM_BURN_CALLERS: MemoryId = MemoryId::new(3);
const MEM_SWAP_V1_INDEX: MemoryId = MemoryId::new(4);
const MEM_SWAP_V1_DATA: MemoryId = MemoryId::new(5);
const MEM_LIQ_V1_INDEX: MemoryId = MemoryId::new(6);
const MEM_LIQ_V1_DATA: MemoryId = MemoryId::new(7);
const MEM_SWAP_V2_INDEX: MemoryId = MemoryId::new(8);
const MEM_SWAP_V2_DATA: MemoryId = MemoryId::new(9);
const MEM_LIQ_V2_INDEX: MemoryId = MemoryId::new(10);
const MEM_LIQ_V2_DATA: MemoryId = MemoryId::new(11);
const MEM_ADMIN_EV_INDEX: MemoryId = MemoryId::new(12);
const MEM_ADMIN_EV_DATA: MemoryId = MemoryId::new(13);
const MEM_VP_SNAP_INDEX: MemoryId = MemoryId::new(14);
const MEM_VP_SNAP_DATA: MemoryId = MemoryId::new(15);
const MEM_BLOCKS_INDEX: MemoryId = MemoryId::new(16);
const MEM_BLOCKS_DATA: MemoryId = MemoryId::new(17);

// ─── SlimState ───────────────────────────────────────────────────────────────
//
// Bounded residual heap state. Written to MemoryId 0 via StableCell. Does NOT
// hold any collections — all of those live in their own stable structures.
//
// `storage_migrated` is the one-shot drain flag: false before Phase A has
// run its drain, true after. Once true the drain path in `post_upgrade` is
// never entered again.

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct SlimState {
    pub config: PoolConfig,
    pub balances: [u128; 3],
    pub admin_fees: [u128; 3],
    pub lp_total_supply: u128,
    pub lp_tx_count: u64,
    pub last_block_hash: Option<[u8; 32]>,
    pub is_paused: bool,
    pub is_initialized: bool,
    /// True once the one-shot drain from the legacy raw-offset-0 blob into
    /// stable structures has completed. Set inside the drain path in
    /// `storage::migration::run_drain_if_needed`.
    pub storage_migrated: bool,
}

impl Default for SlimState {
    fn default() -> Self {
        use crate::types::{FeeCurveParams, TokenConfig};
        let default_token = TokenConfig {
            ledger_id: Principal::anonymous(),
            symbol: String::new(),
            decimals: 0,
            precision_mul: 1,
        };
        Self {
            config: PoolConfig {
                tokens: [default_token.clone(), default_token.clone(), default_token],
                initial_a: 100,
                future_a: 100,
                initial_a_time: 0,
                future_a_time: 0,
                swap_fee_bps: 4,
                admin_fee_bps: 5000,
                admin: Principal::anonymous(),
                fee_curve: Some(FeeCurveParams::default()),
            },
            balances: [0; 3],
            admin_fees: [0; 3],
            lp_total_supply: 0,
            lp_tx_count: 0,
            last_block_hash: None,
            is_paused: false,
            is_initialized: false,
            storage_migrated: false,
        }
    }
}

impl Storable for SlimState {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(Encode!(self).expect("SlimState encode"))
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        Decode!(bytes.as_ref(), SlimState).expect("SlimState decode")
    }

    const BOUND: Bound = Bound::Unbounded;
}

// ─── Storable wrappers ───────────────────────────────────────────────────────
//
// StableBTreeMap keys/values and StableLog entries must implement `Storable`.
// We Candid-encode every variable-size payload (events, blocks, allowances)
// and mark them `Bound::Unbounded`. Principal-keyed maps use a fixed-size
// 29-byte wrapper so BTreeMap can use a bounded node layout.

/// Bounded Principal wrapper for BTreeMap keys. Principal is at most 29 bytes.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StorablePrincipal(pub Principal);

impl Storable for StorablePrincipal {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(self.0.as_slice().to_vec())
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        StorablePrincipal(Principal::from_slice(bytes.as_ref()))
    }

    const BOUND: Bound = Bound::Bounded {
        max_size: 29,
        is_fixed_size: false,
    };
}

/// Composite key `(owner, spender)` for the allowance map. Stored as the
/// concatenation of two principals with a length prefix so BTreeMap can
/// order them deterministically.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AllowanceKey {
    pub owner: Principal,
    pub spender: Principal,
}

impl Storable for AllowanceKey {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let owner = self.owner.as_slice();
        let spender = self.spender.as_slice();
        // Layout: [owner_len: u8][owner bytes][spender_len: u8][spender bytes]
        let mut out = Vec::with_capacity(2 + owner.len() + spender.len());
        out.push(owner.len() as u8);
        out.extend_from_slice(owner);
        out.push(spender.len() as u8);
        out.extend_from_slice(spender);
        Cow::Owned(out)
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        let b = bytes.as_ref();
        let owner_len = b[0] as usize;
        let owner = Principal::from_slice(&b[1..1 + owner_len]);
        let spender_len = b[1 + owner_len] as usize;
        let start = 2 + owner_len;
        let spender = Principal::from_slice(&b[start..start + spender_len]);
        AllowanceKey { owner, spender }
    }

    const BOUND: Bound = Bound::Bounded {
        max_size: 60, // 2 length bytes + 2 * 29 principal bytes
        is_fixed_size: false,
    };
}

/// u128 stored as 16-byte little-endian. Used for LP balances.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StorableU128(pub u128);

impl Storable for StorableU128 {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(self.0.to_le_bytes().to_vec())
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        let mut arr = [0u8; 16];
        arr.copy_from_slice(bytes.as_ref());
        StorableU128(u128::from_le_bytes(arr))
    }

    const BOUND: Bound = Bound::Bounded {
        max_size: 16,
        is_fixed_size: true,
    };
}

/// Empty marker for set-style BTreeMaps (`BTreeMap<K, ()>` isn't supported
/// directly because `()` would need a Storable impl we don't control).
#[derive(Clone, Copy, Debug, Default)]
pub struct Unit;

impl Storable for Unit {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Borrowed(&[])
    }

    fn from_bytes(_: Cow<'_, [u8]>) -> Self {
        Unit
    }

    const BOUND: Bound = Bound::Bounded {
        max_size: 0,
        is_fixed_size: true,
    };
}

/// Helper macro that implements Storable for a Candid-serializable type with
/// unbounded size. We use this for all event and block payloads.
macro_rules! impl_storable_candid_unbounded {
    ($t:ty) => {
        impl Storable for $t {
            fn to_bytes(&self) -> Cow<'_, [u8]> {
                Cow::Owned(Encode!(self).expect(concat!("encode ", stringify!($t))))
            }

            fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
                Decode!(bytes.as_ref(), $t).expect(concat!("decode ", stringify!($t)))
            }

            const BOUND: Bound = Bound::Unbounded;
        }
    };
}

impl_storable_candid_unbounded!(SwapEventV1);
impl_storable_candid_unbounded!(LiquidityEventV1);
impl_storable_candid_unbounded!(SwapEventV2);
impl_storable_candid_unbounded!(LiquidityEventV2);
impl_storable_candid_unbounded!(ThreePoolAdminEvent);
impl_storable_candid_unbounded!(VirtualPriceSnapshot);
impl_storable_candid_unbounded!(Icrc3Block);
impl_storable_candid_unbounded!(LpAllowance);

// ─── MemoryManager + stable structures (thread-local) ────────────────────────
//
// Everything here is lazy-initialized on first access. The MemoryManager
// reads 3 bytes from physical offset 0 and either loads an existing layout
// (magic bytes `MGR`) or initializes a new one (destructive — see migration
// module for the safety argument).

thread_local! {
    static MM: RefCell<MemoryManager<DefaultMemoryImpl>> =
        RefCell::new(MemoryManager::init(DefaultMemoryImpl::default()));

    pub(crate) static SLIM_STATE: RefCell<StableCell<SlimState, Memory>> = RefCell::new(
        StableCell::init(MM.with(|m| m.borrow().get(MEM_SLIM_STATE)), SlimState::default())
            .expect("init SlimState cell"),
    );

    pub(crate) static LP_BALANCES: RefCell<StableBTreeMap<StorablePrincipal, StorableU128, Memory>> =
        RefCell::new(StableBTreeMap::init(MM.with(|m| m.borrow().get(MEM_LP_BALANCES))));

    pub(crate) static LP_ALLOWANCES: RefCell<StableBTreeMap<AllowanceKey, LpAllowance, Memory>> =
        RefCell::new(StableBTreeMap::init(MM.with(|m| m.borrow().get(MEM_LP_ALLOWANCES))));

    pub(crate) static BURN_CALLERS: RefCell<StableBTreeMap<StorablePrincipal, Unit, Memory>> =
        RefCell::new(StableBTreeMap::init(MM.with(|m| m.borrow().get(MEM_BURN_CALLERS))));

    pub(crate) static SWAP_V1_LOG: RefCell<StableLog<SwapEventV1, Memory, Memory>> =
        RefCell::new(
            StableLog::init(
                MM.with(|m| m.borrow().get(MEM_SWAP_V1_INDEX)),
                MM.with(|m| m.borrow().get(MEM_SWAP_V1_DATA)),
            )
            .expect("init swap_v1 log"),
        );

    pub(crate) static LIQ_V1_LOG: RefCell<StableLog<LiquidityEventV1, Memory, Memory>> =
        RefCell::new(
            StableLog::init(
                MM.with(|m| m.borrow().get(MEM_LIQ_V1_INDEX)),
                MM.with(|m| m.borrow().get(MEM_LIQ_V1_DATA)),
            )
            .expect("init liq_v1 log"),
        );

    pub(crate) static SWAP_V2_LOG: RefCell<StableLog<SwapEventV2, Memory, Memory>> =
        RefCell::new(
            StableLog::init(
                MM.with(|m| m.borrow().get(MEM_SWAP_V2_INDEX)),
                MM.with(|m| m.borrow().get(MEM_SWAP_V2_DATA)),
            )
            .expect("init swap_v2 log"),
        );

    pub(crate) static LIQ_V2_LOG: RefCell<StableLog<LiquidityEventV2, Memory, Memory>> =
        RefCell::new(
            StableLog::init(
                MM.with(|m| m.borrow().get(MEM_LIQ_V2_INDEX)),
                MM.with(|m| m.borrow().get(MEM_LIQ_V2_DATA)),
            )
            .expect("init liq_v2 log"),
        );

    pub(crate) static ADMIN_EV_LOG: RefCell<StableLog<ThreePoolAdminEvent, Memory, Memory>> =
        RefCell::new(
            StableLog::init(
                MM.with(|m| m.borrow().get(MEM_ADMIN_EV_INDEX)),
                MM.with(|m| m.borrow().get(MEM_ADMIN_EV_DATA)),
            )
            .expect("init admin_ev log"),
        );

    pub(crate) static VP_SNAP_LOG: RefCell<StableLog<VirtualPriceSnapshot, Memory, Memory>> =
        RefCell::new(
            StableLog::init(
                MM.with(|m| m.borrow().get(MEM_VP_SNAP_INDEX)),
                MM.with(|m| m.borrow().get(MEM_VP_SNAP_DATA)),
            )
            .expect("init vp_snap log"),
        );

    pub(crate) static BLOCKS_LOG: RefCell<StableLog<Icrc3Block, Memory, Memory>> =
        RefCell::new(
            StableLog::init(
                MM.with(|m| m.borrow().get(MEM_BLOCKS_INDEX)),
                MM.with(|m| m.borrow().get(MEM_BLOCKS_DATA)),
            )
            .expect("init blocks log"),
        );
}

// ─── Public API: SlimState cell ──────────────────────────────────────────────

/// Read the current slim state by cloning it out of the cell.
///
/// Cheap — SlimState is bounded constant size.
pub fn get_slim() -> SlimState {
    SLIM_STATE.with(|c| c.borrow().get().clone())
}

/// Replace the slim state in the stable cell. Call this whenever bounded
/// heap fields change, OR defer until `pre_upgrade` flushes once. Current
/// design flushes at `pre_upgrade` only; runtime mutations stay in a heap
/// mirror maintained by `state.rs`.
pub fn set_slim(slim: SlimState) {
    SLIM_STATE
        .with(|c| c.borrow_mut().set(slim))
        .expect("set SlimState cell");
}

// ─── Public API: lp_balances ─────────────────────────────────────────────────

pub fn lp_balance_get(p: &Principal) -> u128 {
    LP_BALANCES.with(|m| {
        m.borrow()
            .get(&StorablePrincipal(*p))
            .map(|v| v.0)
            .unwrap_or(0)
    })
}

pub fn lp_balance_set(p: Principal, amount: u128) {
    LP_BALANCES.with(|m| {
        if amount == 0 {
            m.borrow_mut().remove(&StorablePrincipal(p));
        } else {
            m.borrow_mut()
                .insert(StorablePrincipal(p), StorableU128(amount));
        }
    });
}

pub fn lp_balance_len() -> u64 {
    LP_BALANCES.with(|m| m.borrow().len())
}

/// Iterate every (principal, balance) pair. Used by explorer holders endpoint.
pub fn lp_balance_iter() -> Vec<(Principal, u128)> {
    LP_BALANCES.with(|m| {
        m.borrow()
            .iter()
            .map(|(k, v)| (k.0, v.0))
            .collect()
    })
}

// ─── Public API: lp_allowances ───────────────────────────────────────────────

pub fn allowance_get(owner: &Principal, spender: &Principal) -> Option<LpAllowance> {
    LP_ALLOWANCES.with(|m| {
        m.borrow().get(&AllowanceKey {
            owner: *owner,
            spender: *spender,
        })
    })
}

pub fn allowance_set(owner: Principal, spender: Principal, allowance: LpAllowance) {
    LP_ALLOWANCES.with(|m| {
        m.borrow_mut()
            .insert(AllowanceKey { owner, spender }, allowance);
    });
}

pub fn allowance_remove(owner: &Principal, spender: &Principal) {
    LP_ALLOWANCES.with(|m| {
        m.borrow_mut().remove(&AllowanceKey {
            owner: *owner,
            spender: *spender,
        });
    });
}

// ─── Public API: burn_callers ────────────────────────────────────────────────

pub fn burn_caller_contains(p: &Principal) -> bool {
    BURN_CALLERS.with(|m| m.borrow().contains_key(&StorablePrincipal(*p)))
}

pub fn burn_caller_insert(p: Principal) {
    BURN_CALLERS.with(|m| {
        m.borrow_mut().insert(StorablePrincipal(p), Unit);
    });
}

pub fn burn_caller_remove(p: &Principal) {
    BURN_CALLERS.with(|m| {
        m.borrow_mut().remove(&StorablePrincipal(*p));
    });
}

pub fn burn_caller_list() -> Vec<Principal> {
    BURN_CALLERS.with(|m| m.borrow().iter().map(|(k, _)| k.0).collect())
}

// ─── Public API: event logs ──────────────────────────────────────────────────
//
// Each log gets: `push`, `len`, `get` by index, and a range iterator.
// The `_v1` helpers are for the preserved historical logs and should never
// receive new writes after the drain completes; the functions exist only so
// the drain path can populate them and explorer endpoints can read them.

macro_rules! log_api {
    ($mod_name:ident, $cell:ident, $ty:ty) => {
        pub mod $mod_name {
            use super::*;
            pub fn push(entry: $ty) -> u64 {
                $cell.with(|l| l.borrow().append(&entry).expect("stable log append"))
            }
            pub fn len() -> u64 {
                $cell.with(|l| l.borrow().len())
            }
            pub fn get(idx: u64) -> Option<$ty> {
                $cell.with(|l| l.borrow().get(idx))
            }
            /// Collect `[start, start + count)` into a Vec. Out-of-range
            /// indices are skipped.
            pub fn range(start: u64, count: u64) -> Vec<$ty> {
                $cell.with(|l| {
                    let log = l.borrow();
                    let total = log.len();
                    let end = start.saturating_add(count).min(total);
                    (start..end).filter_map(|i| log.get(i)).collect()
                })
            }
            /// Iterate every entry (for full scans like explorer stats).
            pub fn iter_all() -> Vec<$ty> {
                $cell.with(|l| l.borrow().iter().collect())
            }
        }
    };
}

log_api!(swap_v1, SWAP_V1_LOG, SwapEventV1);
log_api!(liq_v1, LIQ_V1_LOG, LiquidityEventV1);
log_api!(swap_v2, SWAP_V2_LOG, SwapEventV2);
log_api!(liq_v2, LIQ_V2_LOG, LiquidityEventV2);
log_api!(admin_ev, ADMIN_EV_LOG, ThreePoolAdminEvent);
log_api!(vp_snap, VP_SNAP_LOG, VirtualPriceSnapshot);
log_api!(blocks, BLOCKS_LOG, Icrc3Block);

// ─── Migration: one-shot drain from the legacy raw-offset-0 blob ─────────────

pub mod migration {
    //! One-shot drain of the legacy pre-Phase-A state layout into the new
    //! stable structures.
    //!
    //! ## Safety argument
    //!
    //! The pre-Phase-A canister wrote its entire `ThreePoolState` as a
    //! length-prefixed Candid blob at physical stable memory offset 0 (see
    //! the old `save_to_stable_memory` / `load_from_stable_memory`). The new
    //! layout requires `MemoryManager::init` at offset 0, which will
    //! unconditionally overwrite those bytes when it fails to find its
    //! `MGR` magic header.
    //!
    //! The drain runs inside a single `post_upgrade` message:
    //!
    //!   1. Read the raw blob from offsets 0/8 into a Rust `Vec<u8>`.
    //!   2. Candid-decode the blob into the legacy `ThreePoolState` shape.
    //!      **At this point every LP balance, event, and block is in RAM.**
    //!   3. Call `MemoryManager::init`. This writes `MGR` magic over the
    //!      first few bytes of stable memory — the on-disk blob is now
    //!      corrupted, but we already have the data in RAM.
    //!   4. Drain every heap collection from the decoded RAM state into its
    //!      new stable structure (LP balances → BTreeMap, events → logs,
    //!      blocks → log).
    //!   5. Write the residual bounded fields to the SlimState cell with
    //!      `storage_migrated = true`.
    //!
    //! If any step traps, the entire `post_upgrade` message is rolled back
    //! by IC upgrade semantics and the canister remains on the old wasm
    //! with the legacy blob intact. The drain is therefore all-or-nothing:
    //! either we end up in the fully migrated state or we roll back cleanly.
    //!
    //! The drain is guarded by `storage_migrated`: on every subsequent
    //! upgrade `post_upgrade` reads `SlimState` from the cell, sees
    //! `storage_migrated = true`, and skips the drain entirely.

    // NOTE: The actual drain implementation lands in A6. This module
    // currently holds only the safety documentation and type signatures so
    // the rest of the codebase can reference `storage::migration::...`
    // without a compile error during A1–A5.

    /// Returns true if the `MGR` magic bytes are present at physical stable
    /// memory offset 0. Callers use this to decide whether to take the drain
    /// path or the normal load path.
    ///
    /// Implementation detail: MemoryManager's magic is `b"MGR"`. We check
    /// this BEFORE calling `MemoryManager::init` because `init` would
    /// destructively write `MGR` over a non-matching prefix.
    pub fn has_memory_manager_magic() -> bool {
        if ic_cdk::api::stable::stable64_size() == 0 {
            return false;
        }
        let mut magic = [0u8; 3];
        ic_cdk::api::stable::stable64_read(0, &mut magic);
        &magic == b"MGR"
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storable_principal_roundtrip() {
        let p = Principal::anonymous();
        let sp = StorablePrincipal(p);
        let bytes = sp.to_bytes();
        let back = StorablePrincipal::from_bytes(bytes);
        assert_eq!(back.0, p);
    }

    #[test]
    fn storable_u128_roundtrip() {
        for v in [0u128, 1, u128::MAX, 1_000_000_000_000_000_000] {
            let su = StorableU128(v);
            let bytes = su.to_bytes();
            let back = StorableU128::from_bytes(bytes);
            assert_eq!(back.0, v);
        }
    }

    #[test]
    fn allowance_key_roundtrip() {
        let owner = Principal::from_text("2vxsx-fae").unwrap();
        let spender = Principal::anonymous();
        let key = AllowanceKey { owner, spender };
        let bytes = key.to_bytes();
        let back = AllowanceKey::from_bytes(bytes);
        assert_eq!(back.owner, owner);
        assert_eq!(back.spender, spender);
    }

    #[test]
    fn slim_state_default_has_migrated_false() {
        let s = SlimState::default();
        assert!(!s.storage_migrated);
        assert_eq!(s.balances, [0; 3]);
        assert_eq!(s.admin_fees, [0; 3]);
    }

    #[test]
    fn slim_state_candid_roundtrip() {
        let mut s = SlimState::default();
        s.lp_total_supply = 260_185_697_420;
        s.balances = [137_225_054_196, 699_518_465, 688_523_829];
        s.admin_fees = [40_219, 0, 0];
        s.storage_migrated = true;
        let bytes = s.to_bytes();
        let back = SlimState::from_bytes(bytes);
        assert_eq!(back.lp_total_supply, 260_185_697_420);
        assert_eq!(back.balances, s.balances);
        assert_eq!(back.admin_fees, s.admin_fees);
        assert!(back.storage_migrated);
    }

    // NOTE: Direct tests of the StableBTreeMap / StableLog accessors need
    // MemoryManager to be initialized over a concrete memory, which the
    // thread_local above does lazily on first touch. Those tests live in
    // `tests/stable_storage.rs` where we can use a dedicated memory.
}
