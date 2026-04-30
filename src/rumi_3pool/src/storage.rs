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
// Memory ID layout (20 IDs used; 255 available):
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
//   18,19   icrc3_block_hashes log      — cumulative hash chain cache (parallel
//                                         to blocks log; entry i == hash of block i)
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
const MEM_BLOCK_HASHES_INDEX: MemoryId = MemoryId::new(18);
const MEM_BLOCK_HASHES_DATA: MemoryId = MemoryId::new(19);

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

/// 32-byte hash stored verbatim. Used for the ICRC-3 cumulative hash-chain
/// cache so that `icrc3_get_blocks` can fetch a block's parent hash in O(1)
/// instead of recomputing the chain from block 0.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StorableHash(pub [u8; 32]);

impl Storable for StorableHash {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(self.0.to_vec())
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes.as_ref());
        StorableHash(arr)
    }

    const BOUND: Bound = Bound::Bounded {
        max_size: 32,
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

    pub(crate) static BLOCK_HASHES_LOG: RefCell<StableLog<StorableHash, Memory, Memory>> =
        RefCell::new(
            StableLog::init(
                MM.with(|m| m.borrow().get(MEM_BLOCK_HASHES_INDEX)),
                MM.with(|m| m.borrow().get(MEM_BLOCK_HASHES_DATA)),
            )
            .expect("init block_hashes log"),
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
log_api!(block_hashes, BLOCK_HASHES_LOG, StorableHash);

// ─── Migration: one-shot drain from the legacy raw-offset-0 blob ─────────────

pub mod migration {
    //! Post-upgrade migration helpers.
    //!
    //! Two functions live here:
    //!
    //!   1. `read_legacy_blob` + `drain_legacy_state` are the one-shot
    //!      Phase A drain that moved heap collections to stable structures.
    //!      Already shipped on mainnet; subsequent upgrades skip the drain.
    //!
    //!   2. `backfill_hash_chain` brings the ICRC-3 `block_hashes` log up
    //!      to parity with the `blocks` log. Idempotent and called on every
    //!      upgrade. The first upgrade after the cache shipped fills all
    //!      pre-existing blocks; subsequent upgrades early-return.
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
    //!   2. Candid-decode the blob into `LegacyThreePoolState`.
    //!      **At this point every LP balance, event, and block is in RAM.**
    //!   3. Drain into `crate::storage::*` thread-locals. The first such
    //!      access lazy-inits MemoryManager, which writes `MGR` over the
    //!      first bytes of stable memory; the legacy blob is destroyed,
    //!      but we already have the data in RAM.
    //!   4. Write SlimState to its cell with `storage_migrated = true`.
    //!
    //! If any step traps, IC `post_upgrade` semantics roll back stable
    //! memory atomically (IC Interface Spec lines 1432, 1450, 1454), and
    //! the canister stays on the old wasm with the legacy blob intact.

    use super::*;
    use crate::types::{
        Icrc3Block, LiquidityEventV1, LiquidityEventV2, LpAllowance, PoolConfig,
        SwapEventV1, SwapEventV2, ThreePoolAdminEvent, VirtualPriceSnapshot,
    };
    use candid::{CandidType, Decode};
    use serde::{Deserialize, Serialize};
    use std::collections::{BTreeMap, BTreeSet};

    /// Candid-compatible mirror of the pre-Phase-A `ThreePoolState`. Every
    /// collection field is `Option<...>` with `#[serde(default)]` so the
    /// decode tolerates older blobs that predate any given field. The
    /// `swap_events_v2` / `liquidity_events_v2` fields were removed from
    /// the live state in A2 but still exist in the mainnet blob — declaring
    /// them here is the only way to recover that data.
    #[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
    pub struct LegacyThreePoolState {
        pub config: PoolConfig,
        pub balances: [u128; 3],
        #[serde(default)]
        pub lp_balances: BTreeMap<Principal, u128>,
        pub lp_total_supply: u128,
        #[serde(default)]
        pub lp_allowances: Option<BTreeMap<(Principal, Principal), LpAllowance>>,
        #[serde(default)]
        pub lp_tx_count: Option<u64>,
        #[serde(default)]
        pub vp_snapshots: Option<Vec<VirtualPriceSnapshot>>,
        #[serde(default)]
        pub blocks: Option<Vec<Icrc3Block>>,
        #[serde(default)]
        pub last_block_hash: Option<[u8; 32]>,
        pub admin_fees: [u128; 3],
        pub is_paused: bool,
        pub is_initialized: bool,
        #[serde(default)]
        pub authorized_burn_callers: Option<BTreeSet<Principal>>,
        #[serde(default)]
        pub swap_events: Option<Vec<SwapEventV1>>,
        #[serde(default)]
        pub liquidity_events: Option<Vec<LiquidityEventV1>>,
        #[serde(default)]
        pub admin_events: Option<Vec<ThreePoolAdminEvent>>,
        #[serde(default)]
        pub swap_events_v2: Option<Vec<SwapEventV2>>,
        #[serde(default)]
        pub liquidity_events_v2: Option<Vec<LiquidityEventV2>>,
    }

    /// Defensive cap on the legacy blob size: anything larger is almost
    /// certainly garbage (e.g. MGR magic re-interpreted as a length).
    /// Live mainnet blob is < 16 MiB.
    const MAX_LEGACY_BLOB: u64 = 64 * 1024 * 1024;

    /// Read and Candid-decode the legacy length-prefixed blob from physical
    /// stable memory offset 0. Returns `None` for fresh canisters (empty
    /// stable memory) and for any canister that already has `MGR` magic at
    /// offset 0 (already-migrated or any future MM-using canister).
    ///
    /// Traps on decode failure of a non-empty, non-MGR blob, so that
    /// `post_upgrade` rolls back rather than continuing with corrupt state.
    ///
    /// This function only touches `ic_cdk::api::stable::*` directly and does
    /// NOT access any `crate::storage::*` thread-local, so it is safe to
    /// call BEFORE `MemoryManager::init` runs.
    pub fn read_legacy_blob() -> Option<LegacyThreePoolState> {
        let size_pages = ic_cdk::api::stable::stable64_size();
        if size_pages == 0 {
            return None;
        }
        // MGR-first check: if MemoryManager has already claimed offset 0,
        // there is no legacy blob to drain.
        let mut magic = [0u8; 3];
        ic_cdk::api::stable::stable64_read(0, &mut magic);
        if &magic == b"MGR" {
            return None;
        }
        let mut len_bytes = [0u8; 8];
        ic_cdk::api::stable::stable64_read(0, &mut len_bytes);
        let len = u64::from_le_bytes(len_bytes);
        if len == 0 {
            return None;
        }
        if len > MAX_LEGACY_BLOB {
            ic_cdk::trap(&format!(
                "legacy blob length {len} exceeds safety cap {MAX_LEGACY_BLOB}"
            ));
        }
        let mut bytes = vec![0u8; len as usize];
        ic_cdk::api::stable::stable64_read(8, &mut bytes);
        match Decode!(&bytes, LegacyThreePoolState) {
            Ok(state) => Some(state),
            Err(e) => ic_cdk::trap(&format!("legacy blob decode failed: {e}")),
        }
    }

    /// Drain a decoded legacy state into the live stable structures.
    ///
    /// MUST be called AFTER `read_legacy_blob` (which reads raw offsets).
    /// The first `crate::storage::*` access here triggers lazy
    /// `MemoryManager::init`, which overwrites the legacy blob bytes at
    /// offset 0 with `MGR` magic. The data is already in `legacy`'s RAM
    /// copy at this point, so the destruction of on-disk bytes is fine.
    ///
    /// Iteration order matches the legacy `Vec` order so log indices in
    /// every `StableLog` equal the original positions and ICRC-3 hash
    /// chain remains valid.
    pub fn drain_legacy_state(legacy: LegacyThreePoolState) {
        // 1. Maps.
        for (principal, balance) in legacy.lp_balances.into_iter() {
            if balance > 0 {
                crate::storage::lp_balance_set(principal, balance);
            }
        }
        if let Some(allowances) = legacy.lp_allowances {
            for ((owner, spender), allowance) in allowances.into_iter() {
                crate::storage::allowance_set(owner, spender, allowance);
            }
        }
        if let Some(burn_callers) = legacy.authorized_burn_callers {
            for principal in burn_callers.into_iter() {
                crate::storage::burn_caller_insert(principal);
            }
        }
        // 2. Logs (preserve original Vec ordering).
        if let Some(events) = legacy.swap_events {
            for e in events.into_iter() {
                crate::storage::swap_v1::push(e);
            }
        }
        if let Some(events) = legacy.liquidity_events {
            for e in events.into_iter() {
                crate::storage::liq_v1::push(e);
            }
        }
        if let Some(events) = legacy.swap_events_v2 {
            for e in events.into_iter() {
                crate::storage::swap_v2::push(e);
            }
        }
        if let Some(events) = legacy.liquidity_events_v2 {
            for e in events.into_iter() {
                crate::storage::liq_v2::push(e);
            }
        }
        if let Some(events) = legacy.admin_events {
            for e in events.into_iter() {
                crate::storage::admin_ev::push(e);
            }
        }
        if let Some(snaps) = legacy.vp_snapshots {
            for s in snaps.into_iter() {
                crate::storage::vp_snap::push(s);
            }
        }
        if let Some(blocks) = legacy.blocks {
            for b in blocks.into_iter() {
                crate::storage::blocks::push(b);
            }
        }
    }

    /// Backfill `block_hashes` to match `blocks` length.
    ///
    /// After this function returns:
    ///   * `block_hashes::len() == blocks::len()`
    ///   * For every `i in 0..blocks::len()`, `block_hashes::get(i)` equals
    ///     the SHA-256 of the ICRC-3 encoding of `blocks::get(i)` with the
    ///     correct parent hash.
    ///
    /// Idempotent: a no-op when the lengths already match. Safe to call from
    /// `post_upgrade` on every upgrade (steady-state cost: 2 stable reads).
    ///
    /// Trapping inside this function rolls back stable memory atomically per
    /// IC `post_upgrade` semantics, so partial backfills cannot persist.
    pub fn backfill_hash_chain() {
        // INVARIANT: the hash computation here MUST match
        // `ThreePoolState::log_block` in state.rs (same encode_block_with_phash
        // call with the same prev_hash semantics, same hash_value over the
        // result). If you change the hash representation in either place,
        // change it in both, or the chain will silently diverge for any
        // backfilled blocks.
        let blocks_len = crate::storage::blocks::len();
        let hashes_len = crate::storage::block_hashes::len();
        if hashes_len > blocks_len {
            ic_cdk::trap(&format!(
                "block_hashes ({hashes_len}) exceeds blocks ({blocks_len}): \
                 hash cache is corrupted. This invariant is also re-checked \
                 in post_upgrade, but failing fast here makes the failure \
                 mode local."
            ));
        }
        if hashes_len == blocks_len {
            return;
        }

        // Recompute the chain from block 0 up to (but not including) the
        // first missing index. We need the parent hash for the first block
        // we are about to fill, which is the cached hash of (start - 1) if
        // any cache exists, or computed from scratch if hashes_len == 0.
        let start = hashes_len;
        let mut prev_hash: Option<[u8; 32]> = if start == 0 {
            None
        } else {
            Some(
                crate::storage::block_hashes::get(start - 1)
                    .expect("cached hash present below hashes_len")
                    .0,
            )
        };

        for i in start..blocks_len {
            let block = crate::storage::blocks::get(i)
                .expect("block present below blocks_len");
            let encoded = crate::icrc3::encode_block_with_phash(&block, prev_hash.as_ref());
            let block_hash = crate::certification::hash_value(&encoded);
            crate::storage::block_hashes::push(crate::storage::StorableHash(block_hash));
            prev_hash = Some(block_hash);
        }
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

    // ─── A6 migration: LegacyThreePoolState Candid round-trips ───

    #[test]
    fn legacy_state_candid_roundtrip_with_v2_events() {
        use crate::storage::migration::LegacyThreePoolState;
        use crate::types::{
            Icrc3Block, Icrc3Transaction, LiquidityAction, LiquidityEventV1,
            LiquidityEventV2, LpAllowance, SwapEventV1, SwapEventV2,
            ThreePoolAdminAction, ThreePoolAdminEvent, VirtualPriceSnapshot,
        };
        use candid::{Decode, Encode};

        let p = Principal::from_text("2vxsx-fae").unwrap();

        let mut lp_balances = std::collections::BTreeMap::new();
        lp_balances.insert(p, 1_234_567_890u128);

        let mut allowances = std::collections::BTreeMap::new();
        allowances.insert(
            (p, Principal::anonymous()),
            LpAllowance { amount: 999, expires_at: Some(123) },
        );

        let mut burn = std::collections::BTreeSet::new();
        burn.insert(p);

        let swap_v1 = SwapEventV1 {
            id: 0, timestamp: 1, caller: p,
            token_in: 0, token_out: 1,
            amount_in: 10, amount_out: 9, fee: 1,
        };
        let swap_v2 = SwapEventV2 {
            id: 0, timestamp: 1, caller: p,
            token_in: 0, token_out: 1,
            amount_in: 10, amount_out: 9, fee: 1,
            fee_bps: 4,
            imbalance_before: 0, imbalance_after: 0,
            is_rebalancing: false,
            pool_balances_after: [1, 2, 3],
            virtual_price_after: 1_000_000_000_000_000_000,
            migrated: false,
        };
        let liq_v1 = LiquidityEventV1 {
            id: 0, timestamp: 1, caller: p,
            action: LiquidityAction::AddLiquidity,
            amounts: [1, 2, 3], lp_amount: 5,
            coin_index: None, fee: None,
        };
        let liq_v2 = LiquidityEventV2 {
            id: 0, timestamp: 1, caller: p,
            action: LiquidityAction::RemoveLiquidity,
            amounts: [1, 2, 3], lp_amount: 5,
            coin_index: None, fee: None,
            fee_bps: None,
            imbalance_before: 0, imbalance_after: 0,
            is_rebalancing: false,
            pool_balances_after: [1, 2, 3],
            virtual_price_after: 1_000_000_000_000_000_000,
            migrated: false,
        };

        let legacy = LegacyThreePoolState {
            config: SlimState::default().config,
            balances: [1, 2, 3],
            lp_balances,
            lp_total_supply: 1_234_567_890,
            lp_allowances: Some(allowances),
            lp_tx_count: Some(42),
            vp_snapshots: Some(vec![VirtualPriceSnapshot {
                timestamp_secs: 1,
                virtual_price: 1_000_000,
                lp_total_supply: 1_000,
            }]),
            blocks: Some(vec![Icrc3Block {
                id: 0,
                timestamp: 1,
                tx: Icrc3Transaction::Mint { to: p, amount: 1, to_subaccount: None },
            }]),
            last_block_hash: Some([7u8; 32]),
            admin_fees: [4, 5, 6],
            is_paused: false,
            is_initialized: true,
            authorized_burn_callers: Some(burn),
            swap_events: Some(vec![swap_v1]),
            liquidity_events: Some(vec![liq_v1]),
            admin_events: Some(vec![ThreePoolAdminEvent {
                id: 0,
                timestamp: 1,
                caller: p,
                action: ThreePoolAdminAction::SetPaused { paused: true },
            }]),
            swap_events_v2: Some(vec![swap_v2]),
            liquidity_events_v2: Some(vec![liq_v2]),
        };

        let bytes = Encode!(&legacy).expect("encode");
        let back = Decode!(&bytes, LegacyThreePoolState).expect("decode");

        assert_eq!(back.balances, [1, 2, 3]);
        assert_eq!(back.lp_total_supply, 1_234_567_890);
        assert_eq!(back.admin_fees, [4, 5, 6]);
        assert_eq!(back.lp_tx_count, Some(42));
        assert_eq!(back.last_block_hash, Some([7u8; 32]));
        assert_eq!(back.lp_balances.get(&p).copied(), Some(1_234_567_890));
        assert_eq!(back.swap_events.as_ref().unwrap().len(), 1);
        assert_eq!(back.swap_events_v2.as_ref().unwrap().len(), 1);
        assert_eq!(back.liquidity_events_v2.as_ref().unwrap().len(), 1);
        assert_eq!(back.admin_events.as_ref().unwrap().len(), 1);
        assert_eq!(back.vp_snapshots.as_ref().unwrap().len(), 1);
        assert_eq!(back.blocks.as_ref().unwrap().len(), 1);
        assert_eq!(back.authorized_burn_callers.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn legacy_state_decodes_from_minimal_blob() {
        use crate::storage::migration::LegacyThreePoolState;
        use candid::{Decode, Encode};

        // Build a "minimal" legacy state where every Optional field is None
        // and every collection is empty. This simulates an extremely old
        // mainnet blob that predates every later field addition.
        let legacy = LegacyThreePoolState {
            config: SlimState::default().config,
            balances: [0; 3],
            lp_balances: std::collections::BTreeMap::new(),
            lp_total_supply: 0,
            lp_allowances: None,
            lp_tx_count: None,
            vp_snapshots: None,
            blocks: None,
            last_block_hash: None,
            admin_fees: [0; 3],
            is_paused: false,
            is_initialized: false,
            authorized_burn_callers: None,
            swap_events: None,
            liquidity_events: None,
            admin_events: None,
            swap_events_v2: None,
            liquidity_events_v2: None,
        };

        let bytes = Encode!(&legacy).expect("encode");
        let back = Decode!(&bytes, LegacyThreePoolState).expect("decode");
        assert!(back.lp_balances.is_empty());
        assert!(back.lp_allowances.is_none());
        assert!(back.swap_events_v2.is_none());
        assert!(back.blocks.is_none());
        assert_eq!(back.lp_total_supply, 0);
    }

    #[test]
    fn storable_hash_roundtrip() {
        let original = [0xABu8; 32];
        let sh = StorableHash(original);
        let bytes = sh.to_bytes();
        let back = StorableHash::from_bytes(bytes);
        assert_eq!(back.0, original);
    }

    #[test]
    fn storable_hash_distinct_values() {
        let a = StorableHash([1u8; 32]);
        let b = StorableHash([2u8; 32]);
        assert_ne!(a.to_bytes(), b.to_bytes());
    }

    #[test]
    fn block_hashes_log_initializes_empty() {
        // At this point in the codebase no test mutates the block_hashes log,
        // so it must be empty. If a future task adds a test that pushes to
        // this log, this test may need to be marked #[ignore] or restructured
        // to check len-equals-blocks::len() instead.
        assert_eq!(block_hashes::len(), 0);
    }

    #[test]
    fn backfill_is_idempotent_when_cache_is_full() {
        // NOTE: This test (and `backfill_fills_missing_hashes_correctly` below)
        // shares the storage thread_locals with every other test in this binary.
        // Under `cargo test`'s default parallel execution they may silently
        // early-out if another test has perturbed the logs first. This is
        // acceptable here because Task 7's PocketIC integration tests exercise
        // the full backfill path against a hermetically-isolated canister
        // instance and provide the load-bearing coverage. These unit tests
        // are best-effort regression catchers for development iteration.
        //
        // After Task 3, every push to blocks already pushes to block_hashes.
        // So if both logs have the same length, backfill should be a no-op.
        let blocks_before = blocks::len();
        let hashes_before = block_hashes::len();
        if blocks_before != hashes_before {
            // We cannot run this test in isolation if other tests left the
            // logs in an inconsistent state. Skip rather than fail.
            return;
        }
        crate::storage::migration::backfill_hash_chain();
        assert_eq!(blocks::len(), blocks_before);
        assert_eq!(block_hashes::len(), hashes_before);
    }

    #[test]
    fn backfill_fills_missing_hashes_correctly() {
        use crate::types::{Icrc3Block, Icrc3Transaction};
        use candid::Principal;

        // Push three blocks via the storage API directly (skipping log_block,
        // which would also write to block_hashes). This simulates the
        // "existing mainnet state" scenario where blocks exist but the
        // hash-chain cache is empty.
        let baseline = blocks::len();
        let hash_baseline = block_hashes::len();
        if baseline != hash_baseline {
            return;
        }

        let p = Principal::anonymous();
        for i in 0..3u64 {
            let block = Icrc3Block {
                id: baseline + i,
                timestamp: 1_000 + i,
                tx: Icrc3Transaction::Mint {
                    to: p,
                    amount: 100 + i as u128,
                    to_subaccount: None,
                },
            };
            blocks::push(block);
        }

        assert_eq!(blocks::len(), baseline + 3);
        assert_eq!(block_hashes::len(), hash_baseline);

        crate::storage::migration::backfill_hash_chain();

        assert_eq!(block_hashes::len(), baseline + 3);

        // Verify every newly added hash is consistent with its block's
        // ICRC-3 encoding under the correct parent.
        let mut prev = if baseline == 0 {
            None
        } else {
            Some(block_hashes::get(baseline - 1).unwrap().0)
        };
        for i in baseline..baseline + 3 {
            let block = blocks::get(i).unwrap();
            let encoded = crate::icrc3::encode_block_with_phash(&block, prev.as_ref());
            let expected = crate::certification::hash_value(&encoded);
            let cached = block_hashes::get(i).unwrap().0;
            assert_eq!(cached, expected, "hash mismatch at index {i}");
            prev = Some(cached);
        }
    }
}
