# 3Pool Consolidation Proposal: Unified DeFi Canister Architecture

**Date:** 2026-03-14
**Status:** Proposal
**Author:** Robert Ripley / Claude

---

## Executive Summary

This document proposes merging the 3pool AMM logic into the Rumi Protocol backend canister and converting the existing 3pool canister (`fohh4-yyaaa-aaaap-qtkpa-cai`) into a standalone 3USD ICRC-1 token ledger. The goal is to enable atomic composability between lending (vaults) and trading (swaps) within a single async boundary, following the architectural pattern advocated by Yusan and other ICP-native DeFi protocols.

---

## Background: The Composability Problem on ICP

### How ICP Async Works

On the Internet Computer, each canister processes messages sequentially within its own execution environment. Operations within a single canister call are **atomic** — they either fully succeed or fully revert. However, calls *between* canisters are **asynchronous**: the calling canister sends a message, suspends, and resumes when the response arrives. During this suspension window:

- Other messages can interleave (reentrancy risk)
- The callee can fail, leaving the caller in an intermediate state
- There is no built-in cross-canister transaction rollback

This means **composable DeFi on ICP must happen within the same canister** to guarantee atomicity. This is a well-understood constraint discussed extensively by ICP DeFi builders (see: Yusan/Quint discussion in ICP Garden Discord, October 2025).

### Key Insight from Yusan's Architecture

Yusan (an ICP lending + DEX protocol) puts lending, swaps, and liquidations in a single canister specifically to enable:

1. **Flash loans** — borrow, swap, repay in one atomic call (no inter-canister async)
2. **Atomic liquidations** — seize collateral and swap it in the same execution
3. **MEV resistance** — canister message queues process in order, making front-running difficult
4. **Guaranteed execution** — if you're "in" the canister, operations succeed without depending on external service availability

Yusan also uses a **two-layer ownership model**: users deposit funds into an internal wallet (async ledger transfer), then interact with the lending platform purely through internal state changes (synchronous). Exiting a position moves funds to the internal wallet instantly; withdrawing to an external wallet is a separate async step.

---

## Current Rumi Architecture

### Canister Topology

| Canister | ID (Mainnet) | Responsibility |
|----------|-------------|----------------|
| `rumi_protocol_backend` | `tfesu-vyaaa-aaaap-qrd7a-cai` | Vaults, lending, liquidations, stability pool, interest distribution |
| `rumi_3pool` | `fohh4-yyaaa-aaaap-qtkpa-cai` | StableSwap AMM (swap, add/remove liquidity) + 3USD LP token (ICRC-1/2/3) |
| `vault_frontend` | — | SvelteKit frontend |
| Various ledgers | — | icUSD, ckUSDT, ckUSDC, collateral tokens |

### What Works Well

Rumi's backend already uses a **dual-layer pattern** similar to Yusan's for vault operations:

- **Internal accounting is atomic** — vault debt, collateral amounts, and interest are updated in `mutate_state()` blocks
- **External settlement is async and retryable** — ICRC-1 ledger transfers go through a `PendingMarginTransfer` queue with retry logic
- **Reentrancy protection** — `GuardPrincipal` prevents double-execution during async gaps

### What Doesn't Work

The 3pool being in a **separate canister** creates several limitations:

1. **No atomic swap-and-repay** — A user who wants to swap ckUSDT → icUSD → repay vault must execute 3 separate async calls across 2 canisters. If any step fails, the user is left in an intermediate state.

2. **No atomic liquidation-and-swap** — A liquidator who seizes collateral and wants to immediately swap it to a stablecoin must make a separate call to the 3pool. This adds latency and exposes the liquidator to price risk.

3. **No flash loans** — Impossible across canister boundaries since the borrow-use-repay cycle can't be atomic.

4. **Interest donation is fire-and-forget** — The backend's `donate_to_three_pool()` makes an inter-canister call. If the 3pool is paused or unreachable, the donation silently fails.

5. **3pool transfer failure risk** — If a swap's output transfer fails after the input transfer succeeds, the pool's internal accounting diverges from the ledger. No compensation mechanism exists.

---

## Proposed Architecture

### Overview

Move the 3pool's AMM logic (swap math, liquidity calculations, pool balance accounting) into the backend canister. Convert the existing 3pool canister into a pure 3USD ICRC-1 token ledger.

```
BEFORE:
┌─────────────────────┐     async     ┌─────────────────────────┐
│  Backend Canister    │ ◄──────────► │  3Pool Canister          │
│  - Vaults/Lending    │              │  - Swap math             │
│  - Liquidations      │              │  - Pool balances         │
│  - Stability Pool    │              │  - Add/remove liquidity  │
│  - Interest distrib. │              │  - 3USD token (ICRC-1/2) │
└─────────────────────┘              └─────────────────────────┘

AFTER:
┌──────────────────────────────┐     async     ┌──────────────────┐
│  Backend Canister             │ ◄──────────► │  3USD Ledger      │
│  - Vaults/Lending             │              │  (ICRC-1/2/3)     │
│  - Liquidations               │              │  - LP balances    │
│  - Stability Pool             │              │  - Transfer/      │
│  - Interest distribution      │              │    Approve        │
│  - Swap math (NEW)            │              │  - Tx log         │
│  - Pool balance accounting    │              └──────────────────┘
│    (NEW)                      │
│  - Add/remove liquidity       │        async
│    logic (NEW)                │ ◄──────────► [icUSD, ckUSDT,
└──────────────────────────────┘               ckUSDC ledgers]
```

### What Moves to the Backend

| Component | Source | Notes |
|-----------|--------|-------|
| `swap()` logic | `rumi_3pool/src/swap.rs` | Stableswap math (unchanged) |
| `add_liquidity()` / `remove_liquidity()` logic | `rumi_3pool/src/liquidity.rs` | Curve-style calculations (unchanged) |
| `math.rs` | `rumi_3pool/src/math.rs` | D-invariant, Y-solving (unchanged) |
| Pool balances (`[u128; 3]`) | `rumi_3pool/src/state.rs` | Internal balance accounting |
| Admin fees (`[u128; 3]`) | `rumi_3pool/src/state.rs` | Fee accumulation |
| Pool config (A param, fees) | `rumi_3pool/src/state.rs` | `PoolConfig` struct |
| VP snapshots | `rumi_3pool/src/state.rs` | For APY calculation |

### What Stays in the 3Pool Canister (as 3USD Ledger)

| Component | Notes |
|-----------|-------|
| LP balances (`BTreeMap<Principal, u128>`) | User 3USD holdings |
| ICRC-1 transfer/balance/metadata | Standard token operations |
| ICRC-2 approve/allowance/transfer_from | Delegated transfers |
| ICRC-3 transaction log + certification | Index canister support |
| Minting account = backend canister | Backend can mint/burn LP tokens |

### Option: Replace Custom ICRC Code with Standard Ledger

Instead of keeping the custom `icrc_token.rs` implementation, the 3pool canister could be redeployed with the standard `ic-icrc1-ledger` wasm (already available in `src/ledger/`). This gives:

- Battle-tested, audited ICRC-1/2/3 implementation
- Subaccount support (currently not supported by custom implementation)
- Automatic archive canister support for transaction history
- Reduced maintenance burden

The migration would require snapshotting existing LP balances and passing them as initial balances to the standard ledger. The canister ID would remain the same.

---

## What This Enables

### 1. Atomic Swap + Vault Operations

With swap logic in the backend, a user can:

```
// Single canister call — fully atomic
fn swap_and_repay(vault_id, input_token, input_amount, min_output) {
    // 1. Swap ckUSDT → icUSD (internal balance update, no async)
    let icusd_amount = pool_swap(CKUSDT_INDEX, ICUSD_INDEX, input_amount);

    // 2. Repay vault with icUSD (internal state update, no async)
    reduce_vault_debt(vault_id, icusd_amount);

    // 3. Queue async settlement (retryable, non-blocking)
    queue_pending_transfer(caller, ckusdt_ledger, input_amount);
}
```

### 2. Atomic Liquidation Path

```
// Liquidator calls single function
fn liquidate_and_swap(vault_id) {
    // 1. Liquidate vault — seize collateral (internal state)
    let collateral = execute_liquidation(vault_id);

    // 2. Swap collateral to stablecoin (internal pool balance update)
    let stable_amount = pool_swap(COLLATERAL_INDEX, STABLE_INDEX, collateral);

    // All atomic — async settlement happens afterward via pending transfer queue
}
```

### 3. Interest Donation Simplification

Currently `donate_to_three_pool()` requires an async inter-canister call. After consolidation:

```
// Fully synchronous — no inter-canister call
fn donate_interest_to_pool(amount: u128) {
    mutate_state(|s| {
        s.pool_balances[ICUSD_INDEX] += amount;
        // Virtual price increases for all LP holders automatically
    });
}
```

### 4. Flash Loan Foundation

With lending + swaps in one canister, flash loans become architecturally possible:

```
fn flash_loan(amount, callback_data) {
    // 1. Credit borrower's internal balance
    // 2. Execute user's callback (swap, arbitrage, etc.)
    // 3. Verify repayment + fee
    // All within single message execution — atomic
}
```

This is not a near-term priority but becomes possible with this architecture.

---

## Which Operations Are Atomic vs Async

After consolidation, operations fall into two categories:

### Fully Atomic (No Async Required)

| Operation | Why |
|-----------|-----|
| Swap (token A → token B) | Internal pool balance update only |
| Swap + repay vault | Both are internal state changes |
| Liquidation + swap | Both are internal state changes |
| Interest donation to pool | Direct balance increment |
| Price queries (virtual price, get_dy) | Read-only |

### Still Requires Async (Acceptable)

| Operation | Why | Impact |
|-----------|-----|--------|
| Add liquidity | Must pull tokens from user via ICRC-2 transfer_from, then mint 3USD via call to 3USD ledger | LP operations are not composability-critical |
| Remove liquidity | Must burn 3USD (call to ledger), then push tokens to user | Same as above |
| Deposit collateral into vault | Must pull tokens from user via ICRC transfer | Entry point — async is unavoidable |
| Withdraw collateral from vault | Must push tokens to user | Exit point — uses pending transfer queue |
| Initial token deposit for swap | Must pull input token from user | Entry point |
| Receive swap output | Must push output token to user | Uses pending transfer queue |

**Key insight**: The async boundary moves to the **edges** (deposit/withdraw) rather than being in the **middle** of composable operations. This is exactly the Yusan two-layer pattern.

---

## Migration Strategy

### Phase 1: State Migration Planning

1. Snapshot current 3pool state: pool balances, admin fees, config, VP snapshots
2. Design new state structures in the backend to hold pool data
3. Add pool-related fields to `State` in `state.rs`

### Phase 2: Backend AMM Integration

1. Copy swap math, liquidity math, and pool logic into the backend crate
2. Adapt to use the backend's `mutate_state` / `read_state` pattern
3. Expose new candid endpoints: `pool_swap`, `pool_add_liquidity`, `pool_remove_liquidity`, `pool_get_dy`, `pool_virtual_price`, etc.
4. Add composite operations: `swap_and_repay`, `swap_and_borrow`, etc.
5. Update interest distribution to use direct internal donation

### Phase 3: 3USD Ledger Conversion

**Option A — Keep custom ICRC code:**
1. Strip AMM logic from 3pool canister, keeping only `icrc_token.rs`, `icrc3.rs`, and LP state
2. Add a `minting_account` field set to the backend canister principal
3. Expose `mint` and `burn` endpoints callable only by the backend
4. Deploy upgrade to existing 3pool canister (preserves canister ID and LP balances)

**Option B — Replace with standard ic-icrc1-ledger:**
1. Snapshot all LP balances from current 3pool state
2. Build init args for standard ledger with existing balances as initial accounts
3. Set minting account to backend canister principal
4. Reinstall 3pool canister with standard ledger wasm
5. Verify all balances match post-migration

**Recommendation: Option A first, Option B later.** Option A is less risky (no wasm swap, preserves all state naturally). Option B can be done as a follow-up once the architecture is proven.

### Phase 4: Frontend Updates

1. Update `threePoolService.ts` to call backend canister for swap/liquidity operations instead of 3pool canister
2. Add new composite operation UIs (e.g., "swap and repay" button)
3. Update candid declarations
4. 3USD token queries (balance, transfer) continue to go to the 3pool/3USD ledger canister — no change

### Phase 5: Pool Reserve Migration

The trickiest operational step:

1. The 3pool canister currently holds icUSD, ckUSDT, and ckUSDC in its account on each respective ledger
2. These balances need to move to the backend canister's account
3. This requires the 3pool canister to transfer its reserves to the backend
4. Must be done atomically with the state migration to avoid accounting discrepancy

**Approach:**
1. Pause the 3pool (prevent swaps/liquidity changes during migration)
2. 3pool transfers all token reserves to backend canister
3. Backend records the received balances in its new pool state
4. Unpause — now all swap operations go through the backend

---

## Risk Assessment

### Risks of Doing This

| Risk | Severity | Mitigation |
|------|----------|------------|
| Backend canister grows larger (more code + state) | Medium | ICP canisters support up to 400GB stable memory and 4GB heap. Pool state is tiny (~KB). Code size increase is modest. |
| Migration window requires downtime | Medium | Pause pool, migrate, unpause. Can be done in a single upgrade cycle if planned carefully. |
| Increased blast radius — bug in pool code could affect vaults | Medium | Pool math is well-isolated (pure functions). Use module boundaries within the canister. Thorough testing. |
| 3USD ledger transition could lose balances | High | Snapshot and verify pre/post migration. Use Option A (upgrade, not reinstall) to minimize risk. |
| Frontend needs updates | Low | Straightforward — change which canister ID to call for pool operations. |

### Risks of NOT Doing This

| Risk | Severity | Notes |
|------|----------|-------|
| Users can't atomically swap + repay | Medium | Current UX requires multiple transactions with failure risk between them |
| Liquidators exposed to price risk | Medium | Can't atomically convert seized collateral |
| Interest donations can silently fail | Low | Currently fire-and-forget; pool APY can unexpectedly drop |
| 3pool swap failure leaves inconsistent state | Medium | No compensation mechanism for partial swap failure |
| Competitive disadvantage vs Yusan | Medium | Yusan will offer flash loans and atomic composability |

---

## Estimated Scope

### Backend Changes
- New module: `src/rumi_protocol_backend/src/pool.rs` (~500 lines, mostly moved from 3pool)
- State additions: pool config, balances, admin fees, VP snapshots (~50 lines in `state.rs`)
- New endpoints: ~10 new candid methods
- Updated treasury: simplify `donate_to_three_pool()` to internal operation
- Composite operations: `swap_and_repay`, `swap_and_borrow` (~100 lines each)

### 3Pool Canister Changes
- Remove: `swap.rs`, `liquidity.rs`, `math.rs`, `transfers.rs`, pool balance logic
- Add: `mint`/`burn` endpoints for backend
- Keep: `icrc_token.rs`, `icrc3.rs`, `certification.rs`, LP state

### Frontend Changes
- Update `threePoolService.ts` to point swap/liquidity calls at backend
- Add composite operation flows
- Update candid declarations

### Testing
- Port existing 3pool unit tests to backend context
- Add integration tests for composite operations (swap + repay, etc.)
- Migration dry-run on local replica before mainnet

---

## Open Questions

1. **Should we support subaccounts for 3USD?** The current custom implementation doesn't. The standard ledger does. Subaccount support would matter if 3USD is used as collateral or in other protocols.

2. **Should we implement an internal wallet layer (Yusan-style)?** Users could pre-deposit tokens into an internal balance, making all subsequent operations fully synchronous. This is a larger change but would further reduce async failure surface.

3. **Should the stability pool also move into the backend?** Currently it's in the same canister, so this isn't an issue. But worth confirming the architecture is clean.

4. **Priority vs other roadmap items?** This is a significant refactor. Should it be done before or after other planned features (redemptions, new collateral types, etc.)?

5. **Flash loans — do we want them?** The architecture enables them, but they're a double-edged sword (governance attacks, oracle manipulation). Worth a separate design discussion.

---

## Conclusion

The 3pool consolidation aligns Rumi's architecture with the proven pattern for composable DeFi on ICP. The core insight — that DeFi operations must share an async boundary to be atomic — directly motivates merging swap logic into the backend canister. The 3USD token naturally separates into its own ledger canister, preserving its identity while enabling the backend to manage pool operations atomically alongside vaults, liquidations, and interest distribution.

The migration is non-trivial but well-scoped. The math code moves unchanged, the token code stays in place (or gets upgraded to a standard ledger), and the frontend changes are straightforward. The result is a protocol that can offer atomic swap-and-repay, efficient liquidations, and — eventually — flash loans, all within a single canister execution.

---

## Appendix: Source Conversation

The following Discord conversation from the **ICP Garden** server (#yusan channel, October 23, 2025) between **Enzo** (Yusan founder), **Quint**, and **MrMonkey** sparked this proposal. It lays out the rationale for same-canister DeFi composability on ICP.

---

**Enzo** (10/22/25, 10:46 PM):
> Although cross-chain in nature through onesec, Yusan is here to help the ICP ecosystem with its last missing piece in DeFi

**MrMonkey** (10/23/25, 12:27 AM):
> we have never been so accurately captured

**MrMonkey** (10/23/25, 12:28 AM):
> @Enzo I have edited the message friend
> I have hung your portrait on my wall!

**MrMonkey** (10/23/25, 12:29 AM):
> @Enzo Although cross-chain in nature through onesec, Yusan is here to help the ICP ecosystem with its last missing piece in DeFi
> does this mean we'll have all primitives except flash loans?

**Enzo** (10/23/25, 12:57 AM):
> @MrMonkey does this mean we'll have all primitives except flash loans?
> The entire point of having lending and DEX in the same canister is flash loan!

**Quint** (10/23/25, 12:58 AM):
> @MrMonkey does this mean we'll have all primitives except flash loans?
> Since we (probably) won't have a DEX at launch, yes.

**MrMonkey** (10/23/25, 1:02 AM):
> @Enzo The entire point of having lending and DEX in the same canister is flash loan!
> got it, so that implies all assets involved are owned by your canister, right?
> so you can "transact" by simply switching ownerships flags atomically
> basically the whole transaction in one method with no async calls

**Enzo** (10/23/25, 1:04 AM):
> There's no async call between DeFi operation so you can compose as you wish

**MrMonkey** (10/23/25, 1:04 AM):
> right

**MrMonkey** (10/23/25, 1:04 AM):
> but that in turn implies all the assets are (if I ask, e.g. the ledgers of the tokens involved) owned by your canister, and then you assign internal ownerships that can change without async calls to the ledgers (only make the async calls whenever to settle)
> so basically your DeFi becomes constrained to running in one canister

**Enzo** (10/23/25, 1:08 AM):
> If you want to build composable DeFi on ICP it has to happen within the same async boundary, so the same canister

**MrMonkey** (10/23/25, 1:08 AM):
> agreed

**Enzo** (10/23/25, 1:08 AM):
> Which is why liquidations are in the same canister too

**MrMonkey** (10/23/25, 1:08 AM):
> not criticizing, just trying to understand, and as per usual I do so via limitations, not capacity

**Enzo** (10/23/25, 1:08 AM):
> Were liquidations on Kong, someone could monitor position and manipulate the pool just before a liquidation

**MrMonkey** (10/23/25, 1:09 AM):
> yes I am totally with you
> is everything executed in the order requests come in, making MEV hard as well?

**Quint** (10/23/25, 1:10 AM):
> @MrMonkey is everything executed in the order requests come in, making MEV hard as well?
> Yes, normal canister queues apply.

**MrMonkey** (10/23/25, 1:10 AM):
> @Quint Yes, normal canister queues apply.
> (i don't see MEV as a positive btw, so this is great!)
> ok so the only downside I can see is scalability, which assuming proper queues could for some time be mitigated by trading for longer roundtrips...but there comes a time when those queues build up faster than one canister can process...any plans for that day?
> can people set timeouts or remove their requests from the queue?

**Quint** (10/23/25, 1:14 AM):
> @MrMonkey ok so the only downside I can see is scalability, which assuming proper queues could for some time be mitigated by trading for longer roundtrips...
> I don't have specific plans no, this is a downside, where on other fronts we gain speed...

**Quint** (10/23/25, 1:14 AM):
> @MrMonkey can people set timeouts or remove their requests from the queue?
> You can set timeouts on requests, yes.

**Quint** (10/23/25, 1:15 AM):
> If you look deeper into the code of an agent, you'll see that this is a field.
> The replica should drop requests that have expired.

**MrMonkey** (10/23/25, 1:15 AM):
> not when settlement is taken into account, right? And the speed is what you need to sacrifice (with queues) to keep scaling (for a while)

**MrMonkey** (10/23/25, 1:16 AM):
> @Quint The replica should drop requests that have expired.
> excellent

**Quint** (10/23/25, 1:18 AM):
> @MrMonkey not when settlement is taken into account, right? And the speed is what you need to sacrifice (with queues) to keep scaling (for a while)
> For this, I added another little layer within the canister, you can exit the lending platform, with non-async calls..., little secret.
>
> So you are not limited by inter-canister calls that can take a while.
> If you are in, you are guaranteed it succeeds.

**Quint** (10/23/25, 1:19 AM):
> But again, there is a layer of indirection, which also has some downsides.

**Quint** (10/23/25, 1:20 AM):
> If you are in, you are guaranteed it succeeds.
> If you have funds etc ofc

**MrMonkey** (10/23/25, 1:22 AM):
> I don't understand — there are two kinds of ownership: actual ownership as recorded by the external service (e.g. a ledger) and then there is your internal ownerships, as recorded in your canister. Without async calls, you could only transition from one internal ownership model to another, but never settle the actual ownership in the external service without async. And, respectfully, all internal ownership models are equivalent, the distinction is only in actual, async settlement to the external service

**Quint** (10/23/25, 1:23 AM):
> @MrMonkey I don't understand — there are two kinds of ownership: actual ownership as recorded by the external service (e.g. a ledger) and then there is your internal ownerships, as recorded in your canister...
> No, you are right, I didn't explain it well, let me try again.
>
> So, you need to get funds into the canister, this is the first layer. But at this point you are not yet interacting with the lending platform. You have an internal ledger/wallet that just 'adds funds' to your account.
> Secondly, you need to interact with the lending platform, i.e. supply/borrow etc.
>
> You probably think: but this is two steps?
> - Yes, but this also allows you to exit positions without async calls, i.e. it is just moved to the internal wallet, already yours, you just have to withdraw.
> - To solve the two steps, we natively support bulk actions so you can deposit, supply in one call, still holds the guarantees of atomicity
>
> Is this a little more clear @MrMonkey?
> If not, happy to jump of a call, or do a dev session sometime.

**MrMonkey** (10/23/25, 1:53 AM):
> I think so, I'll let you know once my ancient wetware finishes its chugga chugga
