# Implementation Plan: 3USD/ICP AMM Pool Canister

## Overview

Build a new ICP canister that hosts a constant product (x * y = k) AMM pool pairing 3USD with ICP. The canister is designed to host additional pools in the future. This is the first volatile-pair trading venue within the Rumi Protocol ecosystem.

## Why 3USD/ICP Instead of icUSD/ICP

All stablecoin capital goes into the 3pool. The founder deposits icUSD, ckUSDC, and ckUSDT into the 3pool and receives 3USD. That 3USD goes into the AMM pool paired with ICP. One pool of stablecoin capital, not two. The stablecoins work double duty: providing depth in the 3pool AND backing one side of the ICP trading pair.

A user swapping icUSD for ICP goes through two hops: icUSD -> 3pool -> 3USD, then 3USD -> AMM pool -> ICP. The swap router abstracts this. The extra hop has minimal cost because the 3pool's StableSwap curve has negligible slippage for stablecoin swaps.

## Why a Separate Canister

The pool lives in its own canister, not in the backend, 3pool, or liquidator bot.

- **Fund segregation.** The pool canister has its own principal. Its ICP balance on the ICP ledger is pool ICP. The backend's ICP balance is vault collateral. A bug in one canister cannot drain the other. Enforced at the ledger level, not by code correctness.
- **Independent upgradeability.** Pool upgrades cannot corrupt vault state. Backend upgrades cannot corrupt pool state.
- **Cleaner audit scope.** CDP logic and AMM logic are reviewed independently through a defined Candid interface.
- **Scalability.** Future pools are added to this canister or a new one without touching the backend.

The tradeoff is that liquidation settlement requires inter-canister calls (~1-2 sec latency). At current TVL this is acceptable, and the protocol already handles async calls throughout its architecture.

## AMM Curve: Constant Product (x * y = k)

Standard Uniswap v2 formula. Given reserves (x, y) and input dx of token X:

```
dy = (y * dx) / (x + dx)
```

Concentrated liquidity was rejected: an order of magnitude more complex, solves a capital efficiency problem that does not exist yet (founder is the only LP), and thin constant product pools create larger arb opportunities which benefit the bootstrapping phase.

The canister should include a `CurveType` enum for future flexibility:

```rust
pub enum CurveType {
    ConstantProduct,
    StableSwap { amp: u64 },
    Weighted { weight_a: u64, weight_b: u64 },
}
```

Only `ConstantProduct` needs to be implemented now.

## Multi-Pool Architecture

The canister holds a pool registry. Each pool is identified by its token pair and has its own reserves, fee parameters, LP share tracking, and curve type. Additional pools can be added without deploying a new canister.

### Fund Segregation via Subaccounts

Each pool gets its own subaccount on each token's ledger. On ICP, an ICRC-1 account is the tuple `(principal, subaccount)` where subaccount is a 32-byte blob. Each subaccount has an independent balance on the ledger.

- Pool 1 (3USD/ICP): ICP reserves at `(canister_id, hash("3USD/ICP_ICP"))`, 3USD reserves at `(canister_id, hash("3USD/ICP_3USD"))`
- Future pools: reserves at their own deterministically derived subaccounts

Derivation should be deterministic (e.g., SHA-256 of sorted token pair principals) so anyone can verify which subaccount corresponds to which pool.

Subaccounts are not separate canister IDs. They are the canister principal plus a 32-byte suffix. Block explorers on ICP understand subaccounts and can display per-subaccount balances.

### Curve Flexibility Per Pool

Different pools in the same canister can use different curves. The `Pool` struct contains a `curve` field that dispatches to different math. No constraint forces all pools to share one formula.

## Core Data Structures

```rust
pub struct AmmState {
    pub pools: HashMap<PoolId, Pool>,
}

pub type PoolId = String; // e.g., "3USD_ICP"

pub struct Pool {
    pub token_a: Principal,       // ledger canister ID (e.g., 3USD)
    pub token_b: Principal,       // ledger canister ID (e.g., ICP)
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub fee_bps: u16,             // total swap fee in basis points (e.g., 30 = 0.3%)
    pub protocol_fee_bps: u16,    // protocol's share of fee in bps (0 = 100% to LPs)
    pub curve: CurveType,
    pub lp_shares: HashMap<Principal, u64>,
    pub total_lp_shares: u64,
    pub protocol_fees_a: u64,     // accumulated protocol fees in token A
    pub protocol_fees_b: u64,     // accumulated protocol fees in token B
    pub paused: bool,             // emergency halt flag
    pub subaccount_a: [u8; 32],   // derived deterministically
    pub subaccount_b: [u8; 32],
}
```

## Token Transfer Pattern

Use ICRC-2 approve/transferFrom. This avoids the deposit-then-notify pattern that causes the ICPSwap-style stuck funds problem.

Swap flow:
1. User approves the AMM canister to spend X amount of token A via ICRC-2 `approve`.
2. User calls the AMM canister's `swap` function.
3. AMM canister calls `transferFrom` to pull token A from the user into the pool's subaccount.
4. AMM canister transfers token B from the pool's subaccount to the user.

This is the same pattern Rumi uses for vault collateral deposits and the same pattern KongSwap uses.

## Endpoints to Implement

### Core AMM

- `swap(pool_id, token_in, amount_in, min_amount_out) -> Result<SwapResult, Error>`: Execute a swap. Checks slippage tolerance. Uses ICRC-2 transferFrom to pull input, transfers output from pool subaccount to caller.
- `add_liquidity(pool_id, amount_a, amount_b, min_lp_shares) -> Result<u64, Error>`: Deposit both tokens proportionally. Mint LP shares. On first deposit, lock a small amount of LP shares (e.g., 1000 units) to the zero address to prevent rounding attacks.
- `remove_liquidity(pool_id, lp_shares, min_amount_a, min_amount_b) -> Result<(u64, u64), Error>`: Burn LP shares, return proportional reserves.

### Query Endpoints

- `get_pool(pool_id) -> Option<PoolInfo>`: Return reserves, fee, curve type, total LP shares.
- `get_quote(pool_id, token_in, amount_in) -> u64`: Return expected output amount (for frontend display).
- `get_pools() -> Vec<PoolInfo>`: List all pools.
- `get_lp_balance(pool_id, principal) -> u64`: Return caller's LP share balance.

### Admin (Controller Only)

- `create_pool(token_a, token_b, fee_bps, curve_type) -> Result<PoolId, Error>`: Initialize a new pool. Derive subaccounts deterministically.
- `set_fee(pool_id, fee_bps) -> Result<(), Error>`: Update total swap fee.
- `set_protocol_fee(pool_id, protocol_fee_bps) -> Result<(), Error>`: Update protocol's share of swap fees (0 = 100% to LPs, 10000 = 100% to protocol).
- `withdraw_protocol_fees(pool_id) -> Result<(u64, u64), Error>`: Transfer accumulated protocol fees to admin. Returns amounts withdrawn.
- `pause_pool(pool_id) -> Result<(), Error>`: Emergency halt for a specific pool. Blocks swaps but allows liquidity removal.
- `unpause_pool(pool_id) -> Result<(), Error>`: Resume swaps.

## Integration Points

### Liquidation Flow

The liquidator bot or backend can route liquidations through this pool as one step in the existing liquidation cascade. The pool is called via its public `swap` endpoint like any other user. No special privileged liquidation endpoint is needed.

### Router Integration

The pool needs to implement whatever Candid interface the target open-source router requires for pool discovery and swap execution. This is an open question -- the specific router and its interface need to be identified before implementation.

### Frontend

Add a swap tab to the existing Rumi frontend (app.rumiprotocol.com). The UI calls the AMM canister's `get_quote` for preview and `swap` for execution. Same interaction pattern as the 3pool but targeting a different canister.

## Deployment

- Add the new canister to `dfx.json` in the Rumi project.
- Deploy to local replica for testing, then mainnet.
- Controller setup should match the backend canister's controller configuration.
- Fund with cycles.
- Create the initial 3USD/ICP pool via the `create_pool` admin endpoint.
- Founder adds initial liquidity (~$500-$1,000 per side).

## Resolved Decisions

1. **Swap fee: 30 bps (0.3%).** Fee is split between LPs and protocol treasury via a configurable `protocol_fee_bps` parameter (admin setter). Initially 100% of fees go to LPs (protocol_fee_bps = 0). The split works as: on each swap, `total_fee = amount_in * 30/10000`, then `protocol_cut = total_fee * protocol_fee_bps / 10000`, remainder accrues to LP reserves.
2. **LP token standard: internal accounting only.** No ICRC-1 LP tokens. Each pool tracks `lp_shares: HashMap<Principal, u64>` internally. This keeps multi-pool architecture clean (ICRC-1 is one-token-per-canister), avoids confusing wallets with unknown tokens, and doesn't close doors — thin ICRC-1 wrapper canisters per pool can be added later if composability is needed.
3. **Minimum initial deposit: 1000 units locked to zero address** on first `add_liquidity` call per pool (standard Uniswap v2 approach).
4. **3USD price feed: virtual price from the 3pool canister.** The AMM canister queries the 3pool's `get_pool_status()` to get `virtual_price` (18 decimals, represents USD value of 1 3USD). ICP/USD implied price from pool = `(reserve_3usd / reserve_icp) * virtual_price`. This is needed because 3USD is a yield-bearing stablecoin worth slightly more than $1 and increasing over time.

## Open Questions

1. **Router interface.** Which open-source router? What Candid methods must the pool expose to be discoverable?

## Related Documents

- **Stability pool 3USD reserve change:** See `stability-pool-3usd-reserves-plan.md` for the separate plan to change stability pool liquidations to hold 3USD in protocol reserves instead of redeeming through the 3pool. That change is independent of this AMM pool and can be implemented separately.
- **Arbitrage bot:** A personal (not protocol-operated) arb bot is planned to keep prices aligned between this pool and KongSwap ICP/stablecoin pairs. Architecture for the arb bot is not covered here.
