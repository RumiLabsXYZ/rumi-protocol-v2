use candid::{CandidType, Decode, Encode, Nat, Principal};
use ic_canister_log::log;
use ic_canisters_http_types::{HttpRequest, HttpResponse, HttpResponseBuilder};
use ic_cdk::{init, post_upgrade, pre_upgrade, query, update};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

mod admin;
pub mod analytics;
pub mod icrc21;
mod logs;
pub mod math;
pub mod rewards;
pub mod state;
pub mod transfers;
pub mod types;

use crate::logs::INFO;
use crate::math::{
    compute_initial_lp_shares, compute_proportional_lp_shares, compute_remove_liquidity,
    compute_swap, MINIMUM_LIQUIDITY,
};
use crate::state::{mutate_state, read_state, AmmState};
use crate::types::*;

/// New user-pull routes remain unavailable until each configured ledger has a
/// verified ICRC-2 + archive-aware ICRC-3 profile for bounded replay recovery.
/// LP exits and journaled outbound payout recovery do not use this admission gate.
const AMM_V2_INGRESS_ENABLED: bool = false;

// ─── Per-pool reentrancy guard ───
// Prevents concurrent async operations on the same pool. On IC, messages
// interleave at every `await` point, so without locking two swaps can read
// the same reserves and drain the pool. The guard is released via Drop,
// which runs even if the callback traps (since ic-cdk 0.5.1).

thread_local! {
    static POOL_LOCKS: RefCell<BTreeSet<PoolId>> = RefCell::new(BTreeSet::new());
    static RESERVED_PENDING_CLAIM_SLOTS: Cell<usize> = const { Cell::new(0) };
    #[cfg(feature = "test_endpoints")]
    static TEST_PENDING_CLAIM_LIMIT: Cell<Option<usize>> = const { Cell::new(None) };
}

pub(crate) struct PoolGuard {
    pool_id: PoolId,
}

impl PoolGuard {
    pub(crate) fn new(pool_id: PoolId) -> Result<Self, AmmError> {
        if read_state(|s| {
            s.ingress_operations
                .iter()
                .any(|op| op.pool_id == pool_id && op.phase != AmmIngressPhase::Complete)
        }) {
            return Err(AmmError::PoolBusy);
        }
        Self::lock(pool_id)
    }

    fn new_for_request(
        pool_id: PoolId,
        caller: Principal,
        request_id: [u8; 32],
    ) -> Result<Self, AmmError> {
        let is_recovery = read_state(|s| {
            s.ingress_operations.iter().any(|op| {
                op.pool_id == pool_id
                    && op.caller == caller
                    && op.request_id == request_id
                    && op.phase != AmmIngressPhase::Complete
            })
        });
        let has_other_unresolved = read_state(|s| {
            s.ingress_operations.iter().any(|op| {
                op.pool_id == pool_id
                    && op.phase != AmmIngressPhase::Complete
                    && !(op.caller == caller && op.request_id == request_id)
            })
        });
        if has_other_unresolved
            || (read_state(|s| {
                s.ingress_operations
                    .iter()
                    .any(|op| op.pool_id == pool_id && op.phase != AmmIngressPhase::Complete)
            }) && !is_recovery)
        {
            return Err(AmmError::PoolBusy);
        }
        Self::lock(pool_id)
    }

    fn lock(pool_id: PoolId) -> Result<Self, AmmError> {
        POOL_LOCKS.with(|locks| {
            if !locks.borrow_mut().insert(pool_id.clone()) {
                return Err(AmmError::PoolBusy);
            }
            Ok(Self { pool_id })
        })
    }
}

impl Drop for PoolGuard {
    fn drop(&mut self) {
        POOL_LOCKS.with(|locks| {
            locks.borrow_mut().remove(&self.pool_id);
        });
    }
}

// ─── Supply Cache (not persisted to stable memory) ───

/// icUSD ledger canister ID on mainnet.
pub const ICUSD_LEDGER: &str = "t6bor-paaaa-aaaap-qrd5q-cai";
/// 3pool canister ID on mainnet (also the 3USD token ledger).
const THREEPOOL: &str = "fohh4-yyaaa-aaaap-qtkpa-cai";

/// Per-pool subaccount where reward icUSD is held until claimed.
/// Derived deterministically from the pool ID so the backend can
/// compute it client-side and target the correct subaccount in its
/// mint call.
pub fn reward_subaccount_for(pool_id: &PoolId) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"rumi_amm:rewards:");
    hasher.update(pool_id.as_bytes());
    let digest = hasher.finalize();
    let mut sub = [0u8; 32];
    sub.copy_from_slice(&digest);
    sub
}

/// Query the icUSD ledger for the AMM's reward-subaccount balance.
async fn query_reward_subaccount_balance(pool_id: &PoolId) -> Result<u128, AmmError> {
    use icrc_ledger_types::icrc1::account::Account;
    let icusd_ledger = Principal::from_text(ICUSD_LEDGER).expect("invalid icUSD ledger principal");
    let acct = Account {
        owner: ic_cdk::id(),
        subaccount: Some(reward_subaccount_for(pool_id)),
    };
    let result: Result<(Nat,), _> = ic_cdk::call(icusd_ledger, "icrc1_balance_of", (acct,)).await;
    match result {
        Ok((bal,)) => Ok(bal.0.try_into().unwrap_or(u128::MAX)),
        Err((code, msg)) => Err(AmmError::RewardLedgerTransferFailed {
            reason: format!("balance query rejected: {:?} {}", code, msg),
        }),
    }
}

#[derive(Clone, Default)]
struct SupplyCache {
    total_supply_e8s: u128,
    last_updated_ns: u64,
}

/// Cached icUSD holder balances for incremental snapshot computation.
/// Instead of replaying the entire ledger history on every snapshot,
/// we cache the balance map and last-processed tx index, then only
/// replay new transactions since the last run.
#[derive(Clone, Default)]
struct HolderBalanceCache {
    balances: BTreeMap<Principal, u128>,
    total_supply: u128,
    last_processed_index: u64,
}

thread_local! {
    static SUPPLY_CACHE: RefCell<SupplyCache> = RefCell::new(SupplyCache::default());
    static ICUSD_HOLDER_CACHE: RefCell<HolderBalanceCache> = RefCell::new(HolderBalanceCache::default());
}

fn setup_supply_timer() {
    // Fetch immediately, then every 5 minutes
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(0), || {
        ic_cdk::spawn(refresh_supply());
    });
    ic_cdk_timers::set_timer_interval(std::time::Duration::from_secs(300), || {
        ic_cdk::spawn(refresh_supply());
    });
}

async fn refresh_supply() {
    let ledger = Principal::from_text(ICUSD_LEDGER).expect("invalid icUSD ledger principal");
    match ic_cdk::call::<(), (Nat,)>(ledger, "icrc1_total_supply", ()).await {
        Ok((supply,)) => {
            let supply_u128 = supply.0.try_into().unwrap_or(0u128);
            SUPPLY_CACHE.with(|c| {
                let mut cache = c.borrow_mut();
                cache.total_supply_e8s = supply_u128;
                cache.last_updated_ns = ic_cdk::api::time();
            });
            log!(INFO, "Supply cache refreshed: {} e8s", supply_u128);
        }
        Err((code, msg)) => {
            log!(
                INFO,
                "Failed to fetch icUSD total supply: {:?} {}",
                code,
                msg
            );
        }
    }
}

// ─── Holder Snapshot (daily) ───

/// Types for calling icUSD ledger's get_transactions.
#[derive(CandidType, Deserialize)]
struct LedgerAccount {
    owner: Principal,
    subaccount: Option<serde_bytes::ByteBuf>,
}

#[derive(CandidType, Deserialize)]
struct Mint {
    to: LedgerAccount,
    amount: Nat,
}

#[derive(CandidType, Deserialize)]
struct Burn {
    from: LedgerAccount,
    amount: Nat,
}

#[derive(CandidType, Deserialize)]
struct LedgerTransfer {
    from: LedgerAccount,
    to: LedgerAccount,
    amount: Nat,
    fee: Option<Nat>,
}

#[derive(CandidType, Deserialize)]
struct Transaction {
    kind: String,
    mint: Option<Mint>,
    burn: Option<Burn>,
    transfer: Option<LedgerTransfer>,
}

#[derive(CandidType, Deserialize)]
struct GetTransactionsRequest {
    start: Nat,
    length: Nat,
}

#[derive(CandidType, Deserialize)]
struct GetTransactionsResponse {
    log_length: Nat,
    transactions: Vec<Transaction>,
}

/// Server-side cap on entries returned per event-history page. Matches the
/// SAT-006 `icrc3_get_blocks` bound. Audit 2026-06-09 (DOS-001): these query
/// endpoints previously honored an uncapped caller-supplied length, allowing
/// reply-size DoS.
pub(crate) const MAX_EVENT_PAGE: u64 = 2_000;

/// 24-hour interval in seconds.
const SNAPSHOT_INTERVAL_SECS: u64 = 86_400;
/// Max holders to store per snapshot.
const MAX_SNAPSHOT_HOLDERS: usize = 50;
fn setup_snapshot_timer() {
    // First snapshot 60 seconds after boot (let supply cache warm up first),
    // then every 24 hours.
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(60), || {
        ic_cdk::spawn(take_holder_snapshots());
    });
    ic_cdk_timers::set_timer_interval(
        std::time::Duration::from_secs(SNAPSHOT_INTERVAL_SECS),
        || {
            ic_cdk::spawn(take_holder_snapshots());
        },
    );
}

async fn take_holder_snapshots() {
    log!(INFO, "Starting daily holder snapshot collection...");

    // Collect icUSD holders
    match collect_icusd_holders().await {
        Ok(snapshot) => {
            log!(
                INFO,
                "icUSD snapshot: {} holders, supply {}",
                snapshot.holder_count,
                snapshot.total_supply
            );
            mutate_state(|s| {
                if s.holder_snapshots.len() >= state::MAX_HOLDER_SNAPSHOTS {
                    s.holder_snapshots.remove(0);
                }
                s.holder_snapshots.push(snapshot);
            });
        }
        Err(e) => log!(INFO, "Failed to collect icUSD holder snapshot: {}", e),
    }

    // Collect 3USD holders
    match collect_3usd_holders().await {
        Ok(snapshot) => {
            log!(
                INFO,
                "3USD snapshot: {} holders, supply {}",
                snapshot.holder_count,
                snapshot.total_supply
            );
            mutate_state(|s| {
                if s.holder_snapshots.len() >= state::MAX_HOLDER_SNAPSHOTS {
                    s.holder_snapshots.remove(0);
                }
                s.holder_snapshots.push(snapshot);
            });
        }
        Err(e) => log!(INFO, "Failed to collect 3USD holder snapshot: {}", e),
    }

    log!(INFO, "Holder snapshot collection complete.");
}

/// Incrementally replay new icUSD ledger transactions since the last snapshot.
/// On the first call (cold cache), replays from tx 0. On subsequent calls,
/// only fetches transactions added since `last_processed_index`, saving
/// significant cycles and inter-canister calls as the ledger grows.
async fn collect_icusd_holders() -> Result<HolderSnapshot, String> {
    let ledger = Principal::from_text(ICUSD_LEDGER).map_err(|e| format!("{}", e))?;

    // Load cached state
    let (mut balances, mut total_supply, mut start) = ICUSD_HOLDER_CACHE.with(|c| {
        let cache = c.borrow();
        (
            cache.balances.clone(),
            cache.total_supply,
            cache.last_processed_index,
        )
    });

    let batch_size: u64 = 2000;

    loop {
        let request = GetTransactionsRequest {
            start: Nat::from(start),
            length: Nat::from(batch_size),
        };

        let (response,): (GetTransactionsResponse,) =
            ic_cdk::call(ledger, "get_transactions", (request,))
                .await
                .map_err(|(code, msg)| format!("get_transactions failed: {:?} {}", code, msg))?;

        if response.transactions.is_empty() {
            break;
        }

        for tx in &response.transactions {
            match tx.kind.as_str() {
                "mint" => {
                    if let Some(mint) = &tx.mint {
                        let amount: u128 = mint.amount.0.clone().try_into().unwrap_or(0u128);
                        *balances.entry(mint.to.owner).or_insert(0) += amount;
                        total_supply += amount;
                    }
                }
                "burn" => {
                    if let Some(burn) = &tx.burn {
                        let amount: u128 = burn.amount.0.clone().try_into().unwrap_or(0u128);
                        let entry = balances.entry(burn.from.owner).or_insert(0);
                        *entry = entry.saturating_sub(amount);
                        total_supply = total_supply.saturating_sub(amount);
                    }
                }
                "transfer" => {
                    if let Some(xfer) = &tx.transfer {
                        let amount: u128 = xfer.amount.0.clone().try_into().unwrap_or(0u128);
                        let fee: u128 = xfer
                            .fee
                            .as_ref()
                            .map(|f| f.0.clone().try_into().unwrap_or(0u128))
                            .unwrap_or(0);
                        let from_entry = balances.entry(xfer.from.owner).or_insert(0);
                        *from_entry = from_entry.saturating_sub(amount + fee);
                        *balances.entry(xfer.to.owner).or_insert(0) += amount;
                    }
                }
                _ => {}
            }
        }

        let log_length: u64 = response.log_length.0.clone().try_into().unwrap_or(0u64);
        start += response.transactions.len() as u64;
        if start >= log_length {
            break;
        }
    }

    // Remove zero-balance accounts to prevent unbounded cache growth
    // from addresses that once held tokens but no longer do.
    balances.retain(|_, balance| *balance > 0);

    // Persist cache for next incremental run
    ICUSD_HOLDER_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        cache.balances = balances.clone();
        cache.total_supply = total_supply;
        cache.last_processed_index = start;
    });

    // Sort by balance descending and take top holders
    let mut sorted: Vec<(Principal, u128)> = balances.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    let holder_count = sorted.len() as u64;

    let top_holders: Vec<HolderEntry> = sorted
        .into_iter()
        .take(MAX_SNAPSHOT_HOLDERS)
        .map(|(holder, balance)| HolderEntry { holder, balance })
        .collect();

    Ok(HolderSnapshot {
        token: "icUSD".to_string(),
        timestamp: ic_cdk::api::time(),
        holder_count,
        total_supply,
        top_holders,
    })
}

/// Call the 3pool canister's get_all_lp_holders to get 3USD holder data.
async fn collect_3usd_holders() -> Result<HolderSnapshot, String> {
    let threepool = Principal::from_text(THREEPOOL).map_err(|e| format!("{}", e))?;

    // Get total supply
    let (supply,): (Nat,) = ic_cdk::call(threepool, "icrc1_total_supply", ())
        .await
        .map_err(|(code, msg)| format!("icrc1_total_supply failed: {:?} {}", code, msg))?;
    let total_supply: u128 = supply.0.try_into().unwrap_or(0u128);

    // Get all holders (already sorted by balance descending from the 3pool)
    let (holders,): (Vec<(Principal, u128)>,) = ic_cdk::call(threepool, "get_all_lp_holders", ())
        .await
        .map_err(|(code, msg)| format!("get_all_lp_holders failed: {:?} {}", code, msg))?;

    let holder_count = holders.len() as u64;

    let top_holders: Vec<HolderEntry> = holders
        .into_iter()
        .take(MAX_SNAPSHOT_HOLDERS)
        .map(|(holder, balance)| HolderEntry { holder, balance })
        .collect();

    Ok(HolderSnapshot {
        token: "3USD".to_string(),
        timestamp: ic_cdk::api::time(),
        holder_count,
        total_supply,
        top_holders,
    })
}

// ─── TVL Sampler (added 2026-05-09 for amm1ApyService) ───

const TVL_SAMPLE_INTERVAL_SECS: u64 = 6 * 3600; // 4 samples/day

fn setup_tvl_sample_timer() {
    // First sample 90s after boot (let supply cache + protocol_backend_principal config arrive).
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(90), || {
        ic_cdk::spawn(sample_tvl_for_all_pools());
    });
    ic_cdk_timers::set_timer_interval(
        std::time::Duration::from_secs(TVL_SAMPLE_INTERVAL_SECS),
        || ic_cdk::spawn(sample_tvl_for_all_pools()),
    );
}

async fn sample_tvl_for_all_pools() {
    // Price source: ICP/USD comes from rumi_protocol_backend.get_icp_usd_price_e8s.
    // 3USD assumed at $1.00 (peg). Documented limitation: if the peg ever depegs,
    // this sample is inaccurate.
    let pool_ids: Vec<PoolId> = read_state(|s| s.pools.keys().cloned().collect());
    if pool_ids.is_empty() {
        return;
    }

    let icp_price = match fetch_icp_price_e8s().await {
        Ok(p) if p > 0 => p,
        Ok(_) => {
            log!(INFO, "[tvl_sample] icp price returned 0; skipping sample");
            return;
        }
        Err(e) => {
            log!(
                INFO,
                "[tvl_sample] icp price fetch failed: {}; skipping sample",
                e
            );
            return;
        }
    };

    let three_usd_price_e8s: u128 = 100_000_000; // $1.00 in e8s

    for pool_id in pool_ids {
        mutate_state(|s| {
            let pool = match s.pools.get(&pool_id) {
                Some(p) => p,
                None => return,
            };
            // tvl_usd_e8s = reserve_a * price_a / 1e8 + reserve_b * price_b / 1e8
            let tvl_a = pool.reserve_a.saturating_mul(three_usd_price_e8s) / 100_000_000;
            let tvl_b = pool.reserve_b.saturating_mul(icp_price) / 100_000_000;
            let tvl_usd_e8s = tvl_a.saturating_add(tvl_b);

            let sample = TvlSample {
                pool_id: pool_id.clone(),
                timestamp: ic_cdk::api::time(),
                reserve_a: pool.reserve_a,
                reserve_b: pool.reserve_b,
                price_a_e8s: three_usd_price_e8s,
                price_b_e8s: icp_price,
                tvl_usd_e8s,
            };
            s.tvl_samples.push(sample);
            while s.tvl_samples.len() > crate::state::MAX_TVL_SAMPLES {
                s.tvl_samples.remove(0);
            }
        });
    }
}

async fn fetch_icp_price_e8s() -> Result<u128, String> {
    // Re-uses protocol_backend_principal as the source for the price query.
    // If the backend hasn't been configured yet, or it doesn't yet have the
    // get_icp_usd_price_e8s query (deploys can land in either order), the
    // call fails gracefully and the sampler skips this tick.
    let backend = read_state(|s| s.protocol_backend_principal)
        .ok_or_else(|| "protocol_backend_principal not configured".to_string())?;
    let result: Result<(ProtocolStatusLite,), _> =
        ic_cdk::call(backend, "get_icp_usd_price_e8s", ()).await;
    match result {
        Ok((p,)) => Ok(p.price_e8s),
        Err((code, msg)) => Err(format!("call failed: {:?} {}", code, msg)),
    }
}

// ─── Init / Upgrade ───

#[init]
fn init(args: AmmInitArgs) {
    // UPG-006: refuse to init with non-empty stable memory. Catches accidental
    // reinstalls of a canister that already has persisted state. Reinstall mode
    // wipes stable memory before init runs (per IC spec), so this primarily
    // documents intent and guards against future IC behavior changes.
    assert!(
        ic_cdk::api::stable::stable64_size() == 0,
        "refusing to init: stable memory non-empty; use upgrade mode not reinstall"
    );
    mutate_state(|s| s.initialize(args));
    setup_supply_timer();
    setup_snapshot_timer();
    setup_tvl_sample_timer();
    log!(
        INFO,
        "Rumi AMM initialized. Admin: {}",
        read_state(|s| s.admin)
    );
}

#[pre_upgrade]
fn pre_upgrade() {
    log!(INFO, "Rumi AMM pre-upgrade: saving state");
    state::save_to_stable_memory();
}

/// On upgrade, state is restored from stable memory. The `_args` parameter is
/// accepted for Candid interface compatibility with `init` but intentionally
/// ignored; the admin and all other config come from persisted state.
#[post_upgrade]
fn post_upgrade(_args: AmmInitArgs) {
    state::load_from_stable_memory();
    setup_supply_timer();
    setup_snapshot_timer();
    setup_tvl_sample_timer();
    log!(
        INFO,
        "Rumi AMM post-upgrade: state restored. {} pools, {} snapshots",
        read_state(|s| s.pools.len()),
        read_state(|s| s.holder_snapshots.len())
    );
}

// ─── Helpers ───

pub(crate) fn caller_is_admin() -> Result<(), AmmError> {
    let caller = ic_cdk::caller();
    let admin = read_state(|s| s.admin);
    if caller != admin {
        return Err(AmmError::Unauthorized);
    }
    Ok(())
}

fn reject_anonymous() -> Result<(), AmmError> {
    if ic_cdk::caller() == Principal::anonymous() {
        return Err(AmmError::Unauthorized);
    }
    Ok(())
}

/// Derive a deterministic 32-byte subaccount from a pool ID and token label.
pub(crate) fn derive_subaccount(pool_id: &str, token_label: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(pool_id.as_bytes());
    hasher.update(b"_");
    hasher.update(token_label.as_bytes());
    let result = hasher.finalize();
    let mut sub = [0u8; 32];
    sub.copy_from_slice(&result);
    sub
}

/// Build pool ID from two token principals (sorted for determinism).
pub(crate) fn make_pool_id(token_a: Principal, token_b: Principal) -> PoolId {
    let a = token_a.to_text();
    let b = token_b.to_text();
    if a <= b {
        format!("{}_{}", a, b)
    } else {
        format!("{}_{}", b, a)
    }
}

/// Record a failed outbound transfer as a pending claim so the user can retry.
fn record_pending_claim(
    slots: &mut PendingClaimSlots,
    pool_id: &PoolId,
    claimant: Principal,
    token: Principal,
    subaccount: [u8; 32],
    amount: u128,
    reason: &str,
) -> u64 {
    slots.consume_one();
    mutate_state(|s| {
        let id = s.next_claim_id;
        s.next_claim_id += 1;
        s.pending_claims.push(PendingClaim {
            id,
            pool_id: pool_id.clone(),
            claimant,
            token,
            subaccount,
            amount,
            reason: reason.to_string(),
            created_at: ic_cdk::api::time() / 1_000_000_000,
        });
        log!(INFO, "Pending claim #{} recorded: {} owes {} of token {} (pool {})",
            id, claimant, amount, token, pool_id);
        id
    })
}

/// Persist an outbound obligation before any ledger call. Exact transfer
/// arguments are completed after the fee query and then frozen across retries.
fn create_payout_attempt(
    operation_id: u64,
    pool_id: &PoolId,
    claimant: Principal,
    ledger: Principal,
    subaccount: [u8; 32],
    gross_amount: u128,
) -> Result<u64, AmmError> {
    mutate_state(|s| {
        if s.pending_payouts.len() >= state::MAX_PENDING_CLAIMS {
            return Err(AmmError::TransferFailed {
                token: "payout_capacity".to_string(),
                reason: "pending payout journal is full; refusing an unjournaled transfer"
                    .to_string(),
            });
        }
        let id = s.next_payout_id;
        s.next_payout_id = id.checked_add(1).ok_or_else(|| AmmError::TransferFailed {
            token: "payout_id".to_string(),
            reason: "payout identity space exhausted".to_string(),
        })?;
        s.pending_payouts.push(new_payout_row(
            id,
            operation_id,
            pool_id,
            claimant,
            ledger,
            subaccount,
            gross_amount,
        ));
        Ok(id)
    })
}

fn pending_claim_limit() -> usize {
    #[cfg(feature = "test_endpoints")]
    if let Some(limit) = TEST_PENDING_CLAIM_LIMIT.with(Cell::get) {
        return limit;
    }
    state::MAX_PENDING_CLAIMS
}

/// Capacity reservation held over any await that may turn accepted value into
/// a pending claim. Reservations are global because claims span all pools.
struct PendingClaimSlots {
    remaining: usize,
}

impl PendingClaimSlots {
    fn reserve(slots: usize) -> Result<Self, AmmError> {
        let accepted = RESERVED_PENDING_CLAIM_SLOTS.with(|reserved| {
            let current = reserved.get();
            let Some(total) = read_state(|s| s.pending_claims.len())
                .checked_add(current)
                .and_then(|n| n.checked_add(slots))
            else {
                return false;
            };
            if total > pending_claim_limit() {
                return false;
            }
            reserved.set(current + slots);
            true
        });
        if !accepted {
            return Err(AmmError::PendingClaimCapacityReached);
        }
        Ok(Self { remaining: slots })
    }

    /// A pending claim being paid is temporarily removed from the vector.
    /// Keep its occupied slot reserved so another pool cannot consume it.
    fn hold_existing() -> Self {
        RESERVED_PENDING_CLAIM_SLOTS.with(|reserved| reserved.set(reserved.get() + 1));
        Self { remaining: 1 }
    }

    fn consume_one(&mut self) {
        assert!(self.remaining > 0, "pending claim inserted without reserved capacity");
        self.remaining -= 1;
        RESERVED_PENDING_CLAIM_SLOTS.with(|reserved| reserved.set(reserved.get() - 1));
    }
}

impl Drop for PendingClaimSlots {
    fn drop(&mut self) {
        RESERVED_PENDING_CLAIM_SLOTS.with(|reserved| reserved.set(reserved.get() - self.remaining));
    }
}

fn new_payout_row(
    id: u64,
    operation_id: u64,
    pool_id: &PoolId,
    claimant: Principal,
    ledger: Principal,
    subaccount: [u8; 32],
    gross_amount: u128,
) -> AmmPayoutAttempt {
    let memo = payout_memo(id, 0);
    AmmPayoutAttempt {
        id,
        operation_id,
        pool_id: pool_id.clone(),
        claimant,
        ledger,
        subaccount,
        gross_amount,
        send_amount: None,
        fee: None,
        memo,
        created_at_time: ic_cdk::api::time(),
        attempt_generation: 0,
        dispatch_count: 0,
        phase: AmmPayoutPhase::Staged,
        receipt_scan_start: None,
        receipt_scan_cursor: 0,
        receipt_scan_end: None,
        last_error: None,
    }
}

fn payout_memo(id: u64, generation: u32) -> Vec<u8> {
    let mut memo = b"RUMI-AMM-PAYOUT:".to_vec();
    memo.extend_from_slice(&id.to_be_bytes());
    memo.extend_from_slice(&generation.to_be_bytes());
    memo
}

fn ingress_memo(
    caller: Principal,
    request_id: [u8; 32],
    leg_index: u8,
    generation: u32,
) -> Vec<u8> {
    let mut memo = b"RUMI-AMM-INGRESS:".to_vec();
    memo.extend_from_slice(caller.as_slice());
    memo.extend_from_slice(&request_id);
    memo.push(leg_index);
    memo.extend_from_slice(&generation.to_be_bytes());
    memo
}

fn receipt_scan_page_end(cursor: u64, frozen_tip: u64) -> u64 {
    cursor.saturating_add(32).min(frozen_tip)
}

fn checked_receipt_scan_page_end(cursor: u64, tip: u64) -> Result<u64, String> {
    if cursor > tip {
        Err("ledger log length fell below the persisted receipt-scan cursor".to_string())
    } else {
        Ok(receipt_scan_page_end(cursor, tip))
    }
}

fn receipt_scan_tip(frozen_tip: Option<u64>, observed_log_length: u64) -> Result<u64, String> {
    match frozen_tip {
        Some(tip) if observed_log_length < tip => {
            Err("ledger log length fell below the frozen receipt-scan tip".to_string())
        }
        Some(tip) => Ok(tip),
        None => Ok(observed_log_length),
    }
}

fn sort_and_reject_duplicate_block_ids(
    blocks: &mut Vec<icrc_ledger_types::icrc3::blocks::BlockWithId>,
) -> Result<(), String> {
    blocks.sort_by(|a, b| a.id.cmp(&b.id));
    if blocks.windows(2).any(|pair| pair[0].id == pair[1].id) {
        return Err("ICRC-3 direct/archive responses overlap block IDs; cursor held".to_string());
    }
    Ok(())
}

/// Drop a staged request only after source proves that no external value moved.
/// Such a row has no replay-protection duty and must not consume scarce global
/// identity capacity.
fn discard_unaccepted_ingress(state: &mut AmmState, operation_id: u64) {
    let payout_ids = state
        .ingress_operations
        .iter()
        .find(|op| op.id == operation_id)
        .map(|op| op.payout_ids.clone())
        .unwrap_or_default();
    state.ingress_operations.retain(|op| op.id != operation_id);
    state
        .pending_payouts
        .retain(|row| !payout_ids.contains(&row.id));
}

fn payout_phase_has_prior_ambiguity(phase: &AmmPayoutPhase) -> bool {
    matches!(
        phase,
        AmmPayoutPhase::Submitted | AmmPayoutPhase::HeldUnknown
    )
}

/// Install an ingress request and its first payout identity before any ledger
/// await. A caller may retry the same request id and payload; a different
/// payload under that id or a newer request while one is unresolved is denied.
fn begin_swap_operation(
    caller: Principal,
    request_id: [u8; 32],
    pool_id: &PoolId,
    token_in: Principal,
    amount_in: u128,
    min_amount_out: u128,
    token_out: Principal,
    sub_in: [u8; 32],
    sub_out: [u8; 32],
    amount_out: u128,
    total_fee: u128,
    protocol_fee: u128,
) -> Result<(u64, AmmIngressOperation), AmmError> {
    if u64::from_be_bytes(request_id[..8].try_into().expect("fixed request id")) == 0 {
        return Err(AmmError::InvalidInput {
            reason: "request_id sequence must be nonzero".to_string(),
        });
    }
    mutate_state(|s| {
        if let Some(existing) = s.ingress_operations.iter().find(|op| op.caller == caller) {
            let incoming_counter =
                u64::from_be_bytes(request_id[..8].try_into().expect("fixed request id"));
            let existing_counter = u64::from_be_bytes(
                existing.request_id[..8]
                    .try_into()
                    .expect("fixed request id"),
            );
            if incoming_counter < existing_counter {
                return Err(AmmError::DuplicateNonce);
            }
            if request_id == existing.request_id {
                if existing.pool_id != *pool_id
                    || existing.kind
                        != (AmmIngressKind::Swap {
                            token_in,
                            amount_in,
                            min_amount_out,
                        })
                {
                    return Err(AmmError::DuplicateNonce);
                }
                return Ok((existing.id, existing.clone()));
            }
            if incoming_counter == existing_counter || existing.phase != AmmIngressPhase::Complete {
                return Err(AmmError::TransferFailed {
                    token: "pending_operation".to_string(),
                    reason: format!(
                        "request {:?} must be recovered before starting request {:?}",
                        existing.request_id, request_id
                    ),
                });
            }
            s.ingress_operations.retain(|op| op.caller != caller);
        }
        if s.ingress_operations.len() >= state::MAX_INGRESS_OPERATIONS {
            return Err(AmmError::TransferFailed {
                token: "request_capacity".into(),
                reason: "AMM request replay-defense table is full; refusing new ingress".into(),
            });
        }
        let operation_id = s.next_operation_id;
        s.next_operation_id =
            operation_id
                .checked_add(1)
                .ok_or_else(|| AmmError::TransferFailed {
                    token: "operation_id".to_string(),
                    reason: "operation identity space exhausted".to_string(),
                })?;
        if s.pending_payouts.len() >= state::MAX_PENDING_CLAIMS {
            return Err(AmmError::TransferFailed {
                token: "payout_capacity".to_string(),
                reason: "pending payout journal is full; refusing to pull tokens".to_string(),
            });
        }
        let payout_id = s.next_payout_id;
        s.next_payout_id = payout_id
            .checked_add(1)
            .ok_or_else(|| AmmError::TransferFailed {
                token: "payout_id".to_string(),
                reason: "payout identity space exhausted".to_string(),
            })?;
        let ingress_memo = ingress_memo(caller, request_id, 0, 0);
        let leg = AmmIngressLeg {
            ledger: token_in,
            from: caller,
            to_subaccount: sub_in,
            amount: amount_in,
            memo: ingress_memo,
            created_at_time: ic_cdk::api::time(),
            transfer_fee: None,
            attempt_generation: 0,
            dispatch_count: 0,
            block_index: None,
            receipt_scan_start: None,
            receipt_scan_cursor: 0,
            receipt_scan_end: None,
        };
        s.pending_payouts.push(new_payout_row(
            payout_id,
            operation_id,
            pool_id,
            caller,
            token_out,
            sub_out,
            amount_out,
        ));
        let operation = AmmIngressOperation {
            id: operation_id,
            caller,
            request_id,
            pool_id: pool_id.clone(),
            kind: AmmIngressKind::Swap {
                token_in,
                amount_in,
                min_amount_out,
            },
            legs: vec![leg],
            payout_ids: vec![payout_id],
            confirmed_payout_ids: Vec::new(),
            computed_values: vec![amount_out, total_fee, protocol_fee, 0],
            phase: AmmIngressPhase::Prepared,
            result: None,
            last_error: None,
        };
        s.ingress_operations.push(operation.clone());
        Ok((operation_id, operation))
    })
}

fn begin_add_operation(
    caller: Principal,
    request_id: [u8; 32],
    pool_id: &PoolId,
    amount_a: u128,
    amount_b: u128,
    min_lp_shares: u128,
    token_a: Principal,
    token_b: Principal,
    sub_a: [u8; 32],
    sub_b: [u8; 32],
    shares: u128,
) -> Result<(u64, AmmIngressOperation), AmmError> {
    if u64::from_be_bytes(request_id[..8].try_into().expect("fixed request id")) == 0 {
        return Err(AmmError::InvalidInput {
            reason: "request_id sequence must be nonzero".to_string(),
        });
    }
    mutate_state(|s| {
        if let Some(existing) = s.ingress_operations.iter().find(|op| op.caller == caller) {
            let incoming_counter =
                u64::from_be_bytes(request_id[..8].try_into().expect("fixed request id"));
            let existing_counter = u64::from_be_bytes(
                existing.request_id[..8]
                    .try_into()
                    .expect("fixed request id"),
            );
            if incoming_counter < existing_counter {
                return Err(AmmError::DuplicateNonce);
            }
            if request_id == existing.request_id {
                if existing.pool_id != *pool_id
                    || existing.kind
                        != (AmmIngressKind::AddLiquidity {
                            amount_a,
                            amount_b,
                            min_lp_shares,
                        })
                {
                    return Err(AmmError::DuplicateNonce);
                }
                return Ok((existing.id, existing.clone()));
            }
            if incoming_counter == existing_counter || existing.phase != AmmIngressPhase::Complete {
                return Err(AmmError::TransferFailed {
                    token: "pending_operation".to_string(),
                    reason: "prior AMM request must be recovered before a new request".to_string(),
                });
            }
            s.ingress_operations.retain(|op| op.caller != caller);
        }
        if s.ingress_operations.len() >= state::MAX_INGRESS_OPERATIONS {
            return Err(AmmError::TransferFailed {
                token: "request_capacity".into(),
                reason: "AMM request replay-defense table is full; refusing new ingress".into(),
            });
        }
        if s.pending_payouts.len() >= state::MAX_PENDING_CLAIMS {
            return Err(AmmError::TransferFailed {
                token: "payout_capacity".to_string(),
                reason: "pending payout journal is full; refusing to pull tokens".to_string(),
            });
        }
        let operation_id = s.next_operation_id;
        s.next_operation_id =
            operation_id
                .checked_add(1)
                .ok_or_else(|| AmmError::TransferFailed {
                    token: "operation_id".to_string(),
                    reason: "operation identity space exhausted".to_string(),
                })?;
        let payout_id = s.next_payout_id;
        s.next_payout_id = payout_id
            .checked_add(1)
            .ok_or_else(|| AmmError::TransferFailed {
                token: "payout_id".to_string(),
                reason: "payout identity space exhausted".to_string(),
            })?;
        let memo_a = ingress_memo(caller, request_id, 0, 0);
        let memo_b = ingress_memo(caller, request_id, 1, 0);
        let operation = AmmIngressOperation {
            id: operation_id,
            caller,
            request_id,
            pool_id: pool_id.clone(),
            kind: AmmIngressKind::AddLiquidity {
                amount_a,
                amount_b,
                min_lp_shares,
            },
            legs: vec![
                AmmIngressLeg {
                    ledger: token_a,
                    from: caller,
                    to_subaccount: sub_a,
                    amount: amount_a,
                    memo: memo_a,
                    created_at_time: ic_cdk::api::time(),
                    transfer_fee: None,
                    attempt_generation: 0,
                    dispatch_count: 0,
                    block_index: None,
                    receipt_scan_start: None,
                    receipt_scan_cursor: 0,
                    receipt_scan_end: None,
                },
                AmmIngressLeg {
                    ledger: token_b,
                    from: caller,
                    to_subaccount: sub_b,
                    amount: amount_b,
                    memo: memo_b,
                    created_at_time: ic_cdk::api::time(),
                    transfer_fee: None,
                    attempt_generation: 0,
                    dispatch_count: 0,
                    block_index: None,
                    receipt_scan_start: None,
                    receipt_scan_cursor: 0,
                    receipt_scan_end: None,
                },
            ],
            payout_ids: vec![payout_id],
            confirmed_payout_ids: Vec::new(),
            computed_values: vec![shares],
            phase: AmmIngressPhase::Prepared,
            result: None,
            last_error: None,
        };
        // Reserve the exact token-A refund tuple before either input pull.
        s.pending_payouts.push(new_payout_row(
            payout_id,
            operation_id,
            pool_id,
            caller,
            token_a,
            sub_a,
            amount_a,
        ));
        s.ingress_operations.push(operation.clone());
        Ok((operation_id, operation))
    })
}

fn begin_remove_operation(
    caller: Principal,
    request_id: [u8; 32],
    pool_id: &PoolId,
    lp_shares: u128,
    min_amount_a: u128,
    min_amount_b: u128,
    token_a: Principal,
    token_b: Principal,
    sub_a: [u8; 32],
    sub_b: [u8; 32],
    amount_a: u128,
    amount_b: u128,
) -> Result<(u64, AmmIngressOperation), AmmError> {
    if u64::from_be_bytes(request_id[..8].try_into().expect("fixed request id")) == 0 {
        return Err(AmmError::InvalidInput {
            reason: "request_id sequence must be nonzero".to_string(),
        });
    }
    mutate_state(|s| {
        if let Some(existing) = s.ingress_operations.iter().find(|op| op.caller == caller) {
            let incoming_counter =
                u64::from_be_bytes(request_id[..8].try_into().expect("fixed request id"));
            let existing_counter = u64::from_be_bytes(
                existing.request_id[..8]
                    .try_into()
                    .expect("fixed request id"),
            );
            if incoming_counter < existing_counter {
                return Err(AmmError::DuplicateNonce);
            }
            if request_id == existing.request_id {
                if existing.pool_id != *pool_id
                    || existing.kind
                        != (AmmIngressKind::RemoveLiquidity {
                            lp_shares,
                            min_amount_a,
                            min_amount_b,
                        })
                {
                    return Err(AmmError::DuplicateNonce);
                }
                return Ok((existing.id, existing.clone()));
            }
            if incoming_counter == existing_counter || existing.phase != AmmIngressPhase::Complete {
                return Err(AmmError::TransferFailed {
                    token: "pending_operation".to_string(),
                    reason: "prior AMM request must be recovered before a new request".to_string(),
                });
            }
            s.ingress_operations.retain(|op| op.caller != caller);
        }
        if s.ingress_operations.len() >= state::MAX_INGRESS_OPERATIONS {
            return Err(AmmError::TransferFailed {
                token: "request_capacity".into(),
                reason: "AMM request replay-defense table is full; refusing new request".into(),
            });
        }
        let payout_count = usize::from(amount_a > 0) + usize::from(amount_b > 0);
        if s.pending_payouts.len().saturating_add(payout_count) > state::MAX_PENDING_CLAIMS {
            return Err(AmmError::TransferFailed {
                token: "payout_capacity".to_string(),
                reason: "pending payout journal is full; refusing to burn LP shares".to_string(),
            });
        }
        let operation_id = s.next_operation_id;
        s.next_operation_id =
            operation_id
                .checked_add(1)
                .ok_or_else(|| AmmError::TransferFailed {
                    token: "operation_id".into(),
                    reason: "identity space exhausted".into(),
                })?;
        let mut payout_ids = Vec::new();
        if amount_a > 0 {
            let id = s.next_payout_id;
            s.next_payout_id = id.checked_add(1).ok_or_else(|| AmmError::TransferFailed {
                token: "payout_id".into(),
                reason: "identity space exhausted".into(),
            })?;
            s.pending_payouts.push(new_payout_row(
                id,
                operation_id,
                pool_id,
                caller,
                token_a,
                sub_a,
                amount_a,
            ));
            payout_ids.push(id);
        }
        if amount_b > 0 {
            let id = s.next_payout_id;
            s.next_payout_id = id.checked_add(1).ok_or_else(|| AmmError::TransferFailed {
                token: "payout_id".into(),
                reason: "identity space exhausted".into(),
            })?;
            s.pending_payouts.push(new_payout_row(
                id,
                operation_id,
                pool_id,
                caller,
                token_b,
                sub_b,
                amount_b,
            ));
            payout_ids.push(id);
        }
        let operation = AmmIngressOperation {
            id: operation_id,
            caller,
            request_id,
            pool_id: pool_id.clone(),
            kind: AmmIngressKind::RemoveLiquidity {
                lp_shares,
                min_amount_a,
                min_amount_b,
            },
            legs: Vec::new(),
            payout_ids,
            confirmed_payout_ids: Vec::new(),
            computed_values: vec![amount_a, amount_b],
            phase: AmmIngressPhase::Prepared,
            result: None,
            last_error: None,
        };
        s.ingress_operations.push(operation.clone());
        Ok((operation_id, operation))
    })
}

async fn process_ingress_leg(operation_id: u64, leg_index: usize) -> Result<u64, AmmError> {
    let operation = read_state(|s| {
        s.ingress_operations
            .iter()
            .find(|op| op.id == operation_id)
            .cloned()
    })
    .ok_or(AmmError::ClaimNotFound)?;
    let mut leg = operation
        .legs
        .get(leg_index)
        .cloned()
        .ok_or(AmmError::ClaimNotFound)?;
    if let Some(block) = leg.block_index {
        return Ok(block);
    }
    if leg.dispatch_count == 0 && leg.receipt_scan_start.is_none() {
        let start =
            ledger_receipt_tip(leg.ledger)
                .await
                .map_err(|reason| AmmError::TransferFailed {
                    token: format!("ingress:{}", leg_index),
                    reason,
                })?;
        let saved = mutate_state(|s| {
            if let Some(op) = s
                .ingress_operations
                .iter_mut()
                .find(|op| op.id == operation_id && **op == operation)
            {
                if let Some(stored) = op.legs.get_mut(leg_index) {
                    if *stored == leg {
                        stored.receipt_scan_start = Some(start);
                        stored.receipt_scan_cursor = start;
                        return true;
                    }
                }
            }
            false
        });
        if !saved {
            return Err(AmmError::TransferFailed {
                token: format!("ingress:{}", leg_index),
                reason: "ingress changed while capturing pre-dispatch ledger tip".to_string(),
            });
        }
        leg.receipt_scan_start = Some(start);
        leg.receipt_scan_cursor = start;
    }
    if leg.transfer_fee.is_none() {
        let fee = crate::transfers::ledger_fee(leg.ledger).await;
        let saved = mutate_state(|s| {
            if let Some(op) = s
                .ingress_operations
                .iter_mut()
                .find(|op| op.id == operation_id)
            {
                if let Some(stored) = op.legs.get_mut(leg_index) {
                    if *stored == leg {
                        stored.transfer_fee = Some(fee);
                        return true;
                    }
                }
            }
            false
        });
        if !saved {
            return Err(AmmError::TransferFailed {
                token: format!("ingress:{}", leg_index),
                reason: "ingress changed while persisting exact transfer fee".to_string(),
            });
        }
        leg.transfer_fee = Some(fee);
    }
    let prior_dispatches = mutate_state(|s| {
        if let Some(op) = s
            .ingress_operations
            .iter_mut()
            .find(|op| op.id == operation_id)
        {
            op.phase = AmmIngressPhase::Pulling {
                leg_index: leg_index as u32,
            };
            if let Some(stored_leg) = op.legs.get_mut(leg_index) {
                let prior = stored_leg.dispatch_count;
                stored_leg.dispatch_count = stored_leg.dispatch_count.saturating_add(1);
                return prior;
            }
        }
        u32::MAX
    });
    match crate::transfers::transfer_from_user_exact(&leg).await {
        Ok(block) => {
            mutate_state(|s| {
                if let Some(op) = s
                    .ingress_operations
                    .iter_mut()
                    .find(|op| op.id == operation_id)
                {
                    if let Some(stored_leg) = op.legs.get_mut(leg_index) {
                        stored_leg.block_index = Some(block);
                    }
                    op.phase = AmmIngressPhase::Pulled;
                    op.last_error = None;
                }
            });
            Ok(block)
        }
        Err(error) => {
            let (reason, phase) = match error {
                crate::transfers::IngressTransferError::NoEffect(reason)
                    if prior_dispatches == 0 =>
                {
                    (
                        reason,
                        AmmIngressPhase::Rejected {
                            leg_index: leg_index as u32,
                        },
                    )
                }
                crate::transfers::IngressTransferError::NoEffect(reason) => (
                    reason,
                    AmmIngressPhase::HeldUnknown {
                        leg_index: leg_index as u32,
                    },
                ),
                crate::transfers::IngressTransferError::TooOld if prior_dispatches == 0 => (
                    "ledger rejected first ingress dispatch as TooOld; no prior transfer could have committed".to_string(),
                    AmmIngressPhase::Rejected {
                        leg_index: leg_index as u32,
                    },
                ),
                crate::transfers::IngressTransferError::TooOld => (
                    "ledger reports TooOld; exact receipt proof required".to_string(),
                    AmmIngressPhase::HeldTooOld {
                        leg_index: leg_index as u32,
                    },
                ),
                crate::transfers::IngressTransferError::Ambiguous(reason) => (
                    reason,
                    AmmIngressPhase::HeldUnknown {
                        leg_index: leg_index as u32,
                    },
                ),
            };
            mutate_state(|s| {
                if let Some(op) = s
                    .ingress_operations
                    .iter_mut()
                    .find(|op| op.id == operation_id)
                {
                    op.phase = phase;
                    op.last_error = Some(reason.clone());
                }
            });
            Err(AmmError::TransferFailed {
                token: format!("ingress:{}", leg_index),
                reason,
            })
        }
    }
}

#[derive(Clone)]
struct ExactTransferReceipt {
    from: Principal,
    from_subaccount: Option<[u8; 32]>,
    to: Principal,
    to_subaccount: Option<[u8; 32]>,
    amount: u128,
    fee: Option<u128>,
    memo: Vec<u8>,
    created_at_time: u64,
    spender: Option<Principal>,
}

fn value_as_map(
    value: &icrc_ledger_types::icrc::generic_value::ICRC3Value,
) -> Option<&std::collections::BTreeMap<String, icrc_ledger_types::icrc::generic_value::ICRC3Value>>
{
    if let icrc_ledger_types::icrc::generic_value::ICRC3Value::Map(map) = value {
        Some(map)
    } else {
        None
    }
}

fn value_as_nat(
    value: Option<&icrc_ledger_types::icrc::generic_value::ICRC3Value>,
) -> Option<u128> {
    match value? {
        icrc_ledger_types::icrc::generic_value::ICRC3Value::Nat(value) => {
            value.0.clone().try_into().ok()
        }
        _ => None,
    }
}

fn value_as_blob(
    value: Option<&icrc_ledger_types::icrc::generic_value::ICRC3Value>,
) -> Option<Vec<u8>> {
    match value? {
        icrc_ledger_types::icrc::generic_value::ICRC3Value::Blob(value) => Some(value.to_vec()),
        _ => None,
    }
}

fn account_matches(
    value: Option<&icrc_ledger_types::icrc::generic_value::ICRC3Value>,
    owner: Principal,
    expected_subaccount: Option<[u8; 32]>,
) -> Result<bool, String> {
    let Some(icrc_ledger_types::icrc::generic_value::ICRC3Value::Array(parts)) = value else {
        return Err("ICRC-3 transfer account has unknown shape".to_string());
    };
    if parts.is_empty() || parts.len() > 2 {
        return Err("ICRC-3 transfer account has invalid component count".to_string());
    }
    let Some(owner_bytes) = value_as_blob(parts.first()) else {
        return Err("ICRC-3 transfer account owner is not a blob".to_string());
    };
    if owner_bytes.as_slice() != owner.as_slice() {
        return Ok(false);
    }
    let sub = match parts.get(1) {
        Some(icrc_ledger_types::icrc::generic_value::ICRC3Value::Blob(bytes)) => {
            Some(bytes.as_slice())
        }
        Some(_) => return Err("ICRC-3 transfer account subaccount is not a blob".to_string()),
        None => None,
    };
    match (expected_subaccount, parts.len(), sub) {
        (None, 1, None) => Ok(true),
        (Some(expected), 2, Some(bytes)) => Ok(bytes == expected),
        (None, 2, Some(_)) | (Some(_), 1, None) => Ok(false),
        _ => Err("ICRC-3 transfer account subaccount shape is inconsistent".to_string()),
    }
}

fn value_as_u64(value: Option<&icrc_ledger_types::icrc::generic_value::ICRC3Value>) -> Option<u64> {
    match value? {
        icrc_ledger_types::icrc::generic_value::ICRC3Value::Nat(value) => {
            value.0.clone().try_into().ok()
        }
        _ => None,
    }
}

fn block_matches_exact_receipt(
    block: &icrc_ledger_types::icrc3::blocks::BlockWithId,
    expected: &ExactTransferReceipt,
) -> Result<bool, String> {
    let Some(root) = value_as_map(&block.block) else {
        return Err("ICRC-3 block has unknown top-level shape".to_string());
    };
    let Some(tx) = root.get("tx").and_then(value_as_map) else {
        return Err("ICRC-3 block is missing a recognized transaction map".to_string());
    };
    match tx.get("op") {
        Some(icrc_ledger_types::icrc::generic_value::ICRC3Value::Text(op)) if op == "xfer" => {}
        Some(icrc_ledger_types::icrc::generic_value::ICRC3Value::Text(op))
            if matches!(op.as_str(), "mint" | "burn" | "approve") =>
        {
            return Ok(false)
        }
        Some(icrc_ledger_types::icrc::generic_value::ICRC3Value::Text(op)) => {
            return Err(format!(
                "unrecognized ICRC-3 operation `{}`; absence proof held",
                op
            ))
        }
        _ => return Err("ICRC-3 transaction has missing or malformed operation kind".to_string()),
    }
    let tx_time = match tx.get("ts") {
        None => return Ok(false),
        Some(value) => value_as_u64(Some(value))
            .ok_or_else(|| "ICRC-3 transfer has malformed created_at_time".to_string())?,
    };
    let amount = value_as_nat(tx.get("amt"))
        .ok_or_else(|| "ICRC-3 transfer has missing or malformed amount".to_string())?;
    let memo = match tx.get("memo") {
        None => return Ok(false),
        Some(value) => value_as_blob(Some(value))
            .ok_or_else(|| "ICRC-3 transfer has malformed memo".to_string())?,
    };
    let from_matches = account_matches(tx.get("from"), expected.from, expected.from_subaccount)?;
    let to_matches = account_matches(tx.get("to"), expected.to, expected.to_subaccount)?;
    if !from_matches
        || !to_matches
        || amount != expected.amount
        || memo != expected.memo
        || tx_time != expected.created_at_time
    {
        return Ok(false);
    }
    match expected.spender {
        Some(spender) => {
            let Some(spender_value) = tx.get("spender") else {
                return Ok(false);
            };
            if !account_matches(Some(spender_value), spender, None)? {
                return Ok(false);
            }
        }
        None if tx.contains_key("spender") => return Ok(false),
        _ => {}
    }
    if let Some(expected_fee) = expected.fee {
        let Some(fee_value) = tx.get("fee") else {
            return Ok(false);
        };
        let fee = value_as_nat(Some(fee_value))
            .ok_or_else(|| "ICRC-3 transfer has malformed fee".to_string())?;
        if fee != expected_fee {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Capture an authoritative replicated ledger-history lower bound before the
/// first exact transfer dispatch. All possible executions of this tuple occur
/// after this point, so old blocks need not be scanned for this identity.
async fn ledger_receipt_tip(ledger: Principal) -> Result<u64, String> {
    use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
    let request = GetBlocksRequest {
        start: Nat::from(0u64),
        length: Nat::from(1u64),
    };
    let (response,): (GetBlocksResult,) =
        ic_cdk::call(ledger, "icrc3_get_blocks", (vec![request],))
            .await
            .map_err(|(code, message)| {
                format!(
                    "ICRC-3 replicated tip query unavailable: {:?} - {}",
                    code, message
                )
            })?;
    response
        .log_length
        .0
        .try_into()
        .map_err(|_| "ledger log length exceeds u64".to_string())
}

/// Scan at most 32 block indexes per invocation. Archive callbacks are followed
/// only for ranges advertised by `icrc3_get_blocks`; cursor advances only when
/// every index in the bounded window was returned, including archived blocks.
async fn scan_exact_transfer_receipt(
    ledger: Principal,
    cursor: u64,
    frozen_tip: Option<u64>,
    expected: &ExactTransferReceipt,
) -> Result<(Option<u64>, u64, u64), String> {
    use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
    if let Some(tip) = frozen_tip {
        if cursor >= tip {
            return Ok((None, cursor, tip));
        }
    }
    let request_length = frozen_tip
        .map(|tip| tip.saturating_sub(cursor).min(32))
        .unwrap_or(32);
    let request = GetBlocksRequest {
        start: Nat::from(cursor),
        length: Nat::from(request_length),
    };
    let (mut response,): (GetBlocksResult,) =
        ic_cdk::call(ledger, "icrc3_get_blocks", (vec![request.clone()],))
            .await
            .map_err(|(code, message)| {
                format!("ICRC-3 query unavailable: {:?} - {}", code, message)
            })?;
    let log_length: u64 = response
        .log_length
        .0
        .clone()
        .try_into()
        .map_err(|_| "ledger log length exceeds u64".to_string())?;
    let tip = receipt_scan_tip(frozen_tip, log_length)?;
    let end = checked_receipt_scan_page_end(cursor, tip)?;
    if cursor >= tip {
        return Ok((None, cursor, tip));
    }
    let mut blocks = std::mem::take(&mut response.blocks);
    if response.archived_blocks.len() > 32 {
        return Err(
            "ICRC-3 response advertises more archive callbacks than the bounded page can cover"
                .to_string(),
        );
    }
    for archived in response.archived_blocks {
        let (archived_result,): (GetBlocksResult,) = ic_cdk::call(
            archived.callback.canister_id,
            archived.callback.method.as_str(),
            (archived.args,),
        )
        .await
        .map_err(|(code, message)| {
            format!("ICRC-3 archive callback failed: {:?} - {}", code, message)
        })?;
        blocks.extend(archived_result.blocks);
    }
    sort_and_reject_duplicate_block_ids(&mut blocks)?;
    let mut next = cursor;
    for block in &blocks {
        let index: u64 = block
            .id
            .0
            .clone()
            .try_into()
            .map_err(|_| "ledger block index exceeds u64".to_string())?;
        if index < cursor || index >= end {
            continue;
        }
        if block_matches_exact_receipt(block, expected)? {
            return Ok((Some(index), index, log_length));
        }
        if index == next {
            next = next.saturating_add(1);
        }
    }
    if next < end {
        return Err(format!(
            "ICRC-3 response did not cover requested block {}; cursor held",
            next
        ));
    }
    Ok((None, end, tip))
}

/// Send or retry a payout using its persisted exact ICRC-1 identity. `TooOld`
/// is a terminal hold for automatic sending: only exact receipt evidence may
/// later clear it. A lost callback leaves the row and same tuple available.
pub(crate) async fn process_payout_attempt(payout_id: u64) -> Result<u64, String> {
    let mut attempt = read_state(|s| {
        s.pending_payouts
            .iter()
            .find(|p| p.id == payout_id)
            .cloned()
    })
    .ok_or_else(|| "payout attempt not found".to_string())?;
    if attempt.phase == AmmPayoutPhase::Staged {
        return Err(
            "payout is staged behind its parent operation and is not yet payable".to_string(),
        );
    }
    if attempt.phase == AmmPayoutPhase::ReadyForReprice {
        mutate_state(|s| -> Result<(), String> {
            let row = s
                .pending_payouts
                .iter_mut()
                .find(|p| p.id == payout_id)
                .ok_or_else(|| "payout attempt was resolved".to_string())?;
            if row.phase != AmmPayoutPhase::ReadyForReprice {
                return Err("payout phase changed before safe reprice".to_string());
            }
            row.attempt_generation = row
                .attempt_generation
                .checked_add(1)
                .ok_or_else(|| "payout attempt generation exhausted".to_string())?;
            row.memo = payout_memo(row.id, row.attempt_generation);
            row.created_at_time = ic_cdk::api::time();
            row.send_amount = None;
            row.fee = None;
            row.dispatch_count = 0;
            row.receipt_scan_start = None;
            row.receipt_scan_cursor = 0;
            row.receipt_scan_end = None;
            row.phase = AmmPayoutPhase::AwaitingFee;
            row.last_error = None;
            Ok(())
        })?;
        attempt = read_state(|s| {
            s.pending_payouts
                .iter()
                .find(|p| p.id == payout_id)
                .cloned()
        })
        .ok_or_else(|| "payout attempt was resolved".to_string())?;
    }
    if attempt.phase == AmmPayoutPhase::HeldTooOld || attempt.phase == AmmPayoutPhase::LegacyUnknown
    {
        return Err("payout is held pending exact ledger receipt reconciliation".to_string());
    }

    let exact = match (attempt.send_amount, attempt.fee) {
        (Some(send), Some(fee)) => (send, fee),
        _ => {
            // The durable intent and id/memo/time already exist before this
            // await. Persist the complete tuple before the transfer await.
            let fee = crate::transfers::ledger_fee(attempt.ledger).await;
            if attempt.gross_amount <= fee {
                let reason = format!(
                    "gross amount {} does not exceed fee {}",
                    attempt.gross_amount, fee
                );
                mutate_state(|s| {
                    if let Some(row) = s.pending_payouts.iter_mut().find(|p| p.id == payout_id) {
                        row.last_error = Some(reason.clone());
                    }
                });
                return Err(reason);
            }
            let send = attempt.gross_amount - fee;
            mutate_state(|s| {
                if let Some(row) = s.pending_payouts.iter_mut().find(|p| p.id == payout_id) {
                    row.send_amount = Some(send);
                    row.fee = Some(fee);
                    row.phase = AmmPayoutPhase::Ready;
                }
            });
            (send, fee)
        }
    };

    let mut current = read_state(|s| {
        s.pending_payouts
            .iter()
            .find(|p| p.id == payout_id)
            .cloned()
    })
    .ok_or_else(|| "payout attempt was resolved".to_string())?;
    if current.phase == AmmPayoutPhase::HeldTooOld || current.phase == AmmPayoutPhase::LegacyUnknown
    {
        return Err("payout is held pending exact ledger receipt reconciliation".to_string());
    }
    if current.dispatch_count == 0 && current.receipt_scan_start.is_none() {
        // The transfer itself remains safe without ICRC-3 support because all
        // retries keep the exact persisted tuple. The baseline only bounds a
        // later absence proof; if the profile cannot answer, send once and
        // hold ambiguous outcomes rather than blocking LP exits pre-send.
        if let Ok(start) = ledger_receipt_tip(current.ledger).await {
            let saved = mutate_state(|s| {
                if let Some(row) = s
                    .pending_payouts
                    .iter_mut()
                    .find(|row| row.id == payout_id && **row == current)
                {
                    row.receipt_scan_start = Some(start);
                    row.receipt_scan_cursor = start;
                    true
                } else {
                    false
                }
            });
            if !saved {
                return Err("payout changed while capturing pre-dispatch ledger tip".to_string());
            }
            current = read_state(|s| {
                s.pending_payouts
                    .iter()
                    .find(|row| row.id == payout_id)
                    .cloned()
            })
            .ok_or_else(|| "payout attempt was resolved".to_string())?;
        }
    }
    let had_prior_ambiguity = payout_phase_has_prior_ambiguity(&current.phase);
    // Store Submitted before invoking the ledger. A reject or trap is
    // ambiguous, so every subsequent send uses these same args.
    mutate_state(|s| {
        if let Some(row) = s.pending_payouts.iter_mut().find(|p| p.id == payout_id) {
            row.dispatch_count = row.dispatch_count.saturating_add(1);
            row.phase = AmmPayoutPhase::Submitted;
            row.last_error = None;
        }
    });
    let args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: Some(current.subaccount),
        to: icrc_ledger_types::icrc1::account::Account {
            owner: current.claimant,
            subaccount: None,
        },
        amount: Nat::from(exact.0),
        // The exact fee quote is part of the journaled tuple. Repricing is
        // allowed only after a typed no-effect response or full old-identity
        // absence proof after TooOld.
        fee: Some(Nat::from(exact.1)),
        memo: Some(icrc_ledger_types::icrc1::transfer::Memo(
            serde_bytes::ByteBuf::from(current.memo),
        )),
        created_at_time: Some(current.created_at_time),
    };
    let result: Result<(Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,), _> =
        ic_cdk::call(current.ledger, "icrc1_transfer", (args,)).await;
    let block = match result {
        Ok((Ok(index),)) => index
            .0
            .try_into()
            .map_err(|_| "ledger block index exceeds u64".to_string())?,
        Ok((Err(icrc_ledger_types::icrc1::transfer::TransferError::Duplicate {
            duplicate_of,
        }),)) => duplicate_of
            .0
            .try_into()
            .map_err(|_| "duplicate block index exceeds u64".to_string())?,
        Ok((Err(icrc_ledger_types::icrc1::transfer::TransferError::TooOld),)) => {
            let reason =
                "ledger reports TooOld; exact receipt proof required before resolution".to_string();
            mutate_state(|s| {
                if let Some(row) = s.pending_payouts.iter_mut().find(|p| p.id == payout_id) {
                    row.phase = AmmPayoutPhase::HeldTooOld;
                    row.last_error = Some(reason.clone());
                }
            });
            return Err(reason);
        }
        Ok((Err(icrc_ledger_types::icrc1::transfer::TransferError::BadFee { expected_fee }),)) => {
            let reason = format!("icrc1_transfer BadFee; ledger expects fee {}", expected_fee);
            let expected_fee: Option<u128> = expected_fee.0.try_into().ok();
            crate::transfers::invalidate_ledger_fee(current.ledger);
            mutate_state(|s| {
                if let Some(row) = s.pending_payouts.iter_mut().find(|p| p.id == payout_id) {
                    if had_prior_ambiguity {
                        row.phase = AmmPayoutPhase::HeldUnknown;
                    } else if let Some(fee) = expected_fee.filter(|fee| *fee < row.gross_amount) {
                        row.fee = Some(fee);
                        row.send_amount = Some(row.gross_amount - fee);
                        row.phase = AmmPayoutPhase::Ready;
                    } else {
                        row.fee = None;
                        row.send_amount = None;
                        row.phase = AmmPayoutPhase::AwaitingFee;
                    }
                    row.last_error = Some(reason.clone());
                }
            });
            return Err(reason);
        }
        Ok((Err(error),)) => {
            let reason = format!("icrc1_transfer error: {:?}", error);
            mutate_state(|s| {
                if let Some(row) = s.pending_payouts.iter_mut().find(|p| p.id == payout_id) {
                    row.phase = match error {
                        icrc_ledger_types::icrc1::transfer::TransferError::BadBurn { .. }
                        | icrc_ledger_types::icrc1::transfer::TransferError::InsufficientFunds {
                            ..
                        }
                        | icrc_ledger_types::icrc1::transfer::TransferError::CreatedInFuture {
                            ..
                        } if !had_prior_ambiguity => AmmPayoutPhase::Ready,
                        _ => AmmPayoutPhase::HeldUnknown,
                    };
                    row.last_error = Some(reason.clone());
                }
            });
            return Err(reason);
        }
        Err((code, message)) => {
            let reason = format!("icrc1_transfer call failed: {:?} - {}", code, message);
            mutate_state(|s| {
                if let Some(row) = s.pending_payouts.iter_mut().find(|p| p.id == payout_id) {
                    row.phase = AmmPayoutPhase::HeldUnknown;
                    row.last_error = Some(reason.clone());
                }
            });
            return Err(reason);
        }
    };
    // The ledger confirmed this exact identity (or deduplicated it). Removal
    // occurs only after the await; a trap before this point leaves a safe retry.
    mutate_state(|s| {
        s.pending_payouts.retain(|p| p.id != payout_id);
        if let Some(op) = s
            .ingress_operations
            .iter_mut()
            .find(|op| op.id == current.operation_id)
        {
            if !op.confirmed_payout_ids.contains(&payout_id) {
                op.confirmed_payout_ids.push(payout_id);
            }
        }
    });
    Ok(block)
}

pub(crate) fn prepare_protocol_fee_payouts(
    pool_id: &PoolId,
    recipient: Principal,
) -> Result<(u128, u128, Vec<u64>), AmmError> {
    mutate_state(|s| {
        let (token_a, token_b, sub_a, sub_b, fees_a, fees_b) = {
            let pool = s.pools.get(pool_id).ok_or(AmmError::PoolNotFound)?;
            (
                pool.token_a,
                pool.token_b,
                pool.subaccount_a,
                pool.subaccount_b,
                pool.protocol_fees_a,
                pool.protocol_fees_b,
            )
        };
        let count = usize::from(fees_a > 0) + usize::from(fees_b > 0);
        if s.pending_payouts.len().saturating_add(count) > state::MAX_PENDING_CLAIMS {
            return Err(AmmError::TransferFailed {
                token: "payout_capacity".to_string(),
                reason: "pending payout journal is full; refusing to withdraw protocol fees"
                    .to_string(),
            });
        }
        let mut ids = Vec::with_capacity(count);
        if fees_a > 0 {
            let id = s.next_payout_id;
            s.next_payout_id = id.checked_add(1).ok_or_else(|| AmmError::TransferFailed {
                token: "payout_id".into(),
                reason: "identity space exhausted".into(),
            })?;
            let mut row = new_payout_row(id, 0, pool_id, recipient, token_a, sub_a, fees_a);
            row.phase = AmmPayoutPhase::AwaitingFee;
            s.pending_payouts.push(row);
            ids.push(id);
        }
        if fees_b > 0 {
            let id = s.next_payout_id;
            s.next_payout_id = id.checked_add(1).ok_or_else(|| AmmError::TransferFailed {
                token: "payout_id".into(),
                reason: "identity space exhausted".into(),
            })?;
            let mut row = new_payout_row(id, 0, pool_id, recipient, token_b, sub_b, fees_b);
            row.phase = AmmPayoutPhase::AwaitingFee;
            s.pending_payouts.push(row);
            ids.push(id);
        }
        let pool = s.pools.get_mut(pool_id).expect("pool was read above");
        pool.protocol_fees_a = 0;
        pool.protocol_fees_b = 0;
        Ok((fees_a, fees_b, ids))
    })
}

async fn payout_to_user_journaled(
    operation_id: u64,
    pool_id: &PoolId,
    ledger: Principal,
    subaccount: [u8; 32],
    claimant: Principal,
    gross_amount: u128,
) -> Result<u64, String> {
    let payout_id = create_payout_attempt(
        operation_id,
        pool_id,
        claimant,
        ledger,
        subaccount,
        gross_amount,
    )
    .map_err(|e| format!("could not persist payout intent: {:?}", e))?;
    mutate_state(|s| {
        if let Some(row) = s.pending_payouts.iter_mut().find(|p| p.id == payout_id) {
            row.phase = AmmPayoutPhase::AwaitingFee;
        }
    });
    process_payout_attempt(payout_id).await
}
/// Receive a reward donation from the protocol backend. The caller is
/// expected to have already minted `amount` icUSD into this canister's
/// per-pool reward subaccount before invoking this call. This call
/// verifies the on-chain balance grew by at least `amount` and bumps
/// `acc_reward_per_share` (or buffers in `pending_no_lp` if there are
/// no LPs yet). Idempotent on duplicate `nonce`.
#[update]
pub async fn notify_reward_received(
    pool_id: PoolId,
    amount: u128,
    nonce: u64,
) -> Result<(), AmmError> {
    // 1. Caller restriction: only the configured protocol backend principal.
    let caller = ic_cdk::caller();
    let authorized = read_state(|s| s.protocol_backend_principal);
    match authorized {
        Some(p) if p == caller => {}
        _ => return Err(AmmError::Unauthorized),
    }

    // 2. Acquire pool guard before any await. Released via Drop on return.
    let _guard = PoolGuard::new(pool_id.clone())?;

    // 3. Early dedup (avoids unnecessary balance query for repeated nonces).
    let already_processed = read_state(|s| {
        s.pools
            .get(&pool_id)
            .map(|p| p.processed_donation_nonces.contains(&nonce))
            .unwrap_or(false)
    });
    if already_processed {
        log!(
            INFO,
            "[notify_reward_received] dedup on nonce {} for pool {}",
            nonce,
            pool_id
        );
        return Ok(());
    }

    // 4. Verify on-chain balance grew by at least `amount` since last snapshot.
    let on_chain = query_reward_subaccount_balance(&pool_id).await?;

    // 5. Re-check dedup inside the mutate_state lock (race protection
    // for the await above), then bump accumulator and record nonce.
    mutate_state(|s| -> Result<(), AmmError> {
        let pool = s.pools.get_mut(&pool_id).ok_or(AmmError::PoolNotFound)?;

        // Re-check dedup under lock.
        if pool.processed_donation_nonces.contains(&nonce) {
            return Ok(());
        }

        // Verify expected balance growth.
        let expected = pool
            .reward_balance_snapshot
            .checked_add(amount)
            .ok_or(AmmError::MathOverflow)?;
        if on_chain < expected {
            return Err(AmmError::InsufficientOnChainBalance {
                expected,
                actual: on_chain,
            });
        }

        // Bump accumulator (or buffer if no LPs).
        if pool.total_lp_shares > 0 {
            pool.acc_reward_per_share =
                crate::rewards::accumulate(pool.acc_reward_per_share, amount, pool.total_lp_shares);
        } else {
            pool.pending_no_lp = pool.pending_no_lp.saturating_add(amount);
        }
        pool.total_rewards_distributed = pool.total_rewards_distributed.saturating_add(amount);

        // Advance only by the amount acknowledged here. The live balance may
        // include another donation whose receipt has not been accepted yet.
        pool.reward_balance_snapshot = expected;

        // Keep every accepted identity so a delayed backend retry can never
        // be credited again after a bounded dedup window expires.
        pool.processed_donation_nonces.push_back(nonce);

        // Emit event.
        let total_shares = pool.total_lp_shares;
        s.record_reward_event(pool_id.clone(), amount, total_shares, nonce);

        Ok(())
    })?;

    Ok(())
}

/// Claim accumulated reward icUSD for the caller. Settles pending into
/// claimable, transfers claimable, zeroes claimable on success. On
/// transfer failure, restores claimable so the caller can retry.
#[update]
pub async fn claim_rewards(pool_id: PoolId) -> Result<u128, AmmError> {
    let caller = ic_cdk::caller();
    if caller == Principal::anonymous() {
        return Err(AmmError::Unauthorized);
    }

    let _guard = PoolGuard::new(pool_id.clone())?;

    // Phase 1: settle pending into claimable, snapshot the amount, persist.
    let amount = mutate_state(|s| -> Result<u128, AmmError> {
        if s.pending_payouts.len() >= state::MAX_PENDING_CLAIMS
            || s.next_payout_id.checked_add(1).is_none()
        {
            return Err(AmmError::TransferFailed {
                token: "payout_capacity".to_string(),
                reason: "pending payout journal cannot safely accept the reward claim".to_string(),
            });
        }
        let pool = s.pools.get_mut(&pool_id).ok_or(AmmError::PoolNotFound)?;
        let shares = pool.lp_shares.get(&caller).copied().unwrap_or(0);
        let acc = pool.acc_reward_per_share;
        let entry = pool.lp_rewards.entry(caller).or_default();

        crate::rewards::settle(entry, shares, acc);
        crate::rewards::reset_debt(entry, shares, acc);

        let claimable = entry.claimable;
        if claimable < crate::state::MIN_CLAIM_E8S {
            return Err(AmmError::BelowMinClaim {
                claimable,
                min: crate::state::MIN_CLAIM_E8S,
            });
        }
        // Zero only after ensuring durable payout capacity is available.
        entry.claimable = 0;
        Ok(claimable)
    })?;

    // Persist the exact payout identity before the fee query or transfer await.
    let ledger = Principal::from_text(ICUSD_LEDGER).expect("invalid icUSD ledger principal");
    let subaccount = reward_subaccount_for(&pool_id);
    let payout_id = create_payout_attempt(0, &pool_id, caller, ledger, subaccount, amount)?;
    mutate_state(|s| {
        if let Some(row) = s.pending_payouts.iter_mut().find(|p| p.id == payout_id) {
            row.phase = AmmPayoutPhase::AwaitingFee;
        }
        if let Some(pool) = s.pools.get_mut(&pool_id) {
            pool.reward_balance_snapshot = pool.reward_balance_snapshot.saturating_sub(amount);
        }
    });
    let payout_result = process_payout_attempt(payout_id).await;

    match payout_result {
        Ok(_block_index) => {
            // A successful reward transfer decreases the reward subaccount by
            // exactly `amount` (the recipient bears the ledger fee). Do not
            // replace credited liability with the observed total: that total
            // can include a concurrent donation not yet acknowledged below.
            mutate_state(|s| {
                s.record_claim_event(pool_id.clone(), caller, amount);
            });
            Ok(amount)
        }
        Err(e) => {
            // The exact payout remains in `pending_payouts`; restoring
            // claimable here would let the user create a second identity.
            Err(AmmError::RewardLedgerTransferFailed { reason: e })
        }
    }
}

/// Read-only pending reward calculation for UI display.
#[query]
pub fn get_pending_rewards(pool_id: PoolId, principal: Principal) -> Nat {
    read_state(|s| {
        let Some(pool) = s.pools.get(&pool_id) else {
            return Nat::from(0u64);
        };
        let shares = pool.lp_shares.get(&principal).copied().unwrap_or(0);
        let entry = pool.lp_rewards.get(&principal);
        let claimable = entry.map(|e| e.claimable).unwrap_or(0);
        let debt = entry.map(|e| e.reward_debt).unwrap_or(0);
        let unsettled = crate::rewards::pending(shares, pool.acc_reward_per_share, debt);
        Nat::from(claimable.saturating_add(unsettled))
    })
}

// ─── Claims ───

/// Retry a failed outbound transfer. The original claimant or admin can call this.
///
/// To prevent double-claim races (two concurrent calls both reading the same claim
/// then both transferring), we remove the claim from state BEFORE the async transfer.
/// If the transfer fails, we re-add the claim.
#[update]
async fn claim_pending(claim_id: u64) -> Result<(), AmmError> {
    let caller = ic_cdk::caller();
    let claim = read_state(|s| s.pending_claims.iter().find(|c| c.id == claim_id).cloned())
        .ok_or(AmmError::ClaimNotFound)?;
    if caller != claim.claimant && caller_is_admin().is_err() {
        return Err(AmmError::Unauthorized);
    }
    // Legacy rows do not contain created_at_time or memo. Assigning a new
    // identity here could pay twice after a committed-but-lost reply, so they
    // remain visible for evidence-bound manual reconciliation.
    Err(AmmError::TransferFailed {
        token: claim_id.to_string(),
        reason: "legacy claim has no exact transfer identity and is held; do not retry with a new ledger tuple".to_string(),
    })
}

/// View all pending claims.
#[query]
fn get_pending_claims() -> Vec<PendingClaim> {
    read_state(|s| s.pending_claims.clone())
}

#[cfg(feature = "test_endpoints")]
#[update]
fn test_set_pending_claim_limit(limit: u64) -> Result<(), AmmError> {
    caller_is_admin()?;
    let limit = usize::try_from(limit).map_err(|_| AmmError::PendingClaimCapacityReached)?;
    let occupied = read_state(|s| s.pending_claims.len())
        .saturating_add(RESERVED_PENDING_CLAIM_SLOTS.with(Cell::get));
    if limit < occupied || limit > state::MAX_PENDING_CLAIMS {
        return Err(AmmError::PendingClaimCapacityReached);
    }
    TEST_PENDING_CLAIM_LIMIT.with(|test_limit| test_limit.set(Some(limit)));
    Ok(())
}

#[cfg(feature = "test_endpoints")]
#[update]
fn test_insert_pending_claim(pool_id: PoolId, amount: u128) -> Result<u64, AmmError> {
    caller_is_admin()?;
    let _pool_guard = PoolGuard::new(pool_id.clone())?;
    let (token, subaccount) = read_state(|s| {
        let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;
        Ok::<_, AmmError>((pool.token_a, pool.subaccount_a))
    })?;
    let mut slots = PendingClaimSlots::reserve(1)?;
    Ok(record_pending_claim(
        &mut slots,
        &pool_id,
        ic_cdk::caller(),
        token,
        subaccount,
        amount,
        "test-injected claim",
    ))

#[query]
fn get_pending_amm_payouts() -> Vec<AmmPayoutAttempt> {
    let caller = ic_cdk::caller();
    let is_admin = read_state(|s| s.admin == caller);
    if caller == Principal::anonymous() {
        return Vec::new();
    }
    read_state(|s| {
        s.pending_payouts
            .iter()
            .filter(|row| is_admin || row.claimant == caller)
            .cloned()
            .collect()
    })
}

/// Return the caller's last durable request so another device/session can
/// resume the exact request after local browser storage is lost.
#[query]
fn get_my_amm_operation() -> Option<AmmIngressOperation> {
    let caller = ic_cdk::caller();
    if caller == Principal::anonymous() {
        return None;
    }
    read_state(|s| {
        s.ingress_operations
            .iter()
            .find(|op| op.caller == caller)
            .cloned()
    })
}

#[update]
async fn retry_amm_payout(payout_id: u64) -> Result<u64, AmmError> {
    let caller = ic_cdk::caller();
    let payout = read_state(|s| {
        s.pending_payouts
            .iter()
            .find(|p| p.id == payout_id)
            .cloned()
    })
    .ok_or(AmmError::ClaimNotFound)?;
    if caller != payout.claimant && caller_is_admin().is_err() {
        return Err(AmmError::Unauthorized);
    }
    let _guard = PoolGuard::lock(payout.pool_id.clone())?;
    process_payout_attempt(payout_id)
        .await
        .map_err(|reason| AmmError::TransferFailed {
            token: payout.ledger.to_text(),
            reason,
        })
}

#[update]
async fn reconcile_amm_payout(payout_id: u64) -> Result<bool, AmmError> {
    let caller = ic_cdk::caller();
    let payout = read_state(|s| {
        s.pending_payouts
            .iter()
            .find(|p| p.id == payout_id)
            .cloned()
    })
    .ok_or(AmmError::ClaimNotFound)?;
    if caller != payout.claimant && caller_is_admin().is_err() {
        return Err(AmmError::Unauthorized);
    }
    let _guard = PoolGuard::lock(payout.pool_id.clone())?;
    if !matches!(
        payout.phase,
        AmmPayoutPhase::HeldTooOld | AmmPayoutPhase::HeldUnknown | AmmPayoutPhase::Submitted
    ) {
        return Err(AmmError::InvalidInput {
            reason: "payout must be ambiguous or TooOld before receipt scanning".to_string(),
        });
    }
    let expected = ExactTransferReceipt {
        from: ic_cdk::id(),
        from_subaccount: Some(payout.subaccount),
        to: payout.claimant,
        to_subaccount: None,
        amount: payout.send_amount.ok_or(AmmError::ClaimNotFound)?,
        fee: payout.fee,
        memo: payout.memo.clone(),
        created_at_time: payout.created_at_time,
        spender: None,
    };
    match scan_exact_transfer_receipt(
        payout.ledger,
        payout.receipt_scan_cursor,
        payout.receipt_scan_end,
        &expected,
    )
    .await
    {
        Ok((Some(block), cursor, end)) => {
            let changed = mutate_state(|s| {
                if !s.pending_payouts.iter().any(|row| row == &payout) {
                    return false;
                }
                s.pending_payouts.retain(|row| row.id != payout_id);
                if let Some(op) = s
                    .ingress_operations
                    .iter_mut()
                    .find(|op| op.id == payout.operation_id)
                {
                    if !op.confirmed_payout_ids.contains(&payout_id) {
                        op.confirmed_payout_ids.push(payout_id);
                    }
                    if op
                        .payout_ids
                        .iter()
                        .all(|id| op.confirmed_payout_ids.contains(id))
                    {
                        match &op.kind {
                            AmmIngressKind::Swap { .. } => {
                                let result = SwapResult {
                                    amount_out: op.computed_values.first().copied().unwrap_or(0),
                                    fee: op.computed_values.get(1).copied().unwrap_or(0),
                                };
                                op.result = Encode!(&result).ok();
                                op.phase = AmmIngressPhase::Complete;
                            }
                            AmmIngressKind::RemoveLiquidity { .. } => {
                                let result = (
                                    op.computed_values.first().copied().unwrap_or(0),
                                    op.computed_values.get(1).copied().unwrap_or(0),
                                );
                                op.result = Encode!(&result).ok();
                                op.phase = AmmIngressPhase::Complete;
                            }
                            AmmIngressKind::AddLiquidity { .. } => {
                                op.phase = AmmIngressPhase::Complete;
                                op.last_error = Some("token B ingress was rejected; token A refund was proven by exact receipt".to_string());
                            }
                        }
                    }
                }
                true
            });
            if !changed {
                return Err(AmmError::TransferFailed {
                    token: payout.ledger.to_text(),
                    reason: "payout changed while receipt scan was in flight; retry reconciliation"
                        .to_string(),
                });
            }
            log!(
                INFO,
                "Exact ICRC-3 payout receipt proven for AMM payout {} at block {} (scan {}..{})",
                payout_id,
                block,
                cursor,
                end
            );
            Ok(true)
        }
        Ok((None, cursor, end)) => {
            let changed = mutate_state(|s| {
                if let Some(row) = s
                    .pending_payouts
                    .iter_mut()
                    .find(|row| row.id == payout_id && **row == payout)
                {
                    row.receipt_scan_cursor = cursor;
                    row.receipt_scan_end = Some(end);
                    if payout.phase == AmmPayoutPhase::HeldTooOld && cursor >= end {
                        row.phase = AmmPayoutPhase::ReadyForReprice;
                        row.last_error = Some("old payout identity was TooOld and no exact receipt exists in the complete archive-aware history through the frozen scan tip; safe reprice is available".to_string());
                    } else if payout.phase == AmmPayoutPhase::HeldTooOld {
                        row.phase = AmmPayoutPhase::HeldTooOld;
                        row.last_error = Some(format!("exact receipt not yet found; archive-aware scan advanced through block {}", cursor));
                    } else {
                        row.phase = AmmPayoutPhase::HeldUnknown;
                        row.last_error = Some(format!("exact receipt not yet found; ambiguous payout remains held after scanning through block {}", cursor));
                    }
                    true
                } else {
                    false
                }
            });
            if !changed {
                return Err(AmmError::TransferFailed {
                    token: payout.ledger.to_text(),
                    reason: "payout changed while receipt scan was in flight; retry reconciliation"
                        .to_string(),
                });
            }
            Ok(false)
        }
        Err(reason) => {
            mutate_state(|s| {
                if let Some(row) = s
                    .pending_payouts
                    .iter_mut()
                    .find(|row| row.id == payout_id && **row == payout)
                {
                    row.last_error = Some(reason.clone());
                }
            });
            Err(AmmError::TransferFailed {
                token: payout.ledger.to_text(),
                reason,
            })
        }
    }
}

#[update]
async fn reconcile_amm_ingress(request_id: Vec<u8>) -> Result<bool, AmmError> {
    let caller = ic_cdk::caller();
    let request_id: [u8; 32] = request_id.try_into().map_err(|_| AmmError::InvalidInput {
        reason: "request_id must be exactly 32 bytes".to_string(),
    })?;
    let operation = read_state(|s| {
        s.ingress_operations
            .iter()
            .find(|op| op.caller == caller && op.request_id == request_id)
            .cloned()
    })
    .ok_or(AmmError::ClaimNotFound)?;
    let _guard = PoolGuard::lock(operation.pool_id.clone())?;
    let Some((leg_index, leg)) = operation
        .legs
        .iter()
        .enumerate()
        .find(|(_, leg)| leg.block_index.is_none())
    else {
        return Ok(true);
    };
    if !matches!(
        operation.phase,
        AmmIngressPhase::HeldTooOld { .. }
            | AmmIngressPhase::HeldUnknown { .. }
            | AmmIngressPhase::Pulling { .. }
    ) {
        return Err(AmmError::InvalidInput {
            reason: "ingress leg is not ambiguous".to_string(),
        });
    }
    let expected = ExactTransferReceipt {
        from: leg.from,
        from_subaccount: None,
        to: ic_cdk::id(),
        to_subaccount: Some(leg.to_subaccount),
        amount: leg.amount,
        fee: leg.transfer_fee,
        memo: leg.memo.clone(),
        created_at_time: leg.created_at_time,
        spender: Some(ic_cdk::id()),
    };
    match scan_exact_transfer_receipt(
        leg.ledger,
        leg.receipt_scan_cursor,
        leg.receipt_scan_end,
        &expected,
    )
    .await
    {
        Ok((Some(block), _, _)) => {
            let changed = mutate_state(|s| {
                if !s
                    .ingress_operations
                    .iter()
                    .any(|op| op.id == operation.id && *op == operation)
                {
                    return false;
                }
                if let Some(op) = s
                    .ingress_operations
                    .iter_mut()
                    .find(|op| op.id == operation.id)
                {
                    if let Some(stored) = op.legs.get_mut(leg_index) {
                        stored.block_index = Some(block);
                    }
                    op.phase = AmmIngressPhase::Pulled;
                    op.last_error = None;
                }
                true
            });
            if !changed {
                return Err(AmmError::TransferFailed {
                    token: format!("ingress:{}", leg_index),
                    reason:
                        "ingress changed while receipt scan was in flight; retry reconciliation"
                            .to_string(),
                });
            }
            Ok(true)
        }
        Ok((None, cursor, end)) => {
            let changed = mutate_state(|s| {
                if let Some(op) = s
                    .ingress_operations
                    .iter_mut()
                    .find(|op| op.id == operation.id && **op == operation)
                {
                    let prior_legs_confirmed = op
                        .legs
                        .iter()
                        .take(leg_index)
                        .all(|prior| prior.block_index.is_some());
                    let mut rearmed = false;
                    if let Some(stored) = op.legs.get_mut(leg_index) {
                        stored.receipt_scan_cursor = cursor;
                        stored.receipt_scan_end = Some(end);
                        if matches!(operation.phase, AmmIngressPhase::HeldTooOld { .. })
                            && cursor >= end
                            && stored.dispatch_count >= 2
                        {
                            if let Some(generation) = stored.attempt_generation.checked_add(1) {
                                stored.attempt_generation = generation;
                                stored.memo = ingress_memo(
                                    operation.caller,
                                    operation.request_id,
                                    leg_index as u8,
                                    generation,
                                );
                                stored.created_at_time = ic_cdk::api::time();
                                stored.dispatch_count = 0;
                                stored.receipt_scan_start = None;
                                stored.receipt_scan_cursor = 0;
                                stored.receipt_scan_end = None;
                                rearmed = true;
                            }
                        }
                    }
                    if rearmed {
                        op.phase = if prior_legs_confirmed && leg_index > 0 {
                            AmmIngressPhase::Pulled
                        } else {
                            AmmIngressPhase::Prepared
                        };
                        // This branch is reachable only after a typed TooOld
                        // on a retry of the persisted tuple. The original and
                        // retry calls, plus this replicated archive scan, all
                        // originate from this canister to the same pinned
                        // ledger principal; a gap or unavailable archive never
                        // reaches this identity-rotation branch.
                        op.last_error = Some("typed TooOld after an earlier ambiguous dispatch, followed by a complete contiguous archive-aware ICRC-3 scan through the frozen tip with no exact transfer: ingress attempt identity renewed safely".to_string());
                    }
                    true
                } else {
                    false
                }
            });
            if !changed {
                return Err(AmmError::TransferFailed {
                    token: format!("ingress:{}", leg_index),
                    reason:
                        "ingress changed while receipt scan was in flight; retry reconciliation"
                            .to_string(),
                });
            }
            Ok(false)
        }
        Err(reason) => Err(AmmError::TransferFailed {
            token: format!("ingress:{}", leg_index),
            reason,
        }),
    }
}

// ─── Core AMM ───

#[update]
async fn swap(
    pool_id: PoolId,
    token_in: Principal,
    amount_in: u128,
    min_amount_out: u128,
) -> Result<SwapResult, AmmError> {
    let _ = (pool_id, token_in, amount_in, min_amount_out);
    Err(AmmError::TransferFailed {
        token: "swap".to_string(),
        reason: "legacy swap has no caller request identity; use swap_v2".to_string(),
    })
}

#[update]
async fn swap_v2(
    request_id: Vec<u8>,
    pool_id: PoolId,
    token_in: Principal,
    amount_in: u128,
    min_amount_out: u128,
) -> Result<SwapResult, AmmError> {
    if !AMM_V2_INGRESS_ENABLED {
        return Err(AmmError::TransferFailed {
            token: "swap_v2".to_string(),
            reason: "AMM user-pull ingress is staged off until configured ledger history semantics are verified".to_string(),
        });
    }
    if read_state(|s| s.maintenance_mode) {
        return Err(AmmError::MaintenanceMode);
    }
    reject_anonymous()?;
    let request_id: [u8; 32] = request_id.try_into().map_err(|_| AmmError::InvalidInput {
        reason: "request_id must be exactly 32 bytes".to_string(),
    })?;

    // Acquire per-pool lock to prevent interleaving attacks across await points
    let _pool_guard = PoolGuard::new_for_request(pool_id.clone(), ic_cdk::caller(), request_id)?;
    let caller = ic_cdk::caller();

    if let Some(existing) = read_state(|s| {
        s.ingress_operations
            .iter()
            .find(|op| op.caller == caller && op.request_id == request_id)
            .cloned()
    }) {
        if existing.pool_id != pool_id
            || existing.kind
                != (AmmIngressKind::Swap {
                    token_in,
                    amount_in,
                    min_amount_out,
                })
        {
            return Err(AmmError::DuplicateNonce);
        }
        if existing.phase == AmmIngressPhase::Complete {
            let bytes = existing.result.ok_or(AmmError::ClaimNotFound)?;
            return Decode!(&bytes, SwapResult).map_err(|e| AmmError::TransferFailed {
                token: "swap".to_string(),
                reason: format!("stored swap result could not decode: {}", e),
            });
        }
        if existing.phase == AmmIngressPhase::Settling {
            let payout_id = *existing.payout_ids.first().ok_or(AmmError::ClaimNotFound)?;
            if !existing.confirmed_payout_ids.contains(&payout_id) {
                process_payout_attempt(payout_id).await.map_err(|reason| {
                    AmmError::TransferFailed {
                        token: "swap payout".to_string(),
                        reason,
                    }
                })?;
            }
            let result = SwapResult {
                amount_out: *existing
                    .computed_values
                    .first()
                    .ok_or(AmmError::ClaimNotFound)?,
                fee: *existing
                    .computed_values
                    .get(1)
                    .ok_or(AmmError::ClaimNotFound)?,
            };
            let bytes = Encode!(&result).map_err(|e| AmmError::TransferFailed {
                token: "swap".into(),
                reason: e.to_string(),
            })?;
            mutate_state(|s| {
                if let Some(op) = s
                    .ingress_operations
                    .iter_mut()
                    .find(|op| op.id == existing.id)
                {
                    op.phase = AmmIngressPhase::Complete;
                    op.result = Some(bytes);
                    op.last_error = None;
                }
            });
            return Ok(result);
        }
    }

    // Read pool state
    let (token_a, token_b, reserve_a, reserve_b, fee_bps, protocol_fee_bps, sub_a, sub_b, paused) =
        read_state(|s| {
            let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;
            Ok::<_, AmmError>((
                pool.token_a,
                pool.token_b,
                pool.reserve_a,
                pool.reserve_b,
                pool.fee_bps,
                pool.protocol_fee_bps,
                pool.subaccount_a,
                pool.subaccount_b,
                pool.paused,
            ))
        })?;

    if paused {
        return Err(AmmError::PoolPaused);
    }

    // Determine direction
    let (reserve_in, reserve_out, sub_in, sub_out, ledger_in, ledger_out, is_a_to_b) =
        if token_in == token_a {
            (reserve_a, reserve_b, sub_a, sub_b, token_a, token_b, true)
        } else if token_in == token_b {
            (reserve_b, reserve_a, sub_b, sub_a, token_b, token_a, false)
        } else {
            return Err(AmmError::InvalidToken);
        };

    // Compute swap
    let (fresh_amount_out, fresh_total_fee, fresh_protocol_fee) = compute_swap(
        reserve_in,
        reserve_out,
        amount_in,
        fee_bps,
        protocol_fee_bps,
    )?;

    let (operation_id, operation) = begin_swap_operation(
        caller,
        request_id,
        &pool_id,
        token_in,
        amount_in,
        min_amount_out,
        ledger_out,
        sub_in,
        sub_out,
        fresh_amount_out,
        fresh_total_fee,
        fresh_protocol_fee,
    )?;
    if operation.phase == AmmIngressPhase::Complete {
        let bytes = operation.result.ok_or_else(|| AmmError::TransferFailed {
            token: "swap".to_string(),
            reason: "completed request is missing its stored result".to_string(),
        })?;
        return Decode!(&bytes, SwapResult).map_err(|e| AmmError::TransferFailed {
            token: "swap".to_string(),
            reason: format!("stored swap result could not decode: {}", e),
        });
    }
    let amount_out = operation
        .computed_values
        .get(0)
        .copied()
        .ok_or(AmmError::ClaimNotFound)?;
    let total_fee = operation
        .computed_values
        .get(1)
        .copied()
        .ok_or(AmmError::ClaimNotFound)?;
    let protocol_fee = operation
        .computed_values
        .get(2)
        .copied()
        .ok_or(AmmError::ClaimNotFound)?;

    // Persist the exact payout fee/send amount before the first input transfer
    // await. Resumed operations never recompute this tuple after fee drift.
    let payout_id = operation.payout_ids[0];
    let existing_fee = read_state(|s| {
        s.pending_payouts
            .iter()
            .find(|p| p.id == payout_id)
            .and_then(|p| p.fee)
    });
    let fee_out = match existing_fee {
        Some(fee) => fee,
        None => {
            let fee = crate::transfers::ledger_fee(ledger_out).await;
            mutate_state(|s| {
                if let Some(payout) = s.pending_payouts.iter_mut().find(|p| p.id == payout_id) {
                    payout.fee = Some(fee);
                    payout.send_amount = Some(payout.gross_amount.saturating_sub(fee));
                }
            });
            fee
        }
    };
    mutate_state(|s| {
        if let Some(op) = s
            .ingress_operations
            .iter_mut()
            .find(|op| op.id == operation_id)
        {
            if op.computed_values.len() < 4 {
                op.computed_values.resize(4, 0);
            }
            op.computed_values[3] = fee_out;
        }
    });
    let net_out = amount_out.saturating_sub(fee_out);
    // Audit 2026-06-09 (IC-S-003): a zero NET output means transfer_to_user
    // would skip the send entirely (amount_out <= ledger fee) while the input
    // is still pulled and reserves credited, silently consuming the input for
    // nothing. Require a positive net output regardless of min_amount_out.
    if net_out == 0 || net_out < min_amount_out {
        mutate_state(|s| {
            // No external effect occurred, so retaining a replay tombstone
            // here would let cheap rejected requests consume the global cap.
            discard_unaccepted_ingress(s, operation_id);
        });
        return Err(AmmError::InsufficientOutput {
            expected_min: min_amount_out.max(1),
            actual: net_out,
        });
    }

    if let Err(error) = process_ingress_leg(operation_id, 0).await {
        let rejected = read_state(|s| {
            s.ingress_operations
                .iter()
                .find(|op| op.id == operation_id)
                .map(|op| {
                    matches!(op.phase, AmmIngressPhase::Rejected { leg_index: 0 })
                        && op
                            .legs
                            .first()
                            .map(|leg| leg.dispatch_count == 1)
                            .unwrap_or(false)
                })
                .unwrap_or(false)
        });
        if rejected {
            // A typed first-dispatch no-effect proves no external value moved.
            mutate_state(|s| discard_unaccepted_ingress(s, operation_id));
        }
        return Err(error);
    }

    // Apply the swap once, in the same state mutation that advances the durable
    // operation phase. A trap after the callback resumes from Settling.
    let should_settle = read_state(|s| {
        s.ingress_operations
            .iter()
            .find(|op| op.id == operation_id)
            .map(|op| op.phase == AmmIngressPhase::Pulled)
            .unwrap_or(false)
    });
    if should_settle {
        mutate_state(|s| {
            let pool = s
                .pools
                .get_mut(&pool_id)
                .expect("pool must exist: verified at start of swap");
            if is_a_to_b {
                pool.reserve_a += amount_in - protocol_fee;
                pool.protocol_fees_a += protocol_fee;
                pool.reserve_b -= amount_out;
            } else {
                pool.reserve_b += amount_in - protocol_fee;
                pool.protocol_fees_b += protocol_fee;
                pool.reserve_a -= amount_out;
            }
            if let Some(op) = s
                .ingress_operations
                .iter_mut()
                .find(|op| op.id == operation_id)
            {
                op.phase = AmmIngressPhase::Settling;
            }
            if let Some(payout) = s.pending_payouts.iter_mut().find(|p| p.id == payout_id) {
                payout.phase = AmmPayoutPhase::AwaitingFee;
            }
            s.record_swap_event(
                caller,
                pool_id.clone(),
                token_in,
                amount_in,
                ledger_out,
                amount_out,
                total_fee,
            );
        });
    }

    // Send output tokens using the identity created before the first pull.
    let already_paid = read_state(|s| {
        s.ingress_operations
            .iter()
            .find(|op| op.id == operation_id)
            .map(|op| op.confirmed_payout_ids.contains(&payout_id))
            .unwrap_or(false)
    });
    if !already_paid {
        if let Err(reason) = process_payout_attempt(payout_id).await {
            return Err(AmmError::TransferFailed {
                token: "output".to_string(),
                reason,
            });
        }
    }

    let result = SwapResult {
        amount_out,
        fee: total_fee,
    };
    let result_bytes = Encode!(&result).map_err(|e| AmmError::TransferFailed {
        token: "swap".to_string(),
        reason: format!("could not encode stored result: {}", e),
    })?;
    mutate_state(|s| {
        if let Some(op) = s
            .ingress_operations
            .iter_mut()
            .find(|op| op.id == operation_id)
        {
            op.phase = AmmIngressPhase::Complete;
            op.result = Some(result_bytes);
            op.last_error = None;
        }
    });

    analytics::invalidate_cache_for_pool(&pool_id);

    log!(
        INFO,
        "Swap on {}: {} in -> {} out (fee: {}, proto: {})",
        pool_id,
        amount_in,
        amount_out,
        total_fee,
        protocol_fee
    );

    Ok(result)
}

#[update]
async fn add_liquidity(
    pool_id: PoolId,
    amount_a: u128,
    amount_b: u128,
    min_lp_shares: u128,
) -> Result<u128, AmmError> {
    let _ = (pool_id, amount_a, amount_b, min_lp_shares);
    Err(AmmError::TransferFailed {
        token: "add_liquidity".to_string(),
        reason: "legacy add_liquidity has no caller request identity; use add_liquidity_v2"
            .to_string(),
    })
}

#[update]
async fn add_liquidity_v2(
    request_id: Vec<u8>,
    pool_id: PoolId,
    amount_a: u128,
    amount_b: u128,
    min_lp_shares: u128,
) -> Result<u128, AmmError> {
    if !AMM_V2_INGRESS_ENABLED {
        return Err(AmmError::TransferFailed {
            token: "add_liquidity_v2".to_string(),
            reason: "AMM user-pull ingress is staged off until configured ledger history semantics are verified".to_string(),
        });
    }
    if read_state(|s| s.maintenance_mode) {
        return Err(AmmError::MaintenanceMode);
    }
    reject_anonymous()?;
    let request_id: [u8; 32] = request_id.try_into().map_err(|_| AmmError::InvalidInput {
        reason: "request_id must be exactly 32 bytes".to_string(),
    })?;

    // Acquire per-pool lock to prevent interleaving attacks across await points
    let _pool_guard = PoolGuard::new_for_request(pool_id.clone(), ic_cdk::caller(), request_id)?;
    let caller = ic_cdk::caller();

    let (token_a, token_b, reserve_a, reserve_b, total_shares, sub_a, sub_b, paused) =
        read_state(|s| {
            let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;
            Ok::<_, AmmError>((
                pool.token_a,
                pool.token_b,
                pool.reserve_a,
                pool.reserve_b,
                pool.total_lp_shares,
                pool.subaccount_a,
                pool.subaccount_b,
                pool.paused,
            ))
        })?;

    if paused {
        return Err(AmmError::PoolPaused);
    }

    // Compute shares
    let shares = if total_shares == 0 {
        // First deposit — use geometric mean
        compute_initial_lp_shares(amount_a, amount_b)?
    } else {
        compute_proportional_lp_shares(amount_a, amount_b, reserve_a, reserve_b, total_shares)?
    };

    if shares < min_lp_shares {
        return Err(AmmError::InsufficientOutput {
            expected_min: min_lp_shares,
            actual: shares,
        });
    }

    let (operation_id, operation) = begin_add_operation(
        caller,
        request_id,
        &pool_id,
        amount_a,
        amount_b,
        min_lp_shares,
        token_a,
        token_b,
        sub_a,
        sub_b,
        shares,
    )?;
    if operation.phase == AmmIngressPhase::Complete {
        if let Some(reason) = operation.last_error {
            return Err(AmmError::TransferFailed {
                token: "add_liquidity".to_string(),
                reason,
            });
        }
        let bytes = operation.result.ok_or(AmmError::ClaimNotFound)?;
        return Decode!(&bytes, u128).map_err(|e| AmmError::TransferFailed {
            token: "add_liquidity".to_string(),
            reason: format!("stored result could not decode: {}", e),
        });
    }
    if matches!(operation.phase, AmmIngressPhase::Rejected { leg_index: 1 }) {
        let payout_id = *operation
            .payout_ids
            .first()
            .ok_or(AmmError::ClaimNotFound)?;
        if !operation.confirmed_payout_ids.contains(&payout_id) {
            process_payout_attempt(payout_id)
                .await
                .map_err(|reason| AmmError::TransferFailed {
                    token: "token_a_refund".to_string(),
                    reason,
                })?;
        }
        let reason = "token_b ingress was definitely rejected; token_a refund was issued under its exact journaled identity".to_string();
        mutate_state(|s| {
            if let Some(saved) = s
                .ingress_operations
                .iter_mut()
                .find(|op| op.id == operation_id)
            {
                saved.phase = AmmIngressPhase::Complete;
                saved.last_error = Some(reason.clone());
            }
        });
        return Err(AmmError::TransferFailed {
            token: "add_liquidity".to_string(),
            reason,
        });
    }
    let shares = operation
        .computed_values
        .get(0)
        .copied()
        .ok_or(AmmError::ClaimNotFound)?;
    if let Err(error) = process_ingress_leg(operation_id, 0).await {
        let rejected = read_state(|s| {
            s.ingress_operations
                .iter()
                .find(|op| op.id == operation_id)
                .map(|op| {
                    matches!(op.phase, AmmIngressPhase::Rejected { leg_index: 0 })
                        && op
                            .legs
                            .first()
                            .map(|leg| leg.dispatch_count == 1)
                            .unwrap_or(false)
                })
                .unwrap_or(false)
        });
        if rejected {
            // No first-leg transfer occurred, so no replay tombstone is needed.
            mutate_state(|s| discard_unaccepted_ingress(s, operation_id));
        }
        return Err(error);
    }
    if let Err(error) = process_ingress_leg(operation_id, 1).await {
        let op = read_state(|s| {
            s.ingress_operations
                .iter()
                .find(|op| op.id == operation_id)
                .cloned()
        })
        .ok_or(AmmError::ClaimNotFound)?;
        if matches!(op.phase, AmmIngressPhase::Rejected { leg_index: 1 }) {
            let payout_id = *op.payout_ids.first().ok_or(AmmError::ClaimNotFound)?;
            if !op.confirmed_payout_ids.contains(&payout_id) {
                mutate_state(|s| {
                    if let Some(payout) = s.pending_payouts.iter_mut().find(|p| p.id == payout_id) {
                        payout.phase = AmmPayoutPhase::AwaitingFee;
                    }
                });
                process_payout_attempt(payout_id).await.map_err(|reason| {
                    AmmError::TransferFailed {
                        token: "token_a_refund".to_string(),
                        reason,
                    }
                })?;
            }
            mutate_state(|s| {
                if let Some(saved) = s
                    .ingress_operations
                    .iter_mut()
                    .find(|op| op.id == operation_id)
                {
                    saved.phase = AmmIngressPhase::Complete;
                    saved.last_error = Some("token_b ingress was definitely rejected; token_a refund was issued under its exact journaled identity".to_string());
                }
            });
        } else if matches!(op.phase, AmmIngressPhase::Rejected { leg_index: 0 }) {
            mutate_state(|s| {
                s.pending_payouts.retain(|p| p.id != op.payout_ids[0]);
                if let Some(saved) = s
                    .ingress_operations
                    .iter_mut()
                    .find(|op| op.id == operation_id)
                {
                    saved.phase = AmmIngressPhase::Complete;
                    saved.last_error = Some(
                        "token_a ingress was definitely rejected before any input was accepted"
                            .to_string(),
                    );
                }
            });
        }
        return Err(error);
    }

    // Update state (with reward bookkeeping).
    let result_bytes = Encode!(&shares).map_err(|e| AmmError::TransferFailed {
        token: "add_liquidity".to_string(),
        reason: format!("could not encode stored result: {}", e),
    })?;
    mutate_state(|s| {
        let payout_id = s
            .ingress_operations
            .iter()
            .find(|op| op.id == operation_id)
            .and_then(|op| op.payout_ids.first().copied());
        if let Some(payout_id) = payout_id {
            s.pending_payouts.retain(|p| p.id != payout_id);
        }
        let pool = s.pools.get_mut(&pool_id).expect("pool exists");

        // Snapshot pre-update state for reward bookkeeping.
        let was_first_liquidity = pool.total_lp_shares == 0;
        let existing_caller_shares = pool.lp_shares.get(&caller).copied().unwrap_or(0);
        let acc_pre_update = pool.acc_reward_per_share;

        // 1. Settle caller's existing rewards before share change.
        // No-op for first depositor (existing_caller_shares == 0).
        {
            let entry = pool.lp_rewards.entry(caller).or_default();
            crate::rewards::settle(entry, existing_caller_shares, acc_pre_update);
        }

        // 2. Apply share update (existing logic).
        if was_first_liquidity {
            // First deposit: lock MINIMUM_LIQUIDITY to zero address.
            let user_shares = shares - MINIMUM_LIQUIDITY;
            pool.lp_shares
                .insert(Principal::anonymous(), MINIMUM_LIQUIDITY);
            *pool.lp_shares.entry(caller).or_insert(0) += user_shares;
            pool.total_lp_shares = shares;

            log!(
                INFO,
                "Initial liquidity for {}: {} shares ({} locked)",
                pool_id,
                shares,
                MINIMUM_LIQUIDITY
            );
        } else {
            *pool.lp_shares.entry(caller).or_insert(0) += shares;
            pool.total_lp_shares += shares;
        }

        pool.reserve_a += amount_a;
        pool.reserve_b += amount_b;

        // 3. Reset caller's reward_debt against the PRE-DRAIN accumulator.
        // Crucial: this positions the caller to RECEIVE their pro-rata of
        // any subsequent drain (and any future donation) via the standard
        // accumulator math: pending = shares * (acc_post - acc_pre).
        // Doing this AFTER the drain instead would zero out their share
        // of the drain, since reward_debt would equal shares * acc_post.
        let new_caller_shares = pool.lp_shares.get(&caller).copied().unwrap_or(0);
        {
            let entry = pool.lp_rewards.entry(caller).or_default();
            crate::rewards::reset_debt(entry, new_caller_shares, acc_pre_update);
        }

        // 4. On first-liquidity transition, drain pending_no_lp into accumulator.
        // This is logically a synthetic donation that occurs AFTER the new
        // shares are recognized, so the standard accumulator math credits
        // the new shareholders pro-rata. (The anonymous burn-share's
        // pro-rata is permanently stranded, accepted as a tiny rounding
        // loss since MINIMUM_LIQUIDITY is small.)
        //
        // Note: total_rewards_distributed is NOT incremented here.
        // notify_reward_received already incremented it when the donation
        // first arrived (regardless of LP presence). Incrementing again
        // on drain would double-count buffered donations.
        if was_first_liquidity && pool.pending_no_lp > 0 && pool.total_lp_shares > 0 {
            let buffered = pool.pending_no_lp;
            pool.acc_reward_per_share = crate::rewards::accumulate(
                pool.acc_reward_per_share,
                buffered,
                pool.total_lp_shares,
            );
            pool.pending_no_lp = 0;
            log!(
                INFO,
                "[add_liquidity] drained pending_no_lp {} into acc for pool {}",
                buffered,
                pool_id
            );
        }
        if let Some(op) = s
            .ingress_operations
            .iter_mut()
            .find(|op| op.id == operation_id)
        {
            op.phase = AmmIngressPhase::Complete;
            op.result = Some(result_bytes);
            op.last_error = None;
        }
    });
    mutate_state(|s| {
        s.record_liquidity_event(
            caller,
            pool_id.clone(),
            AmmLiquidityAction::AddLiquidity,
            token_a,
            amount_a,
            token_b,
            amount_b,
            shares,
        )
    });
    analytics::invalidate_cache_for_pool(&pool_id);

    log!(
        INFO,
        "Add liquidity to {}: ({}, {}) -> {} shares for {}",
        pool_id,
        amount_a,
        amount_b,
        shares,
        caller
    );

    Ok(shares)
}

/// Remove liquidity from a pool.
///
/// Intentionally NOT gated by maintenance_mode: users must always be able to
/// withdraw their funds. Per-pool `paused` is the correct lever if a specific
/// pool needs to be frozen during an exploit.
#[update]
async fn remove_liquidity(
    pool_id: PoolId,
    lp_shares: u128,
    min_amount_a: u128,
    min_amount_b: u128,
) -> Result<(u128, u128), AmmError> {
    let _ = (pool_id, lp_shares, min_amount_a, min_amount_b);
    Err(AmmError::TransferFailed {
        token: "remove_liquidity".to_string(),
        reason: "legacy remove_liquidity has no caller request identity; use remove_liquidity_v2"
            .to_string(),
    })
}

#[update]
async fn remove_liquidity_v2(
    request_id: Vec<u8>,
    pool_id: PoolId,
    lp_shares: u128,
    min_amount_a: u128,
    min_amount_b: u128,
) -> Result<(u128, u128), AmmError> {
    reject_anonymous()?;
    let request_id: [u8; 32] = request_id.try_into().map_err(|_| AmmError::InvalidInput {
        reason: "request_id must be exactly 32 bytes".to_string(),
    })?;

    // Acquire per-pool lock to prevent interleaving attacks across await points
    let _pool_guard = PoolGuard::new_for_request(pool_id.clone(), ic_cdk::caller(), request_id)?;
    let caller = ic_cdk::caller();

    if let Some(existing) = read_state(|s| {
        s.ingress_operations
            .iter()
            .find(|op| op.caller == caller && op.request_id == request_id)
            .cloned()
    }) {
        if existing.pool_id != pool_id
            || existing.kind
                != (AmmIngressKind::RemoveLiquidity {
                    lp_shares,
                    min_amount_a,
                    min_amount_b,
                })
        {
            return Err(AmmError::DuplicateNonce);
        }
        if existing.phase == AmmIngressPhase::Complete {
            if let Some(reason) = existing.last_error {
                return Err(AmmError::TransferFailed {
                    token: "remove_liquidity".into(),
                    reason,
                });
            }
            let bytes = existing.result.ok_or(AmmError::ClaimNotFound)?;
            return Decode!(&bytes, (u128, u128)).map_err(|e| AmmError::TransferFailed {
                token: "remove_liquidity".into(),
                reason: format!("stored result could not decode: {}", e),
            });
        }
        if existing.phase == AmmIngressPhase::Settling {
            for payout_id in &existing.payout_ids {
                if !existing.confirmed_payout_ids.contains(payout_id) {
                    process_payout_attempt(*payout_id).await.map_err(|reason| {
                        AmmError::TransferFailed {
                            token: "remove_liquidity payout".into(),
                            reason,
                        }
                    })?;
                }
            }
            let result = (
                *existing
                    .computed_values
                    .first()
                    .ok_or(AmmError::ClaimNotFound)?,
                *existing
                    .computed_values
                    .get(1)
                    .ok_or(AmmError::ClaimNotFound)?,
            );
            let bytes = Encode!(&result).map_err(|e| AmmError::TransferFailed {
                token: "remove_liquidity".into(),
                reason: e.to_string(),
            })?;
            mutate_state(|s| {
                if let Some(op) = s
                    .ingress_operations
                    .iter_mut()
                    .find(|op| op.id == existing.id)
                {
                    op.phase = AmmIngressPhase::Complete;
                    op.result = Some(bytes);
                    op.last_error = None;
                }
            });
            return Ok(result);
        }
    }

    let (token_a, token_b, reserve_a, reserve_b, total_shares, sub_a, sub_b, user_shares, paused) =
        read_state(|s| {
            let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;
            let user_shares = pool.lp_shares.get(&caller).copied().unwrap_or(0);
            Ok::<_, AmmError>((
                pool.token_a,
                pool.token_b,
                pool.reserve_a,
                pool.reserve_b,
                pool.total_lp_shares,
                pool.subaccount_a,
                pool.subaccount_b,
                user_shares,
                pool.paused,
            ))
        })?;

    if paused {
        return Err(AmmError::PoolPaused);
    }

    if lp_shares > user_shares {
        return Err(AmmError::InsufficientLpShares {
            required: lp_shares,
            available: user_shares,
        });
    }

    let (fresh_amount_a, fresh_amount_b) =
        compute_remove_liquidity(lp_shares, reserve_a, reserve_b, total_shares)?;

    let (operation_id, operation) = begin_remove_operation(
        caller,
        request_id,
        &pool_id,
        lp_shares,
        min_amount_a,
        min_amount_b,
        token_a,
        token_b,
        sub_a,
        sub_b,
        fresh_amount_a,
        fresh_amount_b,
    )?;
    if operation.phase == AmmIngressPhase::Complete {
        if let Some(reason) = operation.last_error {
            return Err(AmmError::TransferFailed {
                token: "remove_liquidity".to_string(),
                reason,
            });
        }
        let bytes = operation.result.ok_or(AmmError::ClaimNotFound)?;
        return Decode!(&bytes, (u128, u128)).map_err(|e| AmmError::TransferFailed {
            token: "remove_liquidity".to_string(),
            reason: format!("stored result could not decode: {}", e),
        });
    }
    let amount_a = operation
        .computed_values
        .get(0)
        .copied()
        .ok_or(AmmError::ClaimNotFound)?;
    let amount_b = operation
        .computed_values
        .get(1)
        .copied()
        .ok_or(AmmError::ClaimNotFound)?;

    // Enforce slippage against the NET amounts the withdrawer receives (each leg
    // pays `amount - ledger_fee`), so min_amount_a/b are true minimums received.
    // Fee lookups are cached (the transfers below reuse them).
    let fee_a = if amount_a > 0 {
        crate::transfers::ledger_fee(token_a).await
    } else {
        0
    };
    let fee_b = if amount_b > 0 {
        crate::transfers::ledger_fee(token_b).await
    } else {
        0
    };
    let net_a = amount_a.saturating_sub(fee_a);
    let net_b = amount_b.saturating_sub(fee_b);
    // Audit 2026-06-09 (IC-S-003): a payable leg that nets to zero would be
    // silently consumed (shares burned, reserves debited, nothing sent).
    // Reject the whole removal up front, before the LP burn.
    if (amount_a > 0 && net_a == 0) || (amount_b > 0 && net_b == 0) {
        mutate_state(|s| {
            if let Some(op) = s
                .ingress_operations
                .iter_mut()
                .find(|op| op.id == operation_id)
            {
                op.phase = AmmIngressPhase::Complete;
                op.last_error = Some(
                    "withdrawal amount does not exceed ledger fee; no payout was sent".to_string(),
                );
            }
            let ids = s
                .ingress_operations
                .iter()
                .find(|op| op.id == operation_id)
                .map(|op| op.payout_ids.clone())
                .unwrap_or_default();
            s.pending_payouts.retain(|p| !ids.contains(&p.id));
        });
        return Err(AmmError::InsufficientOutput {
            expected_min: 1,
            actual: 0,
        });
    }
    if net_a < min_amount_a || net_b < min_amount_b {
        mutate_state(|s| {
            if let Some(op) = s
                .ingress_operations
                .iter_mut()
                .find(|op| op.id == operation_id)
            {
                op.phase = AmmIngressPhase::Complete;
                op.last_error =
                    Some("withdrawal slippage check failed; no payout was sent".to_string());
            }
            let ids = s
                .ingress_operations
                .iter()
                .find(|op| op.id == operation_id)
                .map(|op| op.payout_ids.clone())
                .unwrap_or_default();
            s.pending_payouts.retain(|p| !ids.contains(&p.id));
        });
        return Err(AmmError::InsufficientOutput {
            expected_min: min_amount_a.max(min_amount_b),
            actual: net_a.min(net_b),
        });
    }

    // Burn shares and activate the exact staged payout tuples atomically.
    let payout_ids = mutate_state(|s| -> Result<Vec<u64>, AmmError> {
        let payout_ids = s
            .ingress_operations
            .iter()
            .find(|op| op.id == operation_id)
            .map(|op| op.payout_ids.clone())
            .ok_or(AmmError::ClaimNotFound)?;
        for payout_id in &payout_ids {
            if let Some(row) = s.pending_payouts.iter_mut().find(|p| p.id == *payout_id) {
                let fee = if row.ledger == token_a { fee_a } else { fee_b };
                row.fee = Some(fee);
                row.send_amount = Some(row.gross_amount.saturating_sub(fee));
                row.phase = AmmPayoutPhase::Ready;
            }
        }
        let pool = s.pools.get_mut(&pool_id).expect("pool exists");

        // Snapshot pre-update state for reward bookkeeping.
        let existing_caller_shares = pool.lp_shares.get(&caller).copied().unwrap_or(0);
        let acc = pool.acc_reward_per_share;

        // 1. Settle caller's existing rewards before share change.
        // Preserves any claimable across this removal (including full
        // exit), so the caller can still claim_rewards() later.
        {
            let entry = pool.lp_rewards.entry(caller).or_default();
            crate::rewards::settle(entry, existing_caller_shares, acc);
        }

        // 2. Apply share decrement (existing logic).
        {
            let entry = pool.lp_shares.get_mut(&caller).expect("user has shares");
            *entry -= lp_shares;
            if *entry == 0 {
                pool.lp_shares.remove(&caller);
            }
        }
        pool.total_lp_shares -= lp_shares;
        pool.reserve_a -= amount_a;
        pool.reserve_b -= amount_b;

        // 3. Reset reward_debt to the post-update share count.
        // Same accumulator (no drain in this path). If both shares and
        // claimable are zero after settle+reset, prune the entry to
        // free storage. Otherwise the caller may have pending icUSD
        // earnings to claim later.
        let new_caller_shares = pool.lp_shares.get(&caller).copied().unwrap_or(0);
        let should_prune = {
            let entry = pool.lp_rewards.entry(caller).or_default();
            crate::rewards::reset_debt(entry, new_caller_shares, acc);
            new_caller_shares == 0 && entry.claimable == 0
        };
        if should_prune {
            pool.lp_rewards.remove(&caller);
        }
        if let Some(op) = s
            .ingress_operations
            .iter_mut()
            .find(|op| op.id == operation_id)
        {
            op.phase = AmmIngressPhase::Settling;
        }
        s.record_liquidity_event(
            caller,
            pool_id.clone(),
            AmmLiquidityAction::RemoveLiquidity,
            token_a,
            amount_a,
            token_b,
            amount_b,
            lp_shares,
        );
        Ok(payout_ids)
    })?;

    // Send tokens with the identities installed atomically with the share burn.
    // On any ambiguous reply, the durable payout row remains the liability.
    for payout_id in payout_ids {
        if let Err(reason) = process_payout_attempt(payout_id).await {
            log!(
                INFO,
                "WARN: remove_liquidity payout {} held for {}: {}",
                payout_id,
                pool_id,
                reason
            );
            return Err(AmmError::TransferFailed {
                token: "remove_liquidity".to_string(),
                reason,
            });
        }
    }
    let result_bytes = Encode!(&(amount_a, amount_b)).map_err(|e| AmmError::TransferFailed {
        token: "remove_liquidity".to_string(),
        reason: format!("could not encode stored result: {}", e),
    })?;
    mutate_state(|s| {
        if let Some(op) = s
            .ingress_operations
            .iter_mut()
            .find(|op| op.id == operation_id)
        {
            op.phase = AmmIngressPhase::Complete;
            op.result = Some(result_bytes);
            op.last_error = None;
        }
    });

    analytics::invalidate_cache_for_pool(&pool_id);

    log!(
        INFO,
        "Remove liquidity from {}: {} shares -> ({}, {}) for {}",
        pool_id,
        lp_shares,
        amount_a,
        amount_b,
        caller
    );

    Ok((amount_a, amount_b))
}

// ─── Query Endpoints ───

#[query]
fn get_pool(pool_id: PoolId) -> Option<PoolInfo> {
    read_state(|s| s.pools.get(&pool_id).map(|p| p.to_info(&pool_id)))
}

#[query]
fn get_pools() -> Vec<PoolInfo> {
    read_state(|s| s.pools.iter().map(|(id, p)| p.to_info(id)).collect())
}

#[query]
fn get_quote(pool_id: PoolId, token_in: Principal, amount_in: u128) -> Result<u128, AmmError> {
    read_state(|s| {
        let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;

        let (reserve_in, reserve_out) = if token_in == pool.token_a {
            (pool.reserve_a, pool.reserve_b)
        } else if token_in == pool.token_b {
            (pool.reserve_b, pool.reserve_a)
        } else {
            return Err(AmmError::InvalidToken);
        };

        let (amount_out, _, _) = compute_swap(
            reserve_in,
            reserve_out,
            amount_in,
            pool.fee_bps,
            pool.protocol_fee_bps,
        )?;
        Ok(amount_out)
    })
}

#[query]
fn get_lp_balance(pool_id: PoolId, user: Principal) -> u128 {
    read_state(|s| {
        s.pools
            .get(&pool_id)
            .and_then(|p| p.lp_shares.get(&user).copied())
            .unwrap_or(0)
    })
}

#[query]
fn is_pool_creation_open() -> bool {
    read_state(|s| s.pool_creation_open)
}

#[query]
fn is_maintenance_mode() -> bool {
    read_state(|s| s.maintenance_mode)
}

#[query]
fn health() -> String {
    let pool_count = read_state(|s| s.pools.len());
    format!("Rumi AMM OK — {} pool(s)", pool_count)
}

#[query]
fn cycles_status() -> rumi_cycle_manager::CycleManagerCyclesStatus {
    let operational = read_state(|s| !s.maintenance_mode);
    rumi_cycle_manager::self_cycles_status(
        2_000_000_000_000,
        operational,
        rumi_cycle_manager::DEFAULT_FREEZE_THRESHOLD_SECS,
    )
}

#[query]
fn cycle_manager_metrics() -> Vec<rumi_cycle_manager::CycleManagerMetric> {
    read_state(|s| {
        vec![
            rumi_cycle_manager::metric(
                "op:pool:count",
                s.pools.len() as u64,
                s.pools.len() as u64,
                Some("AMM pools"),
            ),
            rumi_cycle_manager::metric(
                "op:swap:count",
                s.swap_events.len() as u64,
                s.swap_events.len() as u64,
                Some("cumulative AMM swap events"),
            ),
            rumi_cycle_manager::metric(
                "op:liquidity:count",
                s.liquidity_events.len() as u64,
                s.liquidity_events.len() as u64,
                Some("cumulative AMM liquidity events"),
            ),
        ]
    })
}

// ─── Swap Event History ───

#[query]
fn get_amm_swap_events(start: u64, length: u64) -> Vec<AmmSwapEvent> {
    read_state(|s| {
        let start = start as usize;
        let length = length.min(MAX_EVENT_PAGE) as usize;
        if start >= s.swap_events.len() {
            return vec![];
        }
        let end = std::cmp::min(start + length, s.swap_events.len());
        s.swap_events[start..end].to_vec()
    })
}

#[query]
fn get_amm_swap_event_count() -> u64 {
    read_state(|s| s.swap_events.len() as u64)
}

// ─── Liquidity Event History ───

#[query]
fn get_amm_liquidity_events(start: u64, length: u64) -> Vec<AmmLiquidityEvent> {
    read_state(|s| {
        let start = start as usize;
        let length = length.min(MAX_EVENT_PAGE) as usize;
        if start >= s.liquidity_events.len() {
            return vec![];
        }
        let end = std::cmp::min(start + length, s.liquidity_events.len());
        s.liquidity_events[start..end].to_vec()
    })
}

#[query]
fn get_amm_liquidity_event_count() -> u64 {
    read_state(|s| s.liquidity_events.len() as u64)
}

// ─── Admin Event History ───

#[query]
fn get_amm_admin_events(start: u64, length: u64) -> Vec<AmmAdminEvent> {
    read_state(|s| {
        let start = start as usize;
        let length = length.min(MAX_EVENT_PAGE) as usize;
        if start >= s.admin_events.len() {
            return vec![];
        }
        let end = std::cmp::min(start + length, s.admin_events.len());
        s.admin_events[start..end].to_vec()
    })
}

#[query]
fn get_amm_admin_event_count() -> u64 {
    read_state(|s| s.admin_events.len() as u64)
}

// ─── Holder Snapshots ───

#[query]
fn get_holder_snapshots(token: String, start: u64, length: u64) -> Vec<HolderSnapshot> {
    read_state(|s| {
        let filtered: Vec<&HolderSnapshot> = s
            .holder_snapshots
            .iter()
            .filter(|snap| snap.token == token)
            .collect();
        let start = start as usize;
        let length = length.min(MAX_EVENT_PAGE) as usize;
        if start >= filtered.len() {
            return vec![];
        }
        let end = std::cmp::min(start + length, filtered.len());
        filtered[start..end].iter().map(|s| (*s).clone()).collect()
    })
}

#[query]
fn get_holder_snapshot_count(token: String) -> u64 {
    read_state(|s| {
        s.holder_snapshots
            .iter()
            .filter(|snap| snap.token == token)
            .count() as u64
    })
}

/// Get the most recent snapshot for a given token.
#[query]
fn get_latest_holder_snapshot(token: String) -> Option<HolderSnapshot> {
    read_state(|s| {
        s.holder_snapshots
            .iter()
            .filter(|snap| snap.token == token)
            .last()
            .cloned()
    })
}

// ─── Analytics: pool time series + rankings ───
//
// These mirror the shape of rumi_3pool's analytics endpoints so the
// Explorer `/e/pool/{id}` page can render either pool source with
// minimal branching. Responses are cached with a 60s TTL and
// invalidated on new swap/liquidity events (see record_* call sites).

#[query]
fn get_amm_volume_series(query: AmmSeriesQuery) -> Vec<AmmVolumePoint> {
    analytics::get_volume_series(query)
}

#[query]
fn get_amm_balance_series(query: AmmSeriesQuery) -> Vec<AmmBalancePoint> {
    analytics::get_balance_series(query)
}

#[query]
fn get_amm_fee_series(query: AmmSeriesQuery) -> Vec<AmmFeePoint> {
    analytics::get_fee_series(query)
}

#[query]
fn get_amm_pool_stats(query: AmmStatsQuery) -> AmmPoolStats {
    analytics::get_pool_stats(query)
}

#[query]
fn get_amm_top_swappers(query: AmmTopSwappersQuery) -> Vec<(Principal, u64, u128)> {
    analytics::get_top_swappers(query)
}

#[query]
fn get_amm_top_lps(query: AmmTopLpsQuery) -> Vec<(Principal, u128, u32)> {
    analytics::get_top_lps(query)
}

#[query]
fn get_amm_swap_events_by_principal(query: AmmEventsByPrincipalQuery) -> Vec<AmmSwapEvent> {
    analytics::get_swap_events_by_principal(query)
}

#[query]
fn get_amm_liquidity_events_by_principal(
    query: AmmEventsByPrincipalQuery,
) -> Vec<AmmLiquidityEvent> {
    analytics::get_liquidity_events_by_principal(query)
}

#[query]
fn get_amm_swap_events_by_time_range(query: AmmEventsByTimeRangeQuery) -> Vec<AmmSwapEvent> {
    analytics::get_swap_events_by_time_range(query)
}

#[query]
pub fn get_amm_reward_series(
    pool_id: PoolId,
    window_days: u32,
) -> Vec<crate::analytics::DailyRewardPoint> {
    read_state(|s| {
        crate::analytics::build_reward_series(
            &s.reward_events,
            &pool_id,
            window_days,
            ic_cdk::api::time(),
        )
    })
}

#[query]
pub fn get_amm_tvl_series(pool_id: PoolId, window_days: u32) -> Vec<TvlSample> {
    read_state(|s| {
        let now = ic_cdk::api::time();
        let cutoff =
            now.saturating_sub((window_days as u64).saturating_mul(86_400 * 1_000_000_000));
        s.tvl_samples
            .iter()
            .filter(|s| s.pool_id == pool_id && s.timestamp >= cutoff)
            .cloned()
            .collect()
    })
}

// ─── ICRC-21 / ICRC-28 / ICRC-10 ───

#[update]
fn icrc21_canister_call_consent_message(
    request: icrc21::ConsentMessageRequest,
) -> icrc21::Icrc21ConsentMessageResult {
    icrc21::icrc21_canister_call_consent_message(request)
}

#[query]
fn icrc28_trusted_origins() -> icrc21::Icrc28TrustedOriginsResponse {
    icrc21::icrc28_trusted_origins()
}

#[query]
fn icrc10_supported_standards() -> Vec<icrc21::StandardRecord> {
    icrc21::icrc10_supported_standards()
}

// ─── Inspect Message (cycle-drain protection) ───
// Runs on a single replica before consensus. NOT a security boundary (can be
// bypassed by a malicious boundary node), but saves cycles by rejecting
// anonymous callers before Candid decoding. Real access control is duplicated
// inside each method.

#[ic_cdk::inspect_message]
fn inspect_message() {
    let method = ic_cdk::api::call::method_name();
    match method.as_str() {
        // ICRC-21 consent messages must accept all callers (wallet integration)
        "icrc21_canister_call_consent_message" => ic_cdk::api::call::accept_message(),
        // All other update methods: reject anonymous to save cycles
        _ => {
            if ic_cdk::api::caller() != Principal::anonymous() {
                ic_cdk::api::call::accept_message();
            }
            // Silently drop anonymous calls
        }
    }
}

// ─── HTTP Request (CoinGecko API) ───

#[query]
fn http_request(req: HttpRequest) -> HttpResponse {
    let path = req.path();

    match path {
        "/api/supply" => {
            let (supply_e8s, _updated_ns) = SUPPLY_CACHE.with(|c| {
                let cache = c.borrow();
                (cache.total_supply_e8s, cache.last_updated_ns)
            });
            // Return total supply with decimals included (CoinGecko requirement)
            let supply_with_decimals = supply_e8s as f64 / 1e8;
            HttpResponseBuilder::ok()
                .header("Content-Type", "text/plain")
                .header("Access-Control-Allow-Origin", "*")
                .with_body_and_content_length(format!("{}", supply_with_decimals))
                .build()
        }
        "/api/supply/raw" => {
            let supply_e8s = SUPPLY_CACHE.with(|c| c.borrow().total_supply_e8s);
            HttpResponseBuilder::ok()
                .header("Content-Type", "text/plain")
                .header("Access-Control-Allow-Origin", "*")
                .with_body_and_content_length(format!("{}", supply_e8s))
                .build()
        }
        _ => HttpResponseBuilder::not_found()
            .with_body_and_content_length("Not found")
            .build(),
    }
}

// ─── Tests ───

#[cfg(test)]
mod dos_001_tests {
    use super::*;

    fn dummy_swap_event(i: u64, pool: &str, who: Principal) -> AmmSwapEvent {
        AmmSwapEvent {
            id: i,
            caller: who,
            pool_id: pool.to_string(),
            token_in: Principal::anonymous(),
            amount_in: 1,
            token_out: Principal::anonymous(),
            amount_out: 1,
            fee: 0,
            timestamp: i,
        }
    }

    #[test]
    fn dos_001_event_queries_clamped_to_max_page() {
        // Audit 2026-06-09 (DOS-001): a huge caller-supplied length must
        // return at most MAX_EVENT_PAGE entries, not the whole history.
        let who = Principal::self_authenticating(&[42]);
        let pool = "dos001-pool";
        let total = MAX_EVENT_PAGE + 100;
        mutate_state(|s| {
            for i in 0..total {
                s.swap_events.push(dummy_swap_event(i, pool, who));
            }
        });

        assert_eq!(
            get_amm_swap_events(0, u64::MAX).len() as u64,
            MAX_EVENT_PAGE
        );
        // In-cap requests are unaffected.
        assert_eq!(get_amm_swap_events(0, 5).len(), 5);

        // The analytics by-principal and time-range variants share the cap.
        let by_principal = analytics::get_swap_events_by_principal(AmmEventsByPrincipalQuery {
            pool: pool.to_string(),
            who,
            start: 0,
            length: u64::MAX,
        });
        assert_eq!(by_principal.len() as u64, MAX_EVENT_PAGE);

        let by_time = analytics::get_swap_events_by_time_range(AmmEventsByTimeRangeQuery {
            pool: pool.to_string(),
            start_ns: 0,
            end_ns: u64::MAX,
            limit: u64::MAX,
        });
        assert_eq!(by_time.len() as u64, MAX_EVENT_PAGE);
    }
}

#[cfg(test)]
mod amm_receipt_tests {
    use super::*;
    use icrc_ledger_types::icrc::generic_value::ICRC3Value as V;
    use std::collections::BTreeMap;

    fn account(owner: Principal, sub: Option<[u8; 32]>) -> V {
        let mut parts = vec![V::Blob(serde_bytes::ByteBuf::from(
            owner.as_slice().to_vec(),
        ))];
        if let Some(sub) = sub {
            parts.push(V::Blob(serde_bytes::ByteBuf::from(sub.to_vec())));
        }
        V::Array(parts)
    }

    fn transfer_block(
        op: &str,
        ts: u64,
        spender: Option<Principal>,
        fee: u128,
    ) -> icrc_ledger_types::icrc3::blocks::BlockWithId {
        let from = Principal::self_authenticating(b"amm receipt from");
        let to = Principal::self_authenticating(b"amm receipt to");
        let sub = [7u8; 32];
        let mut tx = BTreeMap::new();
        tx.insert("op".into(), V::Text(op.into()));
        tx.insert("from".into(), account(from, None));
        tx.insert("to".into(), account(to, Some(sub)));
        tx.insert("amt".into(), V::Nat(Nat::from(123u64)));
        tx.insert("fee".into(), V::Nat(Nat::from(fee)));
        tx.insert(
            "memo".into(),
            V::Blob(serde_bytes::ByteBuf::from(b"same memo".to_vec())),
        );
        tx.insert("ts".into(), V::Nat(Nat::from(ts)));
        if let Some(spender) = spender {
            tx.insert("spender".into(), account(spender, None));
        }
        let mut root = BTreeMap::new();
        root.insert("tx".into(), V::Map(tx));
        icrc_ledger_types::icrc3::blocks::BlockWithId {
            id: Nat::from(4u64),
            block: V::Map(root),
        }
    }

    #[test]
    fn receipt_match_binds_operation_time_fee_and_spender() {
        let from = Principal::self_authenticating(b"amm receipt from");
        let to = Principal::self_authenticating(b"amm receipt to");
        let spender = Principal::self_authenticating(b"amm receipt spender");
        let expected = ExactTransferReceipt {
            from,
            from_subaccount: None,
            to,
            to_subaccount: Some([7u8; 32]),
            amount: 123,
            fee: Some(9),
            memo: b"same memo".to_vec(),
            created_at_time: 77,
            spender: Some(spender),
        };
        assert!(block_matches_exact_receipt(
            &transfer_block("xfer", 77, Some(spender), 9),
            &expected
        )
        .unwrap());
        assert!(!block_matches_exact_receipt(
            &transfer_block("mint", 77, Some(spender), 9),
            &expected
        )
        .unwrap());
        assert!(!block_matches_exact_receipt(
            &transfer_block("xfer", 78, Some(spender), 9),
            &expected
        )
        .unwrap());
        assert!(
            !block_matches_exact_receipt(&transfer_block("xfer", 77, None, 9), &expected).unwrap()
        );
        assert!(!block_matches_exact_receipt(
            &transfer_block("xfer", 77, Some(spender), 10),
            &expected
        )
        .unwrap());
        assert!(block_matches_exact_receipt(
            &transfer_block("future_op", 77, Some(spender), 9),
            &expected
        )
        .is_err());
        assert!(block_matches_exact_receipt(
            &transfer_block("xfer", 77, Some(spender), 9),
            &expected
        )
        .is_ok());
        let mut ordinary_xfer = transfer_block("xfer", 77, Some(spender), 9);
        if let icrc_ledger_types::icrc::generic_value::ICRC3Value::Map(root) =
            &mut ordinary_xfer.block
        {
            if let Some(icrc_ledger_types::icrc::generic_value::ICRC3Value::Map(tx)) =
                root.get_mut("tx")
            {
                tx.remove("ts");
                tx.remove("memo");
            }
        }
        assert!(!block_matches_exact_receipt(&ordinary_xfer, &expected).unwrap());
    }

    #[test]
    fn duplicate_direct_and_archive_block_ids_fail_closed() {
        let direct = transfer_block("xfer", 77, None, 9);
        let mut archived_conflict = transfer_block("xfer", 77, None, 9);
        if let icrc_ledger_types::icrc::generic_value::ICRC3Value::Map(root) =
            &mut archived_conflict.block
        {
            if let Some(icrc_ledger_types::icrc::generic_value::ICRC3Value::Map(tx)) =
                root.get_mut("tx")
            {
                tx.insert(
                    "amt".to_string(),
                    icrc_ledger_types::icrc::generic_value::ICRC3Value::Nat(Nat::from(124u64)),
                );
            }
        }
        let mut blocks = vec![direct, archived_conflict];
        assert!(sort_and_reject_duplicate_block_ids(&mut blocks).is_err());
    }

    #[test]
    fn payout_reprice_requires_no_prior_ambiguous_dispatch_and_rotates_identity() {
        assert!(!payout_phase_has_prior_ambiguity(&AmmPayoutPhase::Ready));
        assert!(!payout_phase_has_prior_ambiguity(
            &AmmPayoutPhase::AwaitingFee
        ));
        assert!(payout_phase_has_prior_ambiguity(&AmmPayoutPhase::Submitted));
        assert!(payout_phase_has_prior_ambiguity(
            &AmmPayoutPhase::HeldUnknown
        ));
        assert_ne!(payout_memo(9, 0), payout_memo(9, 1));
    }

    #[test]
    fn receipt_scan_pages_stop_at_the_persisted_tip() {
        let frozen_tip = 65;
        assert_eq!(receipt_scan_page_end(0, frozen_tip), 32);
        assert_eq!(receipt_scan_page_end(32, frozen_tip), 64);
        assert_eq!(receipt_scan_page_end(64, frozen_tip), 65);
        // New blocks arriving after the frozen tip do not extend this scan.
        assert_eq!(receipt_scan_page_end(65, frozen_tip), 65);
        assert_eq!(receipt_scan_tip(None, 65).unwrap(), 65);
        assert_eq!(receipt_scan_tip(Some(65), 96).unwrap(), 65);
        assert!(receipt_scan_tip(Some(65), 64).is_err());
        assert!(checked_receipt_scan_page_end(66, 65).is_err());
    }

    #[test]
    fn ingress_attempt_generations_have_distinct_exact_memos() {
        let caller = Principal::self_authenticating(b"amm ingress caller");
        let request = [3u8; 32];
        let first = ingress_memo(caller, request, 1, 0);
        let retry = ingress_memo(caller, request, 1, 1);
        assert_ne!(first, retry);
        assert_ne!(first, ingress_memo(caller, request, 0, 0));
    }

    #[test]
    fn first_no_effect_rejection_releases_global_ingress_capacity() {
        let caller = Principal::self_authenticating(b"capacity caller");
        let pool_id = "capacity-pool".to_string();
        let mut state = AmmState::default();
        let complete = AmmIngressOperation {
            id: 0,
            caller,
            request_id: [0; 32],
            pool_id: pool_id.clone(),
            kind: AmmIngressKind::Swap {
                token_in: Principal::anonymous(),
                amount_in: 1,
                min_amount_out: 0,
            },
            legs: Vec::new(),
            payout_ids: Vec::new(),
            confirmed_payout_ids: Vec::new(),
            computed_values: Vec::new(),
            phase: AmmIngressPhase::Complete,
            result: None,
            last_error: None,
        };
        state
            .ingress_operations
            .resize(state::MAX_INGRESS_OPERATIONS - 1, complete.clone());
        let rejected_id = (state::MAX_INGRESS_OPERATIONS - 1) as u64;
        let mut rejected = complete;
        rejected.id = rejected_id;
        rejected.phase = AmmIngressPhase::Rejected { leg_index: 0 };
        rejected.payout_ids.push(8);
        state.ingress_operations.push(rejected);
        state.pending_payouts.push(AmmPayoutAttempt {
            id: 8,
            operation_id: rejected_id,
            pool_id,
            claimant: caller,
            ledger: Principal::anonymous(),
            subaccount: [0; 32],
            gross_amount: 1,
            send_amount: None,
            fee: None,
            memo: Vec::new(),
            created_at_time: 0,
            attempt_generation: 0,
            dispatch_count: 0,
            phase: AmmPayoutPhase::Staged,
            receipt_scan_start: None,
            receipt_scan_cursor: 0,
            receipt_scan_end: None,
            last_error: None,
        });

        assert_eq!(
            state.ingress_operations.len(),
            state::MAX_INGRESS_OPERATIONS
        );
        discard_unaccepted_ingress(&mut state, rejected_id);
        assert_eq!(
            state.ingress_operations.len(),
            state::MAX_INGRESS_OPERATIONS - 1
        );
        assert!(state.pending_payouts.is_empty());
    }
}
