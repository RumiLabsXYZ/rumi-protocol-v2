use candid::{CandidType, Nat, Principal};
use ic_cdk::{query, update, init, pre_upgrade, post_upgrade};
use ic_canister_log::log;
use ic_canisters_http_types::{HttpRequest, HttpResponse, HttpResponseBuilder};
use serde::Deserialize;
use sha2::{Sha256, Digest};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

pub mod types;
pub mod state;
pub mod math;
pub mod rewards;
pub mod transfers;
pub mod icrc21;
pub mod analytics;
mod logs;

use crate::types::*;
use crate::state::{mutate_state, read_state, MAX_PROCESSED_NONCES};
use crate::math::{compute_swap, compute_initial_lp_shares, compute_proportional_lp_shares,
                   compute_remove_liquidity, MINIMUM_LIQUIDITY};
use crate::transfers::{transfer_from_user, transfer_reward_icusd, transfer_to_user};
use crate::logs::INFO;

// ─── Per-pool reentrancy guard ───
// Prevents concurrent async operations on the same pool. On IC, messages
// interleave at every `await` point, so without locking two swaps can read
// the same reserves and drain the pool. The guard is released via Drop,
// which runs even if the callback traps (since ic-cdk 0.5.1).

thread_local! {
    static POOL_LOCKS: RefCell<BTreeSet<PoolId>> = RefCell::new(BTreeSet::new());
}

struct PoolGuard {
    pool_id: PoolId,
}

impl PoolGuard {
    fn new(pool_id: PoolId) -> Result<Self, AmmError> {
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
    let icusd_ledger = Principal::from_text(ICUSD_LEDGER)
        .expect("invalid icUSD ledger principal");
    let acct = Account {
        owner: ic_cdk::id(),
        subaccount: Some(reward_subaccount_for(pool_id)),
    };
    let result: Result<(Nat,), _> = ic_cdk::call(
        icusd_ledger,
        "icrc1_balance_of",
        (acct,),
    ).await;
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
            log!(INFO, "Failed to fetch icUSD total supply: {:?} {}", code, msg);
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
        || { ic_cdk::spawn(take_holder_snapshots()); },
    );
}

async fn take_holder_snapshots() {
    log!(INFO, "Starting daily holder snapshot collection...");

    // Collect icUSD holders
    match collect_icusd_holders().await {
        Ok(snapshot) => {
            log!(INFO, "icUSD snapshot: {} holders, supply {}",
                snapshot.holder_count, snapshot.total_supply);
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
            log!(INFO, "3USD snapshot: {} holders, supply {}",
                snapshot.holder_count, snapshot.total_supply);
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
        (cache.balances.clone(), cache.total_supply, cache.last_processed_index)
    });

    let batch_size: u64 = 2000;

    loop {
        let request = GetTransactionsRequest {
            start: Nat::from(start),
            length: Nat::from(batch_size),
        };

        let (response,): (GetTransactionsResponse,) = ic_cdk::call(
            ledger, "get_transactions", (request,)
        ).await.map_err(|(code, msg)| format!("get_transactions failed: {:?} {}", code, msg))?;

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
                        let fee: u128 = xfer.fee.as_ref()
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
    log!(INFO, "Rumi AMM initialized. Admin: {}", read_state(|s| s.admin));
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
    log!(INFO, "Rumi AMM post-upgrade: state restored. {} pools, {} snapshots",
        read_state(|s| s.pools.len()),
        read_state(|s| s.holder_snapshots.len()));
}

// ─── Helpers ───

fn caller_is_admin() -> Result<(), AmmError> {
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
fn derive_subaccount(pool_id: &str, token_label: &str) -> [u8; 32] {
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
fn make_pool_id(token_a: Principal, token_b: Principal) -> PoolId {
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
    pool_id: &PoolId,
    claimant: Principal,
    token: Principal,
    subaccount: [u8; 32],
    amount: u128,
    reason: &str,
) -> u64 {
    mutate_state(|s| {
        if s.pending_claims.len() >= state::MAX_PENDING_CLAIMS {
            log!(INFO, "WARN: pending_claims at capacity ({}). Dropping oldest claim.", state::MAX_PENDING_CLAIMS);
            s.pending_claims.remove(0);
        }
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

// ─── Admin Endpoints ───

#[update]
fn create_pool(args: CreatePoolArgs) -> Result<PoolId, AmmError> {
    // Admin exempt from maintenance mode — can set up pools while canister is locked
    if read_state(|s| s.maintenance_mode) && caller_is_admin().is_err() {
        return Err(AmmError::MaintenanceMode);
    }

    let is_admin = caller_is_admin().is_ok();

    if !is_admin {
        // Permissionless path: gate must be open, constant product only, fee clamped
        if !read_state(|s| s.pool_creation_open) {
            return Err(AmmError::PoolCreationClosed);
        }
        if args.curve != CurveType::ConstantProduct {
            return Err(AmmError::Unauthorized);
        }
        if args.fee_bps < 1 || args.fee_bps > 1000 {
            return Err(AmmError::FeeBpsOutOfRange);
        }
    }

    // Validate fee_bps for all callers (admin included) to prevent creating
    // permanently broken pools where compute_swap would always error
    if args.fee_bps > 10_000 {
        return Err(AmmError::FeeBpsOutOfRange);
    }

    if args.token_a == args.token_b {
        return Err(AmmError::InvalidToken);
    }

    let pool_id = make_pool_id(args.token_a, args.token_b);

    mutate_state(|s| {
        if s.pools.contains_key(&pool_id) {
            return Err(AmmError::PoolAlreadyExists);
        }

        let subaccount_a = derive_subaccount(&pool_id, "token_a");
        let subaccount_b = derive_subaccount(&pool_id, "token_b");

        // Ensure token_a/token_b are stored in sorted order matching pool_id
        let (token_a, token_b) = if args.token_a.to_text() <= args.token_b.to_text() {
            (args.token_a, args.token_b)
        } else {
            (args.token_b, args.token_a)
        };

        let pool = Pool {
            token_a,
            token_b,
            reserve_a: 0,
            reserve_b: 0,
            fee_bps: args.fee_bps,
            protocol_fee_bps: 0, // 100% to LPs initially
            curve: args.curve,
            lp_shares: BTreeMap::new(),
            total_lp_shares: 0,
            protocol_fees_a: 0,
            protocol_fees_b: 0,
            paused: false,
            subaccount_a,
            subaccount_b,
            lp_rewards: BTreeMap::new(),
            acc_reward_per_share: 0,
            pending_no_lp: 0,
            total_rewards_distributed: 0,
            processed_donation_nonces: std::collections::VecDeque::new(),
            reward_balance_snapshot: 0,
        };

        log!(INFO, "Pool created: {} (fee: {} bps, admin: {})", pool_id, args.fee_bps, is_admin);
        s.pools.insert(pool_id.clone(), pool);
        s.record_admin_event(ic_cdk::caller(), AmmAdminAction::CreatePool {
            pool_id: pool_id.clone(),
            token_a,
            token_b,
            fee_bps: args.fee_bps,
        });
        Ok(pool_id)
    })
}

#[update]
fn set_fee(pool_id: PoolId, fee_bps: u16) -> Result<(), AmmError> {
    let caller = ic_cdk::caller();
    caller_is_admin()?;
    if fee_bps > 10_000 {
        return Err(AmmError::FeeBpsOutOfRange);
    }
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).ok_or(AmmError::PoolNotFound)?;
        pool.fee_bps = fee_bps;
        log!(INFO, "Pool {} fee set to {} bps", pool_id, fee_bps);
        s.record_admin_event(caller, AmmAdminAction::SetFee { pool_id: pool_id.clone(), fee_bps });
        Ok(())
    })
}

#[update]
fn set_protocol_fee(pool_id: PoolId, protocol_fee_bps: u16) -> Result<(), AmmError> {
    let caller = ic_cdk::caller();
    caller_is_admin()?;
    if protocol_fee_bps > 10_000 {
        return Err(AmmError::FeeBpsOutOfRange);
    }
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).ok_or(AmmError::PoolNotFound)?;
        pool.protocol_fee_bps = protocol_fee_bps;
        log!(INFO, "Pool {} protocol fee set to {} bps", pool_id, protocol_fee_bps);
        s.record_admin_event(caller, AmmAdminAction::SetProtocolFee { pool_id: pool_id.clone(), protocol_fee_bps });
        Ok(())
    })
}

#[update]
async fn withdraw_protocol_fees(pool_id: PoolId) -> Result<(u128, u128), AmmError> {
    caller_is_admin()?;

    // Acquire per-pool lock to prevent concurrent fee withdrawals
    let _pool_guard = PoolGuard::new(pool_id.clone())?;

    let (token_a, token_b, sub_a, sub_b, fees_a, fees_b, admin) = read_state(|s| {
        let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;
        Ok::<_, AmmError>((
            pool.token_a, pool.token_b,
            pool.subaccount_a, pool.subaccount_b,
            pool.protocol_fees_a, pool.protocol_fees_b,
            s.admin,
        ))
    })?;

    if fees_a == 0 && fees_b == 0 {
        return Ok((0, 0));
    }

    // Optimistic deduct: zero out fees in state BEFORE transferring.
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).expect("pool must exist: verified at start of withdraw_protocol_fees");
        pool.protocol_fees_a = 0;
        pool.protocol_fees_b = 0;
    });

    let mut withdrawn_a = 0u128;
    let mut withdrawn_b = 0u128;
    let mut errors = Vec::new();

    if fees_a > 0 {
        match transfer_to_user(token_a, sub_a, admin, fees_a).await {
            Ok(_) => withdrawn_a = fees_a,
            Err(reason) => {
                log!(INFO, "WARN: withdraw_protocol_fees transfer_a failed: {}. Rolling back.", reason);
                errors.push(format!("token_a: {}", reason));
            }
        }
    }

    if fees_b > 0 {
        match transfer_to_user(token_b, sub_b, admin, fees_b).await {
            Ok(_) => withdrawn_b = fees_b,
            Err(reason) => {
                log!(INFO, "WARN: withdraw_protocol_fees transfer_b failed: {}. Rolling back.", reason);
                errors.push(format!("token_b: {}", reason));
            }
        }
    }

    // Roll back any fees that failed to transfer
    let rollback_a = fees_a - withdrawn_a;
    let rollback_b = fees_b - withdrawn_b;
    if rollback_a > 0 || rollback_b > 0 {
        mutate_state(|s| {
            let pool = s.pools.get_mut(&pool_id).expect("pool must exist: verified at start of withdraw_protocol_fees");
            pool.protocol_fees_a += rollback_a;
            pool.protocol_fees_b += rollback_b;
        });
    }

    if !errors.is_empty() {
        return Err(AmmError::TransferFailed {
            token: "protocol_fees".to_string(),
            reason: errors.join("; "),
        });
    }

    log!(INFO, "Protocol fees withdrawn from {}: ({}, {})", pool_id, withdrawn_a, withdrawn_b);
    mutate_state(|s| {
        s.record_admin_event(ic_cdk::caller(), AmmAdminAction::WithdrawProtocolFees {
            pool_id: pool_id.clone(),
            amount_a: withdrawn_a,
            amount_b: withdrawn_b,
        });
    });
    Ok((withdrawn_a, withdrawn_b))
}

#[update]
fn pause_pool(pool_id: PoolId) -> Result<(), AmmError> {
    let caller = ic_cdk::caller();
    caller_is_admin()?;
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).ok_or(AmmError::PoolNotFound)?;
        pool.paused = true;
        log!(INFO, "Pool {} paused", pool_id);
        s.record_admin_event(caller, AmmAdminAction::PausePool { pool_id: pool_id.clone() });
        Ok(())
    })
}

#[update]
fn unpause_pool(pool_id: PoolId) -> Result<(), AmmError> {
    let caller = ic_cdk::caller();
    caller_is_admin()?;
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).ok_or(AmmError::PoolNotFound)?;
        pool.paused = false;
        log!(INFO, "Pool {} unpaused", pool_id);
        s.record_admin_event(caller, AmmAdminAction::UnpausePool { pool_id: pool_id.clone() });
        Ok(())
    })
}

#[update]
fn set_pool_creation_open(open: bool) -> Result<(), AmmError> {
    caller_is_admin()?;
    mutate_state(|s| s.pool_creation_open = open);
    log!(INFO, "Pool creation open: {}", open);
    mutate_state(|s| {
        s.record_admin_event(ic_cdk::caller(), AmmAdminAction::SetPoolCreationOpen { open });
    });
    Ok(())
}

#[update]
fn set_admin(new_admin: Principal) -> Result<(), AmmError> {
    caller_is_admin()?;
    if new_admin == Principal::anonymous() {
        return Err(AmmError::Unauthorized);
    }
    let old_admin = read_state(|s| s.admin);
    mutate_state(|s| s.admin = new_admin);
    log!(INFO, "Admin transferred: {} -> {}", old_admin, new_admin);
    Ok(())
}

#[update]
fn set_maintenance_mode(enabled: bool) -> Result<(), AmmError> {
    caller_is_admin()?;
    mutate_state(|s| s.maintenance_mode = enabled);
    log!(INFO, "Maintenance mode: {}", enabled);
    mutate_state(|s| {
        s.record_admin_event(ic_cdk::caller(), AmmAdminAction::SetMaintenanceMode { enabled });
    });
    Ok(())
}

/// Configure which principal is allowed to call `notify_reward_received`.
/// Required before AMM1 earnings distribution can begin. Only callable
/// by admin. Set to the rumi_protocol_backend canister principal.
#[update]
fn set_protocol_backend_principal(principal: Principal) -> Result<(), AmmError> {
    caller_is_admin()?;
    if principal == Principal::anonymous() {
        return Err(AmmError::Unauthorized);
    }
    mutate_state(|s| s.protocol_backend_principal = Some(principal));
    log!(INFO, "Protocol backend principal set to: {}", principal);
    mutate_state(|s| {
        s.record_admin_event(
            ic_cdk::caller(),
            AmmAdminAction::SetProtocolBackendPrincipal { principal },
        );
    });
    Ok(())
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
        log!(INFO, "[notify_reward_received] dedup on nonce {} for pool {}", nonce, pool_id);
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
        let expected = pool.reward_balance_snapshot.saturating_add(amount);
        if on_chain < expected {
            return Err(AmmError::InsufficientOnChainBalance {
                expected,
                actual: on_chain,
            });
        }

        // Bump accumulator (or buffer if no LPs).
        if pool.total_lp_shares > 0 {
            pool.acc_reward_per_share = crate::rewards::accumulate(
                pool.acc_reward_per_share,
                amount,
                pool.total_lp_shares,
            );
        } else {
            pool.pending_no_lp = pool.pending_no_lp.saturating_add(amount);
        }
        pool.total_rewards_distributed = pool.total_rewards_distributed.saturating_add(amount);

        // Update snapshot to match the new on-chain balance.
        pool.reward_balance_snapshot = on_chain;

        // Record nonce + ring-buffer prune.
        pool.processed_donation_nonces.push_back(nonce);
        while pool.processed_donation_nonces.len() > MAX_PROCESSED_NONCES {
            pool.processed_donation_nonces.pop_front();
        }

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
        // Optimistically zero claimable; will be restored on transfer fail.
        entry.claimable = 0;
        Ok(claimable)
    })?;

    // Phase 2: ICRC-1 transfer to caller from the reward subaccount.
    let transfer_result = transfer_reward_icusd(&pool_id, caller, amount).await;

    match transfer_result {
        Ok(_block_index) => {
            // Refetch live balance so the snapshot reflects amount + ledger fee
            // (and any concurrent third-party transfers). Mirrors the
            // notify_reward_received pattern of trusting on-chain truth.
            // On query failure, fall back to subtracting amount + hardcoded fee.
            let after_balance = query_reward_subaccount_balance(&pool_id).await;
            mutate_state(|s| {
                if let Some(pool) = s.pools.get_mut(&pool_id) {
                    match after_balance {
                        Ok(on_chain) => {
                            pool.reward_balance_snapshot = on_chain;
                        }
                        Err(_) => {
                            pool.reward_balance_snapshot = pool
                                .reward_balance_snapshot
                                .saturating_sub(
                                    amount.saturating_add(crate::state::ICUSD_LEDGER_FEE_E8S),
                                );
                        }
                    }
                }
                s.record_claim_event(pool_id.clone(), caller, amount);
            });
            Ok(amount)
        }
        Err(e) => {
            // Restore claimable on failure (the user can retry).
            mutate_state(|s| {
                if let Some(pool) = s.pools.get_mut(&pool_id) {
                    if let Some(entry) = pool.lp_rewards.get_mut(&caller) {
                        entry.claimable = entry.claimable.saturating_add(amount);
                    }
                }
            });
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

    // Atomically find and remove the claim from state (prevents double-claim).
    let claim = mutate_state(|s| {
        let idx = s.pending_claims
            .iter()
            .position(|c| c.id == claim_id)
            .ok_or(AmmError::ClaimNotFound)?;
        let claim = s.pending_claims.remove(idx);
        Ok::<_, AmmError>(claim)
    })?;

    let is_admin = caller_is_admin().is_ok();
    if caller != claim.claimant && !is_admin {
        // Not authorized — re-add the claim before returning error
        mutate_state(|s| s.pending_claims.push(claim));
        return Err(AmmError::Unauthorized);
    }

    let claim_claimant = claim.claimant;
    let claim_amount = claim.amount;

    match transfer_to_user(claim.token, claim.subaccount, claim.claimant, claim.amount).await {
        Ok(_) => {
            log!(INFO, "Pending claim #{} resolved: {} received {} of token {}",
                claim_id, claim_claimant, claim_amount, claim.token);
            mutate_state(|s| {
                s.record_admin_event(caller, AmmAdminAction::ClaimPending {
                    claim_id,
                    claimant: claim_claimant,
                    amount: claim_amount,
                });
            });
            Ok(())
        }
        Err(reason) => {
            // Transfer failed — re-add the claim so user can retry
            log!(INFO, "claim_pending #{} transfer failed: {}. Re-adding claim.", claim_id, reason);
            mutate_state(|s| s.pending_claims.push(claim));
            Err(AmmError::TransferFailed {
                token: claim_id.to_string(),
                reason,
            })
        }
    }
}

/// View all pending claims.
#[query]
fn get_pending_claims() -> Vec<PendingClaim> {
    read_state(|s| s.pending_claims.clone())
}

/// Admin: force-remove a pending claim without transferring (e.g., after manual resolution).
#[update]
fn resolve_pending_claim(claim_id: u64) -> Result<(), AmmError> {
    let caller = ic_cdk::caller();
    caller_is_admin()?;
    mutate_state(|s| {
        let before = s.pending_claims.len();
        s.pending_claims.retain(|c| c.id != claim_id);
        if s.pending_claims.len() == before {
            return Err(AmmError::ClaimNotFound);
        }
        log!(INFO, "Pending claim #{} force-resolved by admin", claim_id);
        s.record_admin_event(caller, AmmAdminAction::ResolvePendingClaim { claim_id });
        Ok(())
    })
}

// ─── Core AMM ───

#[update]
async fn swap(
    pool_id: PoolId,
    token_in: Principal,
    amount_in: u128,
    min_amount_out: u128,
) -> Result<SwapResult, AmmError> {
    if read_state(|s| s.maintenance_mode) {
        return Err(AmmError::MaintenanceMode);
    }
    reject_anonymous()?;

    // Acquire per-pool lock to prevent interleaving attacks across await points
    let _pool_guard = PoolGuard::new(pool_id.clone())?;
    let caller = ic_cdk::caller();

    // Read pool state
    let (token_a, token_b, reserve_a, reserve_b, fee_bps, protocol_fee_bps, sub_a, sub_b, paused) =
        read_state(|s| {
            let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;
            Ok::<_, AmmError>((
                pool.token_a, pool.token_b,
                pool.reserve_a, pool.reserve_b,
                pool.fee_bps, pool.protocol_fee_bps,
                pool.subaccount_a, pool.subaccount_b,
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
    let (amount_out, total_fee, protocol_fee) =
        compute_swap(reserve_in, reserve_out, amount_in, fee_bps, protocol_fee_bps)?;

    if amount_out < min_amount_out {
        return Err(AmmError::InsufficientOutput {
            expected_min: min_amount_out,
            actual: amount_out,
        });
    }

    // Pull input tokens from user
    transfer_from_user(ledger_in, caller, sub_in, amount_in)
        .await
        .map_err(|reason| AmmError::TransferFailed {
            token: "input".to_string(),
            reason,
        })?;

    // Input tokens are now on-ledger in our subaccount — record immediately
    // so state matches on-chain reality even if the output transfer fails.
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).expect("pool must exist: verified at start of swap");
        if is_a_to_b {
            pool.reserve_a += amount_in - protocol_fee;
            pool.protocol_fees_a += protocol_fee;
        } else {
            pool.reserve_b += amount_in - protocol_fee;
            pool.protocol_fees_b += protocol_fee;
        }
    });

    // Send output tokens to user
    match transfer_to_user(ledger_out, sub_out, caller, amount_out).await {
        Ok(_) => {
            // Output sent — deduct from reserves
            mutate_state(|s| {
                let pool = s.pools.get_mut(&pool_id).expect("pool must exist: verified at start of swap");
                if is_a_to_b {
                    pool.reserve_b -= amount_out;
                } else {
                    pool.reserve_a -= amount_out;
                }
            });
        }
        Err(reason) => {
            // Output transfer failed — rollback input reserve change
            mutate_state(|s| {
                let pool = s.pools.get_mut(&pool_id).expect("pool must exist: verified at start of swap");
                if is_a_to_b {
                    pool.reserve_a -= amount_in - protocol_fee;
                    pool.protocol_fees_a -= protocol_fee;
                } else {
                    pool.reserve_b -= amount_in - protocol_fee;
                    pool.protocol_fees_b -= protocol_fee;
                }
            });

            // Attempt to refund input tokens to user
            if let Err(refund_err) = transfer_to_user(ledger_in, sub_in, caller, amount_in).await {
                log!(INFO, "CRITICAL: swap output failed AND input refund failed for {}: {}. \
                     Recording pending claim for {} of {} tokens.", pool_id, refund_err, amount_in, ledger_in);
                record_pending_claim(&pool_id, caller, ledger_in, sub_in, amount_in, &format!(
                    "Swap output transfer failed, then refund failed: {}", refund_err
                ));
            }

            return Err(AmmError::TransferFailed {
                token: "output".to_string(),
                reason,
            });
        }
    }

    // Record swap event for explorer history
    mutate_state(|s| {
        s.record_swap_event(caller, pool_id.clone(), token_in, amount_in, ledger_out, amount_out, total_fee);
    });
    analytics::invalidate_cache_for_pool(&pool_id);

    log!(INFO, "Swap on {}: {} in -> {} out (fee: {}, proto: {})",
        pool_id, amount_in, amount_out, total_fee, protocol_fee);

    Ok(SwapResult {
        amount_out,
        fee: total_fee,
    })
}

#[update]
async fn add_liquidity(
    pool_id: PoolId,
    amount_a: u128,
    amount_b: u128,
    min_lp_shares: u128,
) -> Result<u128, AmmError> {
    if read_state(|s| s.maintenance_mode) {
        return Err(AmmError::MaintenanceMode);
    }
    reject_anonymous()?;

    // Acquire per-pool lock to prevent interleaving attacks across await points
    let _pool_guard = PoolGuard::new(pool_id.clone())?;
    let caller = ic_cdk::caller();

    let (token_a, token_b, reserve_a, reserve_b, total_shares, sub_a, sub_b, paused) =
        read_state(|s| {
            let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;
            Ok::<_, AmmError>((
                pool.token_a, pool.token_b,
                pool.reserve_a, pool.reserve_b,
                pool.total_lp_shares,
                pool.subaccount_a, pool.subaccount_b,
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

    // Pull both tokens from user.
    // If token_b transfer fails after token_a succeeded, refund token_a.
    transfer_from_user(token_a, caller, sub_a, amount_a)
        .await
        .map_err(|reason| AmmError::TransferFailed {
            token: "token_a".to_string(),
            reason,
        })?;

    if let Err(reason) = transfer_from_user(token_b, caller, sub_b, amount_b).await {
        // Refund token_a back to user. If refund fails, record a pending claim.
        if let Err(refund_err) = transfer_to_user(token_a, sub_a, caller, amount_a).await {
            log!(INFO, "CRITICAL: token_b transfer failed AND token_a refund failed: {}. \
                 Recording pending claim for {} of token_a in pool {}.", refund_err, amount_a, pool_id);
            record_pending_claim(&pool_id, caller, token_a, sub_a, amount_a, &format!(
                "add_liquidity token_b failed, then token_a refund failed: {}", refund_err
            ));
        }
        return Err(AmmError::TransferFailed {
            token: "token_b".to_string(),
            reason,
        });
    }

    // Update state (with reward bookkeeping).
    mutate_state(|s| {
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
            pool.lp_shares.insert(Principal::anonymous(), MINIMUM_LIQUIDITY);
            *pool.lp_shares.entry(caller).or_insert(0) += user_shares;
            pool.total_lp_shares = shares;

            log!(INFO, "Initial liquidity for {}: {} shares ({} locked)",
                pool_id, shares, MINIMUM_LIQUIDITY);
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
            log!(INFO, "[add_liquidity] drained pending_no_lp {} into acc for pool {}", buffered, pool_id);
        }
    });

    mutate_state(|s| {
        s.record_liquidity_event(
            caller, pool_id.clone(), AmmLiquidityAction::AddLiquidity,
            token_a, amount_a, token_b, amount_b, shares,
        );
    });
    analytics::invalidate_cache_for_pool(&pool_id);

    log!(INFO, "Add liquidity to {}: ({}, {}) -> {} shares for {}",
        pool_id, amount_a, amount_b, shares, caller);

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
    reject_anonymous()?;

    // Acquire per-pool lock to prevent interleaving attacks across await points
    let _pool_guard = PoolGuard::new(pool_id.clone())?;
    let caller = ic_cdk::caller();

    let (token_a, token_b, reserve_a, reserve_b, total_shares, sub_a, sub_b, user_shares, paused) =
        read_state(|s| {
            let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;
            let user_shares = pool.lp_shares.get(&caller).copied().unwrap_or(0);
            Ok::<_, AmmError>((
                pool.token_a, pool.token_b,
                pool.reserve_a, pool.reserve_b,
                pool.total_lp_shares,
                pool.subaccount_a, pool.subaccount_b,
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

    let (amount_a, amount_b) = compute_remove_liquidity(lp_shares, reserve_a, reserve_b, total_shares)?;

    if amount_a < min_amount_a || amount_b < min_amount_b {
        return Err(AmmError::InsufficientOutput {
            expected_min: min_amount_a.max(min_amount_b),
            actual: amount_a.min(amount_b),
        });
    }

    // Burn LP shares and update reserves FIRST (optimistic),
    // then transfer tokens. This ensures the protocol never overpays
    // if a transfer fails mid-way.
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).expect("pool exists");
        let entry = pool.lp_shares.get_mut(&caller).expect("user has shares");
        *entry -= lp_shares;
        if *entry == 0 {
            pool.lp_shares.remove(&caller);
        }
        pool.total_lp_shares -= lp_shares;
        pool.reserve_a -= amount_a;
        pool.reserve_b -= amount_b;
    });

    // Send tokens to user. If either fails, shares are already burned
    // but tokens remain in the pool subaccount. Record pending claims.
    let mut transfer_errors = Vec::new();

    if amount_a > 0 {
        if let Err(reason) = transfer_to_user(token_a, sub_a, caller, amount_a).await {
            log!(INFO, "WARN: remove_liquidity transfer_a failed for {}: {}. Recording pending claim.", pool_id, reason);
            record_pending_claim(&pool_id, caller, token_a, sub_a, amount_a, &format!(
                "remove_liquidity transfer_a failed: {}", reason
            ));
            transfer_errors.push(format!("token_a: {}", reason));
        }
    }

    if amount_b > 0 {
        if let Err(reason) = transfer_to_user(token_b, sub_b, caller, amount_b).await {
            log!(INFO, "WARN: remove_liquidity transfer_b failed for {}: {}. Recording pending claim.", pool_id, reason);
            record_pending_claim(&pool_id, caller, token_b, sub_b, amount_b, &format!(
                "remove_liquidity transfer_b failed: {}", reason
            ));
            transfer_errors.push(format!("token_b: {}", reason));
        }
    }

    if !transfer_errors.is_empty() {
        return Err(AmmError::TransferFailed {
            token: "output".to_string(),
            reason: format!("{}. Pending claims recorded — retry via claim_pending().", transfer_errors.join("; ")),
        });
    }

    mutate_state(|s| {
        s.record_liquidity_event(
            caller, pool_id.clone(), AmmLiquidityAction::RemoveLiquidity,
            token_a, amount_a, token_b, amount_b, lp_shares,
        );
    });
    analytics::invalidate_cache_for_pool(&pool_id);

    log!(INFO, "Remove liquidity from {}: {} shares -> ({}, {}) for {}",
        pool_id, lp_shares, amount_a, amount_b, caller);

    Ok((amount_a, amount_b))
}

// ─── Query Endpoints ───

#[query]
fn get_pool(pool_id: PoolId) -> Option<PoolInfo> {
    read_state(|s| s.pools.get(&pool_id).map(|p| p.to_info(&pool_id)))
}

#[query]
fn get_pools() -> Vec<PoolInfo> {
    read_state(|s| {
        s.pools.iter().map(|(id, p)| p.to_info(id)).collect()
    })
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
            reserve_in, reserve_out, amount_in, pool.fee_bps, pool.protocol_fee_bps,
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

// ─── Swap Event History ───

#[query]
fn get_amm_swap_events(start: u64, length: u64) -> Vec<AmmSwapEvent> {
    read_state(|s| {
        let start = start as usize;
        let length = length as usize;
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
        let length = length as usize;
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
        let length = length as usize;
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
        let filtered: Vec<&HolderSnapshot> = s.holder_snapshots
            .iter()
            .filter(|snap| snap.token == token)
            .collect();
        let start = start as usize;
        let length = length as usize;
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
        s.holder_snapshots.iter().filter(|snap| snap.token == token).count() as u64
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
        _ => {
            HttpResponseBuilder::not_found()
                .with_body_and_content_length("Not found")
                .build()
        }
    }
}
