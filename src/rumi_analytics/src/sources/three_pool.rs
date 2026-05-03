//! Typed wrapper around rumi_3pool queries.
//!
//! Every function here returns Result<T, String> and never panics. Errors
//! propagate up to the caller (collectors), which increment the per-source
//! error counter and skip the snapshot for this tick.
//!
//! The 3pool .did uses `nat` (arbitrary precision) for balances, lp_total_supply,
//! and virtual_price. We decode those as `candid::Nat` then convert to u128.

use candid::{CandidType, Deserialize, Nat, Principal};

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct TokenConfigRaw {
    pub ledger_id: Principal,
    pub symbol: String,
    pub decimals: u8,
    pub precision_mul: u64,
}

/// Raw decoded form of `PoolStatus` using `candid::Nat` for arbitrary-precision
/// fields. Never stored directly; converted to `ThreePoolStatusSubset` immediately.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct PoolStatusRaw {
    pub balances: Vec<Nat>,
    pub lp_total_supply: Nat,
    pub current_a: u64,
    pub virtual_price: Nat,
    pub swap_fee_bps: u64,
    pub admin_fee_bps: u64,
    pub tokens: Vec<TokenConfigRaw>,
}

/// Converted form with all `nat` fields represented as `u128`.
#[derive(Clone, Debug)]
pub struct ThreePoolStatusSubset {
    pub balances: Vec<u128>,
    pub lp_total_supply: u128,
    #[allow(dead_code)]
    pub current_a: u64,
    pub virtual_price: u128,
    #[allow(dead_code)]
    pub swap_fee_bps: u64,
    #[allow(dead_code)]
    pub admin_fee_bps: u64,
    pub decimals: Vec<u8>,
}

fn nat_to_u128(nat: Nat, field: &str) -> Result<u128, String> {
    nat.0
        .try_into()
        .map_err(|e| format!("3pool {} nat -> u128: {}", field, e))
}

pub async fn get_pool_status(three_pool: Principal) -> Result<ThreePoolStatusSubset, String> {
    let res: Result<(PoolStatusRaw,), _> =
        ic_cdk::api::call::call(three_pool, "get_pool_status", ()).await;
    match res {
        Ok((raw,)) => {
            let balances = raw
                .balances
                .into_iter()
                .enumerate()
                .map(|(i, n)| nat_to_u128(n, &format!("balances[{}]", i)))
                .collect::<Result<Vec<_>, _>>()?;
            let decimals = raw.tokens.iter().map(|t| t.decimals).collect();
            Ok(ThreePoolStatusSubset {
                balances,
                lp_total_supply: nat_to_u128(raw.lp_total_supply, "lp_total_supply")?,
                current_a: raw.current_a,
                virtual_price: nat_to_u128(raw.virtual_price, "virtual_price")?,
                swap_fee_bps: raw.swap_fee_bps,
                admin_fee_bps: raw.admin_fee_bps,
                decimals,
            })
        }
        Err((code, msg)) => Err(format!("get_pool_status: {:?} {}", code, msg)),
    }
}

// --- Event tailing (Phase 4) ---

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ThreePoolSwapEvent {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub token_in: u8,
    pub token_out: u8,
    pub amount_in: Nat,
    pub amount_out: Nat,
    pub fee: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ThreePoolLiquidityAction {
    AddLiquidity,
    RemoveLiquidity,
    RemoveOneCoin,
    Donate,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ThreePoolLiquidityEvent {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub action: ThreePoolLiquidityAction,
    pub amounts: Vec<Nat>,
    pub lp_amount: Nat,
    pub coin_index: Option<u8>,
    pub fee: Option<Nat>,
}

pub async fn get_swap_events(
    three_pool: Principal,
    start: u64,
    length: u64,
) -> Result<Vec<ThreePoolSwapEvent>, String> {
    let (events,): (Vec<ThreePoolSwapEvent>,) =
        ic_cdk::call(three_pool, "get_swap_events", (start, length))
            .await
            .map_err(|(code, msg)| format!("get_swap_events: {:?} {}", code, msg))?;
    Ok(events)
}

pub async fn get_swap_event_count(three_pool: Principal) -> Result<u64, String> {
    let (count,): (u64,) = ic_cdk::call(three_pool, "get_swap_event_count", ())
        .await
        .map_err(|(code, msg)| format!("get_swap_event_count: {:?} {}", code, msg))?;
    Ok(count)
}

pub async fn get_liquidity_events(
    three_pool: Principal,
    start: u64,
    length: u64,
) -> Result<Vec<ThreePoolLiquidityEvent>, String> {
    let (events,): (Vec<ThreePoolLiquidityEvent>,) =
        ic_cdk::call(three_pool, "get_liquidity_events", (start, length))
            .await
            .map_err(|(code, msg)| format!("get_liquidity_events: {:?} {}", code, msg))?;
    Ok(events)
}

pub async fn get_liquidity_event_count(three_pool: Principal) -> Result<u64, String> {
    let (count,): (u64,) = ic_cdk::call(three_pool, "get_liquidity_event_count", ())
        .await
        .map_err(|(code, msg)| format!("get_liquidity_event_count: {:?} {}", code, msg))?;
    Ok(count)
}

// --- v2 event tailing ---
//
// rumi_3pool exposes a parallel set of v2 endpoints for swaps and liquidity
// events written under the dynamic-fee schema. v1 is frozen at the migration
// cutoff, v2 is the live log. The frontend `fetchThreePoolSwapEventsCombined`
// merges both — analytics needs the v2 tail too or its evt_swaps mirror
// silently misses every swap since the migration. See
// `tailing/three_pool_swaps_v2.rs` for the consumer.
//
// v2 endpoint quirks worth noting in case future readers wonder:
//
// 1. Pagination semantics differ from v1. v1 is `(start, length)` returning
//    oldest-first; v2 is `(limit, offset)` returning *newest-first*, with
//    `offset` measured from the newest event. The tailer paginates from the
//    oldest unseen entry by computing `offset = total - cursor - batch`.
// 2. There is no `get_swap_event_count_v2` deployed on rumi_3pool today —
//    only `get_liquidity_event_count_v2`. The swap tailer derives its total
//    from the newest event's `id` field (ids are sequential from 0, so
//    `total = newest.id + 1`). When/if a count endpoint lands on rumi_3pool
//    we can switch to it without changing tailer behavior.
// 3. v2 events carry a `migrated` flag set on the v1 entries that were
//    backfilled into v2 at migration time. The tailer skips `migrated == true`
//    entries since their v1 originals are already mirrored via the v1 tailer.

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ThreePoolSwapEventV2 {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub token_in: u8,
    pub token_out: u8,
    pub amount_in: u128,
    pub amount_out: u128,
    pub fee: u128,
    #[allow(dead_code)]
    pub fee_bps: u16,
    #[allow(dead_code)]
    pub imbalance_before: u64,
    #[allow(dead_code)]
    pub imbalance_after: u64,
    #[allow(dead_code)]
    pub is_rebalancing: bool,
    #[allow(dead_code)]
    pub pool_balances_after: [u128; 3],
    #[allow(dead_code)]
    pub virtual_price_after: u128,
    #[serde(default)]
    pub migrated: bool,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ThreePoolLiquidityEventV2 {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub action: ThreePoolLiquidityAction,
    pub amounts: [u128; 3],
    pub lp_amount: u128,
    pub coin_index: Option<u8>,
    pub fee: Option<u128>,
    #[allow(dead_code)]
    pub fee_bps: Option<u16>,
    #[allow(dead_code)]
    pub imbalance_before: u64,
    #[allow(dead_code)]
    pub imbalance_after: u64,
    #[allow(dead_code)]
    pub is_rebalancing: bool,
    #[allow(dead_code)]
    pub pool_balances_after: [u128; 3],
    #[allow(dead_code)]
    pub virtual_price_after: u128,
    #[serde(default)]
    pub migrated: bool,
}

pub async fn get_swap_events_v2(
    three_pool: Principal,
    limit: u64,
    offset: u64,
) -> Result<Vec<ThreePoolSwapEventV2>, String> {
    let (events,): (Vec<ThreePoolSwapEventV2>,) =
        ic_cdk::call(three_pool, "get_swap_events_v2", (limit, offset))
            .await
            .map_err(|(code, msg)| format!("get_swap_events_v2: {:?} {}", code, msg))?;
    Ok(events)
}

pub async fn get_liquidity_event_count_v2(three_pool: Principal) -> Result<u64, String> {
    let (count,): (u64,) = ic_cdk::call(three_pool, "get_liquidity_event_count_v2", ())
        .await
        .map_err(|(code, msg)| format!("get_liquidity_event_count_v2: {:?} {}", code, msg))?;
    Ok(count)
}

pub async fn get_liquidity_events_v2(
    three_pool: Principal,
    limit: u64,
    offset: u64,
) -> Result<Vec<ThreePoolLiquidityEventV2>, String> {
    let (events,): (Vec<ThreePoolLiquidityEventV2>,) =
        ic_cdk::call(three_pool, "get_liquidity_events_v2", (limit, offset))
            .await
            .map_err(|(code, msg)| format!("get_liquidity_events_v2: {:?} {}", code, msg))?;
    Ok(events)
}
