# ICPswap Routing Integration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add ICPswap as a swap provider alongside Rumi AMM, quote both per swap, and route the user through whichever path produces the best output. Enables Rob to migrate LP capital from Rumi AMM to ICPswap's 3USD/ICP pool without degrading user experience.

**Architecture:** Frontend-only refactor. Introduce a `SwapProvider` abstraction (Rumi AMM, ICPswap 3USD/ICP, ICPswap icUSD/ICP). The router fans out quotes in parallel and picks the best output per swap. No canister code changes. Existing route types stay valid; new route types get added for ICPswap-direct paths.

**Tech Stack:** SvelteKit, TypeScript, `@dfinity/agent`, `@slide-computer/signer-agent` (Oisy batching), vitest.

---

## Context & Decisions (from 2026-04-15 brainstorm)

**Problem:** Rob is spread across three LP positions (Rumi AMM 3USD/ICP, ICPswap icUSD/ICP, ICPswap 3USD/ICP). He wants to consolidate capital toward ICPswap's 3USD/ICP pool for visibility, rankings, and volume-bot reduction, but keep Rumi AMM alive with reduced liquidity and keep icUSD/ICP at marketing-size depth.

**Strategic allocation (Option B from brainstorm):**
- ~70-80% of Rumi AMM capital migrates to ICPswap 3USD/ICP (manual op, outside code scope).
- Rumi AMM keeps ~20% for redundancy and fee earning.
- ICPswap icUSD/ICP stays at current size as "billboard" plus small-swap venue.
- 3pool and 3USD token are unchanged.

**Routing design:** Quote every relevant provider for each swap, pick best output. Do NOT hard-code "always through 3pool" (taxes users and degrades pool imbalance). Let liquidity allocation drive natural routing: where capital is deep, quotes win organically.

**Out of scope for this plan:**
- Arb bot rewrite (separate workstream).
- LP capital migration (manual via Rob's wallet).
- Split routing across providers (V2 optimization, not needed for MVP).
- KongSwap provider (noted in 2026-03-09 research; add later if needed).
- Canister changes (Rumi AMM stays live with a lower LP share; no pause flag needed).

---

## Task 1 Findings (applied to later tasks)

From `docs/superpowers/plans/2026-04-15-icpswap-routing-research.md` (commit `c288e9c`):

- **ICPswap 3USD/ICP pool:** `mu2zw-6iaaa-aaaar-qb56q-cai` (30 bps). token0 = 3USD, token1 = ICP.
- **ICPswap icUSD/ICP pool:** `nqxwe-hiaaa-aaaar-qb5yq-cai` (30 bps). **token0 = ICP, token1 = icUSD** (opposite of the plan's initial assumption — Task 11 Step 1 uses the correct order below).
- Both pools share byte-identical candid (one declaration set covers both).
- V3-style pools with a combined **`depositFromAndSwap`** method — **use it wherever the plan currently shows separate `depositFrom` + `swap` calls.** It's one call, one ICRC-2 approval, atomic from the UI's perspective. An explicit `withdraw` is still required afterward (V3 pools keep output on internal subaccount until withdrawn).
- `DepositArgs`/`WithdrawArgs.fee` is the ledger's ICRC-1 fee (query `icrc1_fee()` on the input/output ledger; don't hardcode).
- Both pools are thin (3USD/ICP ~$1k, icUSD/ICP ~$3.3k). Quote-both-pick-best still works — Rumi AMM will just win most routes until LPs deepen the ICPswap side. Default UI slippage should be 1–2%.

---

## File Structure

**New files:**
- `src/declarations/icpswap_pool/icpswap_pool.did` - Candid for an ICPswap v3-style pool canister.
- `src/declarations/icpswap_pool/icpswap_pool.did.js` - Generated IDL factory.
- `src/declarations/icpswap_pool/icpswap_pool.did.d.ts` - TypeScript types.
- `src/vault_frontend/src/lib/services/providers/types.ts` - `SwapProvider` interface and quote types.
- `src/vault_frontend/src/lib/services/providers/rumiAmmProvider.ts` - Wraps existing `ammService`.
- `src/vault_frontend/src/lib/services/providers/icpswapProvider.ts` - ICPswap deposit/swap/withdraw flow.
- `src/vault_frontend/src/lib/services/providers/providerRegistry.ts` - Instantiates providers, exposes `quoteAll()` and `bestQuote()`.
- `src/vault_frontend/src/lib/services/providers/rumiAmmProvider.spec.ts` - Unit tests.
- `src/vault_frontend/src/lib/services/providers/icpswapProvider.spec.ts` - Unit tests (mocked pool).
- `src/vault_frontend/src/lib/services/providers/providerRegistry.spec.ts` - Unit tests for quote selection.

**Modified files:**
- `src/vault_frontend/src/lib/config.ts` - Add ICPswap canister IDs and IDL export.
- `src/vault_frontend/src/lib/services/swapRouter.ts` - Use provider registry instead of direct `ammService` calls.
- `src/vault_frontend/src/lib/components/swap/SwapInterface.svelte` - Show selected provider in route preview.
- `src/vault_frontend/src/lib/services/pnp.ts` - Register ICPswap pool canister for ICRC-21 consent.

---

## Task 1: Verify ICPswap interface and gather pool canister IDs

**Files:** None (research task, output is a markdown note for Task 2).

This is a no-code task. Verification of external interfaces must happen before we generate declarations.

- [ ] **Step 1: Fetch ICPswap pool candid from ICPSwap-Labs**

Get the pool canister `.did` by querying one of Rob's LP pools directly. On mainnet:

```bash
dfx canister --network ic metadata <POOL_ID> candid:service > /tmp/icpswap_pool.did
```

If Rob can provide the pool canister IDs, use those. Otherwise, look them up:

```bash
# Query NodeIndex for all pools
dfx canister --network ic call ggzvv-5qaaa-aaaag-qck7a-cai getAllPools '()'
```

Filter output for pools containing the icUSD ledger `t6bor-paaaa-aaaap-qrd5q-cai` and the 3USD ledger `fohh4-yyaaa-aaaap-qtkpa-cai`.

- [ ] **Step 2: Identify the exact swap interface**

Read the `.did` file. Determine whether the pool is V3-style (sqrtPrice, zeroForOne) or V2-style (constant product). Confirm presence of:
- `quote` (query method) - calculates expected output without state change.
- `deposit` or `depositFrom` (update method) - pulls input token via ICRC-2.
- `swap` (update method) - executes the swap once deposited.
- `withdraw` (update method) - sends output token back to caller.
- `metadata` (query method) - returns pool info including `token0`, `token1`, `fee`, `sqrtPriceX96` (if V3).

If there is a combined `depositAndSwap` or `swapExactInputSingle` method, prefer it to reduce calls.

- [ ] **Step 3: Note pool IDs and fee tier**

Write to the plan's running notes (below):
- ICPswap 3USD/ICP pool canister ID.
- ICPswap icUSD/ICP pool canister ID.
- Fee tier for each (3 bps, 30 bps, 100 bps).
- Whether `token0` is icUSD or ICP in the icUSD/ICP pool (affects `zeroForOne` flag).
- Whether `token0` is 3USD or ICP in the 3USD/ICP pool.

- [ ] **Step 4: Verify liquidity depth**

```bash
dfx canister --network ic call <POOL_ID> metadata '()'
```

Note the reserves / sqrt price. Confirm depth is sufficient for the swap sizes we expect to route through each pool. If icUSD/ICP is below ~$5k depth, flag in plan - quote-both will naturally skip it for mid-size swaps.

- [ ] **Step 5: Commit research notes**

Create `docs/superpowers/plans/2026-04-15-icpswap-routing-research.md` with the pool IDs, interface summary, and depth findings. Commit:

```bash
git add docs/superpowers/plans/2026-04-15-icpswap-routing-research.md
git commit -m "docs: ICPswap pool interface and canister ID research"
```

---

## Task 2: Add ICPswap pool declarations to the repo

**Files:**
- Create: `src/declarations/icpswap_pool/icpswap_pool.did`
- Create: `src/declarations/icpswap_pool/icpswap_pool.did.js`
- Create: `src/declarations/icpswap_pool/icpswap_pool.did.d.ts`
- Create: `src/declarations/icpswap_pool/index.js`
- Create: `src/declarations/icpswap_pool/index.d.ts`

- [ ] **Step 1: Copy the verified .did file**

From Task 1 Step 1, place the pool candid at `src/declarations/icpswap_pool/icpswap_pool.did`. Follow the pattern from `src/declarations/rumi_amm/` for layout.

- [ ] **Step 2: Generate IDL factory and types**

```bash
cd src/declarations/icpswap_pool
didc bind icpswap_pool.did -t js > icpswap_pool.did.js
didc bind icpswap_pool.did -t ts > icpswap_pool.did.d.ts
```

If `didc` is not installed, use the `rumi_amm` declarations as a template and translate by hand (the generated output has a predictable structure).

- [ ] **Step 3: Create the index files**

`src/declarations/icpswap_pool/index.js`:

```javascript
export { idlFactory } from "./icpswap_pool.did.js";
```

`src/declarations/icpswap_pool/index.d.ts`:

```typescript
export { idlFactory, _SERVICE } from "./icpswap_pool.did";
```

Intentionally minimal (different from `src/declarations/rumi_amm/index.js`). The `rumi_amm` barrel hardcodes a `CANISTER_ID_RUMI_AMM` env var and exports a `createActor` helper. ICPswap has many pool canisters (one per token pair), not a single canister ID, so a default `createActor` would be misleading. Consumers (see Task 3) import `idlFactory` directly from `.did.js` rather than through this index, so the richer pattern would also be dead code.

- [ ] **Step 4: Verify the declarations compile**

```bash
cd src/vault_frontend && npm run check
```

Expected: zero new type errors.

- [ ] **Step 5: Commit**

```bash
git add src/declarations/icpswap_pool/
git commit -m "feat(declarations): add ICPswap pool candid bindings"
```

---

## Task 3: Add ICPswap canister IDs and IDL to config

**Files:**
- Modify: `src/vault_frontend/src/lib/config.ts`

- [ ] **Step 1: Add the ICPswap pool IDs to `CANISTER_IDS`**

In `src/vault_frontend/src/lib/config.ts`, add to the `CANISTER_IDS` object (use pool IDs from Task 1 Step 3; placeholders shown):

```typescript
// ICPswap pools (external DEX for routing)
ICPSWAP_3USD_ICP_POOL: "<ID_FROM_TASK_1>",
ICPSWAP_ICUSD_ICP_POOL: "<ID_FROM_TASK_1>",
```

Add the IDL import at the top of the file:

```typescript
import { idlFactory as icpswapPoolIDL } from '$declarations/icpswap_pool/icpswap_pool.did.js';
```

And export it inside the `CONFIG` object at the bottom:

```typescript
// Export IDLs through config for convenience
rumi_backendIDL,
icp_ledgerIDL,
icusd_ledgerIDL,
threePoolIDL,
rumiAmmIDL,
icusdIndexIDL,
analyticsIDL,
icpswapPoolIDL,
```

- [ ] **Step 2: Add getter methods on CONFIG**

Inside the `CONFIG` object, add:

```typescript
get icpswap3UsdIcpPoolId() {
  return CANISTER_IDS.ICPSWAP_3USD_ICP_POOL;
},

get icpswapIcUsdIcpPoolId() {
  return CANISTER_IDS.ICPSWAP_ICUSD_ICP_POOL;
},
```

- [ ] **Step 3: Verify**

```bash
cd src/vault_frontend && npm run check
```

Expected: zero type errors.

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/lib/config.ts
git commit -m "feat(config): register ICPswap 3USD/ICP and icUSD/ICP pool canister IDs"
```

---

## Task 4: Define the SwapProvider interface and quote types

**Files:**
- Create: `src/vault_frontend/src/lib/services/providers/types.ts`

- [ ] **Step 1: Write the interface**

Create `src/vault_frontend/src/lib/services/providers/types.ts`:

```typescript
import type { AmmToken } from '../ammService';

export interface ProviderQuote {
  /** Provider identifier, e.g. "rumi_amm" or "icpswap_3usd_icp". */
  provider: ProviderId;
  /** Token pair summary for display, e.g. "3USD/ICP via Rumi AMM". */
  label: string;
  /** Estimated output in raw units of the output token. */
  amountOut: bigint;
  /** Fee percentage (for display only), e.g. "0.30%". */
  feeDisplay: string;
  /** Estimated price impact in basis points (0 to 10000). */
  priceImpactBps: number;
  /** Provider-specific hints passed back to `swap()` (e.g. pool ID for Rumi AMM). */
  meta: Record<string, unknown>;
}

export interface ProviderSwapResult {
  /** Actual output amount received. */
  amountOut: bigint;
}

export type ProviderId =
  | 'rumi_amm'
  | 'icpswap_3usd_icp'
  | 'icpswap_icusd_icp';

export interface SwapProvider {
  readonly id: ProviderId;

  /** Whether this provider can quote and swap this pair. */
  supports(tokenIn: AmmToken, tokenOut: AmmToken): boolean;

  /** Query-only quote. Must not mutate state. */
  quote(tokenIn: AmmToken, tokenOut: AmmToken, amountIn: bigint): Promise<ProviderQuote>;

  /**
   * Execute the swap. Assumes the caller already has a quote from `quote()`
   * and passes it back so the provider can reuse cached lookups.
   */
  swap(
    tokenIn: AmmToken,
    tokenOut: AmmToken,
    amountIn: bigint,
    minOut: bigint,
    quote: ProviderQuote,
  ): Promise<ProviderSwapResult>;
}
```

- [ ] **Step 2: Verify compile**

```bash
cd src/vault_frontend && npm run check
```

Expected: zero type errors.

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/services/providers/types.ts
git commit -m "feat(swap): define SwapProvider interface and quote types"
```

---

## Task 5: Implement RumiAmmProvider (wraps existing ammService)

**Files:**
- Create: `src/vault_frontend/src/lib/services/providers/rumiAmmProvider.ts`
- Create: `src/vault_frontend/src/lib/services/providers/rumiAmmProvider.spec.ts`

- [ ] **Step 1: Write the failing test**

Create `src/vault_frontend/src/lib/services/providers/rumiAmmProvider.spec.ts`:

```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { Principal } from '@dfinity/principal';

// Mock ammService before importing the provider
vi.mock('../ammService', () => ({
  ammService: {
    getPools: vi.fn(),
    getQuote: vi.fn(),
    swap: vi.fn(),
  },
  AMM_TOKENS: [],
}));

import { ammService } from '../ammService';
import { RumiAmmProvider } from './rumiAmmProvider';
import type { AmmToken } from '../ammService';

const tokenThreeUsd: AmmToken = {
  symbol: '3USD', ledgerId: 'fohh4-yyaaa-aaaap-qtkpa-cai',
  decimals: 8, threePoolIndex: -1, is3USD: true,
} as AmmToken;
const tokenIcp: AmmToken = {
  symbol: 'ICP', ledgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
  decimals: 8, threePoolIndex: -1, is3USD: false,
} as AmmToken;

describe('RumiAmmProvider', () => {
  let provider: RumiAmmProvider;
  beforeEach(() => {
    vi.clearAllMocks();
    provider = new RumiAmmProvider();
  });

  it('supports 3USD <-> ICP', () => {
    expect(provider.supports(tokenThreeUsd, tokenIcp)).toBe(true);
    expect(provider.supports(tokenIcp, tokenThreeUsd)).toBe(true);
  });

  it('does not support stable <-> stable', () => {
    const stable: AmmToken = { ...tokenIcp, symbol: 'ckUSDT', threePoolIndex: 1 };
    expect(provider.supports(stable, tokenIcp)).toBe(false);
  });

  it('returns a quote with pool ID cached in meta', async () => {
    vi.mocked(ammService.getPools).mockResolvedValue([
      { pool_id: 'pool-abc',
        token_a: Principal.fromText(tokenThreeUsd.ledgerId),
        token_b: Principal.fromText(tokenIcp.ledgerId),
      } as any,
    ]);
    vi.mocked(ammService.getQuote).mockResolvedValue(500_000_000n);

    const q = await provider.quote(tokenThreeUsd, tokenIcp, 100_000_000n);

    expect(q.provider).toBe('rumi_amm');
    expect(q.amountOut).toBe(500_000_000n);
    expect(q.meta.poolId).toBe('pool-abc');
    expect(q.feeDisplay).toBe('0.30%');
  });
});
```

- [ ] **Step 2: Run test to verify failure**

```bash
cd src/vault_frontend && npx vitest run src/lib/services/providers/rumiAmmProvider.spec.ts
```

Expected: FAIL with module not found.

- [ ] **Step 3: Implement the provider**

Create `src/vault_frontend/src/lib/services/providers/rumiAmmProvider.ts`:

```typescript
import { Principal } from '@dfinity/principal';
import { ammService, type AmmToken } from '../ammService';
import { CANISTER_IDS } from '../../config';
import type { SwapProvider, ProviderQuote, ProviderSwapResult } from './types';

const FEE_DISPLAY = '0.30%';
const FEE_BPS = 30;

export class RumiAmmProvider implements SwapProvider {
  readonly id = 'rumi_amm' as const;
  private _cachedPoolId: string | null = null;

  supports(tokenIn: AmmToken, tokenOut: AmmToken): boolean {
    const isPair = (a: AmmToken, b: AmmToken) => a.is3USD && b.symbol === 'ICP';
    return isPair(tokenIn, tokenOut) || isPair(tokenOut, tokenIn);
  }

  async quote(tokenIn: AmmToken, tokenOut: AmmToken, amountIn: bigint): Promise<ProviderQuote> {
    const poolId = await this.getPoolId();
    const tokenInPrincipal = Principal.fromText(tokenIn.ledgerId);
    const amountOut = await ammService.getQuote(poolId, tokenInPrincipal, amountIn);
    return {
      provider: this.id,
      label: `${tokenIn.symbol} -> ${tokenOut.symbol} via Rumi AMM`,
      amountOut,
      feeDisplay: FEE_DISPLAY,
      priceImpactBps: this.estimatePriceImpactBps(amountIn, amountOut, tokenIn, tokenOut),
      meta: { poolId, feeBps: FEE_BPS },
    };
  }

  async swap(
    tokenIn: AmmToken, tokenOut: AmmToken, amountIn: bigint, minOut: bigint, quote: ProviderQuote,
  ): Promise<ProviderSwapResult> {
    const poolId = quote.meta.poolId as string;
    const tokenInPrincipal = Principal.fromText(tokenIn.ledgerId);
    const result = await ammService.swap(poolId, tokenInPrincipal, amountIn, minOut, tokenIn);
    return { amountOut: result.amount_out };
  }

  private async getPoolId(): Promise<string> {
    if (this._cachedPoolId) return this._cachedPoolId;
    const pools = await ammService.getPools();
    const threeUsd = CANISTER_IDS.THREEPOOL;
    const icp = CANISTER_IDS.ICP_LEDGER;
    const pool = pools.find((p: { pool_id: string; token_a: Principal; token_b: Principal }) => {
      const a = p.token_a.toText();
      const b = p.token_b.toText();
      return (a === threeUsd && b === icp) || (a === icp && b === threeUsd);
    });
    if (!pool) throw new Error('3USD/ICP Rumi AMM pool not found');
    this._cachedPoolId = pool.pool_id;
    return pool.pool_id;
  }

  private estimatePriceImpactBps(
    amountIn: bigint, amountOut: bigint, tokenIn: AmmToken, tokenOut: AmmToken,
  ): number {
    // Rough estimate; a precise value would require reserves. Leave at 0 for MVP
    // and improve later if the UI needs finer slippage guidance.
    return 0;
  }
}
```

- [ ] **Step 4: Run test to verify pass**

```bash
cd src/vault_frontend && npx vitest run src/lib/services/providers/rumiAmmProvider.spec.ts
```

Expected: PASS (all 3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/services/providers/rumiAmmProvider.ts src/vault_frontend/src/lib/services/providers/rumiAmmProvider.spec.ts
git commit -m "feat(swap): add RumiAmmProvider wrapping ammService"
```

---

## Task 6: Implement IcpswapProvider (quote method)

**Files:**
- Create: `src/vault_frontend/src/lib/services/providers/icpswapProvider.ts`
- Create: `src/vault_frontend/src/lib/services/providers/icpswapProvider.spec.ts`

Note: this task implements only `quote()`. The `swap()` method is added in Task 7.

- [ ] **Step 1: Write the failing test**

Create `src/vault_frontend/src/lib/services/providers/icpswapProvider.spec.ts`:

```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@dfinity/agent', () => ({
  Actor: { createActor: vi.fn() },
  HttpAgent: { create: vi.fn().mockResolvedValue({}) },
}));

import { Actor } from '@dfinity/agent';
import { IcpswapProvider } from './icpswapProvider';
import type { AmmToken } from '../ammService';

const icUsd: AmmToken = {
  symbol: 'icUSD', ledgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
  decimals: 8, threePoolIndex: 0, is3USD: false,
} as AmmToken;
const icp: AmmToken = {
  symbol: 'ICP', ledgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
  decimals: 8, threePoolIndex: -1, is3USD: false,
} as AmmToken;

describe('IcpswapProvider (quote)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('id is icpswap_icusd_icp when constructed for icUSD/ICP pool', () => {
    const provider = new IcpswapProvider({
      id: 'icpswap_icusd_icp',
      poolCanisterId: 'abc-icusd',
      token0LedgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
      token1LedgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
      feeBps: 30,
    });
    expect(provider.id).toBe('icpswap_icusd_icp');
  });

  it('supports the configured token pair in both directions', () => {
    const provider = new IcpswapProvider({
      id: 'icpswap_icusd_icp',
      poolCanisterId: 'abc-icusd',
      token0LedgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
      token1LedgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
      feeBps: 30,
    });
    expect(provider.supports(icUsd, icp)).toBe(true);
    expect(provider.supports(icp, icUsd)).toBe(true);
  });

  it('calls pool.quote with zeroForOne=true when tokenIn is token0', async () => {
    const mockPool = {
      quote: vi.fn().mockResolvedValue({ ok: '1000000000' }),
    };
    vi.mocked(Actor.createActor).mockReturnValue(mockPool as any);

    const provider = new IcpswapProvider({
      id: 'icpswap_icusd_icp',
      poolCanisterId: 'abc-icusd',
      token0LedgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
      token1LedgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
      feeBps: 30,
    });

    const q = await provider.quote(icUsd, icp, 500_000_000n);

    expect(q.provider).toBe('icpswap_icusd_icp');
    expect(q.amountOut).toBe(1_000_000_000n);
    expect(mockPool.quote).toHaveBeenCalledWith({
      amountIn: '500000000',
      zeroForOne: true,
      amountOutMinimum: '0',
    });
  });
});
```

- [ ] **Step 2: Run test to verify failure**

```bash
cd src/vault_frontend && npx vitest run src/lib/services/providers/icpswapProvider.spec.ts
```

Expected: FAIL (module not found).

- [ ] **Step 3: Implement the provider (quote only, swap stub for now)**

Create `src/vault_frontend/src/lib/services/providers/icpswapProvider.ts`:

```typescript
import { Actor, HttpAgent } from '@dfinity/agent';
import type { _SERVICE as IcpswapPool } from '$declarations/icpswap_pool/icpswap_pool.did';
import { idlFactory as icpswapPoolIDL } from '$declarations/icpswap_pool/icpswap_pool.did.js';
import { CONFIG } from '../../config';
import type { AmmToken } from '../ammService';
import type { SwapProvider, ProviderQuote, ProviderSwapResult, ProviderId } from './types';

export interface IcpswapProviderConfig {
  id: Extract<ProviderId, 'icpswap_3usd_icp' | 'icpswap_icusd_icp'>;
  /** ICPswap pool canister ID. */
  poolCanisterId: string;
  /** Ledger IDs as declared in the pool metadata (token0, token1). */
  token0LedgerId: string;
  token1LedgerId: string;
  /** Fee tier in basis points (3, 30, or 100). */
  feeBps: number;
}

export class IcpswapProvider implements SwapProvider {
  readonly id: ProviderId;
  private readonly config: IcpswapProviderConfig;
  private _actor: IcpswapPool | null = null;

  constructor(config: IcpswapProviderConfig) {
    this.id = config.id;
    this.config = config;
  }

  supports(tokenIn: AmmToken, tokenOut: AmmToken): boolean {
    const { token0LedgerId, token1LedgerId } = this.config;
    const isPair = (a: string, b: string) =>
      (a === token0LedgerId && b === token1LedgerId) ||
      (a === token1LedgerId && b === token0LedgerId);
    return isPair(tokenIn.ledgerId, tokenOut.ledgerId);
  }

  async quote(tokenIn: AmmToken, tokenOut: AmmToken, amountIn: bigint): Promise<ProviderQuote> {
    const pool = await this.getActor();
    const zeroForOne = tokenIn.ledgerId === this.config.token0LedgerId;
    const result = await pool.quote({
      amountIn: amountIn.toString(),
      zeroForOne,
      amountOutMinimum: '0',
    });
    const amountOut = this.unwrapResult(result);
    const feePct = (this.config.feeBps / 100).toFixed(2);
    return {
      provider: this.id,
      label: `${tokenIn.symbol} -> ${tokenOut.symbol} via ICPswap`,
      amountOut,
      feeDisplay: `${feePct}%`,
      priceImpactBps: 0, // refined in future
      meta: {
        poolCanisterId: this.config.poolCanisterId,
        zeroForOne,
      },
    };
  }

  async swap(
    _tokenIn: AmmToken, _tokenOut: AmmToken, _amountIn: bigint, _minOut: bigint, _quote: ProviderQuote,
  ): Promise<ProviderSwapResult> {
    throw new Error('IcpswapProvider.swap not implemented (see Task 7)');
  }

  private async getActor(): Promise<IcpswapPool> {
    if (this._actor) return this._actor;
    const agent = await HttpAgent.create({ host: CONFIG.host });
    this._actor = Actor.createActor<IcpswapPool>(icpswapPoolIDL, {
      agent,
      canisterId: this.config.poolCanisterId,
    });
    return this._actor;
  }

  private unwrapResult(result: { ok: string } | { err: unknown }): bigint {
    if ('ok' in result) return BigInt(result.ok);
    throw new Error(`ICPswap quote failed: ${JSON.stringify(result.err)}`);
  }
}
```

Note: if the ICPswap candid uses different result shapes than `{ok: string} | {err}`, adjust based on the actual `.did.d.ts` from Task 2. The structure above matches ICPswap's documented V3 pool interface.

- [ ] **Step 4: Run test to verify pass**

```bash
cd src/vault_frontend && npx vitest run src/lib/services/providers/icpswapProvider.spec.ts
```

Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/services/providers/icpswapProvider.ts src/vault_frontend/src/lib/services/providers/icpswapProvider.spec.ts
git commit -m "feat(swap): add IcpswapProvider with quote() implementation"
```

---

## Task 7: Implement IcpswapProvider.swap() for standard wallets

**Files:**
- Modify: `src/vault_frontend/src/lib/services/providers/icpswapProvider.ts`
- Modify: `src/vault_frontend/src/lib/services/providers/icpswapProvider.spec.ts`

ICPswap's V3 pool flow (verified in Task 1): `depositFrom` (ICRC-2 pull) -> `swap` -> `withdraw`. If the verified `.did` includes `depositFromAndSwap`, prefer that and combine the first two steps.

- [ ] **Step 1: Write the failing test**

Append to `src/vault_frontend/src/lib/services/providers/icpswapProvider.spec.ts`:

```typescript
describe('IcpswapProvider.swap', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('executes depositFrom -> swap -> withdraw and returns the withdrawn amount', async () => {
    const mockPool = {
      depositFrom: vi.fn().mockResolvedValue({ ok: '500000000' }),
      swap: vi.fn().mockResolvedValue({ ok: '495000000' }),
      withdraw: vi.fn().mockResolvedValue({ ok: '495000000' }),
    };
    vi.mocked(Actor.createActor).mockReturnValue(mockPool as any);

    // Also mock the input-token ledger for the ICRC-2 approval call.
    // In this test we rely on the approval happening inside swap(); if the
    // provider delegates approval to an external helper, mock that instead.

    const provider = new IcpswapProvider({
      id: 'icpswap_icusd_icp',
      poolCanisterId: 'abc-icusd',
      token0LedgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
      token1LedgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
      feeBps: 30,
    });

    const quote = await provider.quote(icUsd, icp, 500_000_000n);
    const result = await provider.swap(icUsd, icp, 500_000_000n, 490_000_000n, quote);

    expect(result.amountOut).toBe(495_000_000n);
    expect(mockPool.depositFrom).toHaveBeenCalled();
    expect(mockPool.swap).toHaveBeenCalled();
    expect(mockPool.withdraw).toHaveBeenCalled();
  });

  it('throws if the withdrawn amount is below minOut', async () => {
    const mockPool = {
      depositFrom: vi.fn().mockResolvedValue({ ok: '500000000' }),
      swap: vi.fn().mockResolvedValue({ ok: '100000000' }),
      withdraw: vi.fn().mockResolvedValue({ ok: '100000000' }),
    };
    vi.mocked(Actor.createActor).mockReturnValue(mockPool as any);

    const provider = new IcpswapProvider({
      id: 'icpswap_icusd_icp',
      poolCanisterId: 'abc-icusd',
      token0LedgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
      token1LedgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
      feeBps: 30,
    });

    const quote = await provider.quote(icUsd, icp, 500_000_000n);
    await expect(
      provider.swap(icUsd, icp, 500_000_000n, 490_000_000n, quote)
    ).rejects.toThrow(/slippage/i);
  });
});
```

- [ ] **Step 2: Run test to verify failure**

```bash
cd src/vault_frontend && npx vitest run src/lib/services/providers/icpswapProvider.spec.ts -t "IcpswapProvider.swap"
```

Expected: FAIL (swap throws "not implemented").

- [ ] **Step 3: Implement swap()**

Replace the `swap` stub in `icpswapProvider.ts`:

```typescript
async swap(
  tokenIn: AmmToken, tokenOut: AmmToken, amountIn: bigint, minOut: bigint, quote: ProviderQuote,
): Promise<ProviderSwapResult> {
  const pool = await this.getActor();
  const zeroForOne = quote.meta.zeroForOne as boolean;

  // Step 1: depositFrom pulls tokens via ICRC-2 (caller must have pre-approved
  // the pool canister). Approval is the caller's responsibility -- a future
  // refactor may absorb it into swap(), but for MVP swapRouter handles it.
  const depositResult = await pool.depositFrom({
    token: tokenIn.ledgerId,
    amount: amountIn.toString(),
    fee: 0, // pool-specific; may need lookup if non-zero
  });
  this.unwrapResult(depositResult);

  // Step 2: swap within the pool
  const swapResult = await pool.swap({
    amountIn: amountIn.toString(),
    zeroForOne,
    amountOutMinimum: minOut.toString(),
  });
  const swapOut = this.unwrapResult(swapResult);

  // Step 3: withdraw output to caller
  const withdrawResult = await pool.withdraw({
    token: tokenOut.ledgerId,
    amount: swapOut.toString(),
    fee: 0,
  });
  const withdrawn = this.unwrapResult(withdrawResult);

  if (withdrawn < minOut) {
    throw new Error(`ICPswap swap failed slippage check: got ${withdrawn}, minimum ${minOut}`);
  }

  return { amountOut: withdrawn };
}
```

Note: approval of the input-token allowance to the pool canister is handled in the swapRouter (Task 9). If your verified `.did` in Task 1 exposes a `depositFromAndSwap` method, collapse steps 1 and 2.

- [ ] **Step 4: Run test to verify pass**

```bash
cd src/vault_frontend && npx vitest run src/lib/services/providers/icpswapProvider.spec.ts
```

Expected: PASS (5 tests total).

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/services/providers/icpswapProvider.ts src/vault_frontend/src/lib/services/providers/icpswapProvider.spec.ts
git commit -m "feat(swap): implement IcpswapProvider.swap 3-step flow"
```

---

## Task 8: Create the provider registry with best-quote selection

**Files:**
- Create: `src/vault_frontend/src/lib/services/providers/providerRegistry.ts`
- Create: `src/vault_frontend/src/lib/services/providers/providerRegistry.spec.ts`

- [ ] **Step 1: Write the failing test**

Create `src/vault_frontend/src/lib/services/providers/providerRegistry.spec.ts`:

```typescript
import { describe, it, expect, vi } from 'vitest';
import type { SwapProvider, ProviderQuote } from './types';
import type { AmmToken } from '../ammService';
import { ProviderRegistry } from './providerRegistry';

const tokenA: AmmToken = { symbol: 'A', ledgerId: 'aaa', decimals: 8, threePoolIndex: -1, is3USD: false } as AmmToken;
const tokenB: AmmToken = { symbol: 'B', ledgerId: 'bbb', decimals: 8, threePoolIndex: -1, is3USD: false } as AmmToken;

function makeProvider(id: string, amountOut: bigint): SwapProvider {
  return {
    id: id as any,
    supports: () => true,
    quote: vi.fn().mockResolvedValue({
      provider: id, label: `${id} label`, amountOut, feeDisplay: '0.30%', priceImpactBps: 0, meta: {},
    } as ProviderQuote),
    swap: vi.fn(),
  };
}

describe('ProviderRegistry', () => {
  it('quoteAll returns quotes from every supporting provider', async () => {
    const reg = new ProviderRegistry([
      makeProvider('rumi_amm', 100n),
      makeProvider('icpswap_3usd_icp', 110n),
    ]);
    const quotes = await reg.quoteAll(tokenA, tokenB, 500n);
    expect(quotes).toHaveLength(2);
    expect(quotes.map(q => q.amountOut).sort()).toEqual([100n, 110n]);
  });

  it('bestQuote picks the provider with the highest amountOut', async () => {
    const reg = new ProviderRegistry([
      makeProvider('rumi_amm', 100n),
      makeProvider('icpswap_3usd_icp', 110n),
    ]);
    const best = await reg.bestQuote(tokenA, tokenB, 500n);
    expect(best.provider).toBe('icpswap_3usd_icp');
    expect(best.amountOut).toBe(110n);
  });

  it('bestQuote skips providers that throw during quote', async () => {
    const working = makeProvider('rumi_amm', 100n);
    const broken = makeProvider('icpswap_3usd_icp', 0n);
    (broken.quote as any).mockRejectedValue(new Error('pool paused'));
    const reg = new ProviderRegistry([working, broken]);
    const best = await reg.bestQuote(tokenA, tokenB, 500n);
    expect(best.provider).toBe('rumi_amm');
  });

  it('throws when no provider supports the pair', async () => {
    const reg = new ProviderRegistry([
      { ...makeProvider('rumi_amm', 100n), supports: () => false } as SwapProvider,
    ]);
    await expect(reg.bestQuote(tokenA, tokenB, 500n)).rejects.toThrow(/no provider/i);
  });
});
```

- [ ] **Step 2: Run test to verify failure**

```bash
cd src/vault_frontend && npx vitest run src/lib/services/providers/providerRegistry.spec.ts
```

Expected: FAIL (module not found).

- [ ] **Step 3: Implement the registry**

Create `src/vault_frontend/src/lib/services/providers/providerRegistry.ts`:

```typescript
import type { AmmToken } from '../ammService';
import type { SwapProvider, ProviderQuote } from './types';

export class ProviderRegistry {
  constructor(private readonly providers: SwapProvider[]) {}

  /**
   * Quote every provider that supports the pair, in parallel. Providers that
   * throw are silently skipped -- their failures show up as missing entries
   * in the returned array.
   */
  async quoteAll(tokenIn: AmmToken, tokenOut: AmmToken, amountIn: bigint): Promise<ProviderQuote[]> {
    const supporting = this.providers.filter(p => p.supports(tokenIn, tokenOut));
    const results = await Promise.allSettled(
      supporting.map(p => p.quote(tokenIn, tokenOut, amountIn))
    );
    return results
      .filter((r): r is PromiseFulfilledResult<ProviderQuote> => r.status === 'fulfilled')
      .map(r => r.value);
  }

  /**
   * Returns the quote with the highest amountOut. Throws if no provider
   * supports the pair or all providers erroneously returned zero output.
   */
  async bestQuote(tokenIn: AmmToken, tokenOut: AmmToken, amountIn: bigint): Promise<ProviderQuote> {
    const quotes = await this.quoteAll(tokenIn, tokenOut, amountIn);
    if (quotes.length === 0) {
      throw new Error(`No provider supports ${tokenIn.symbol} -> ${tokenOut.symbol}`);
    }
    return quotes.reduce((best, q) => (q.amountOut > best.amountOut ? q : best));
  }

  /** Return the provider instance by ID. */
  get(id: string): SwapProvider {
    const p = this.providers.find(x => x.id === id);
    if (!p) throw new Error(`Unknown provider: ${id}`);
    return p;
  }
}
```

- [ ] **Step 4: Run test to verify pass**

```bash
cd src/vault_frontend && npx vitest run src/lib/services/providers/providerRegistry.spec.ts
```

Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/services/providers/providerRegistry.ts src/vault_frontend/src/lib/services/providers/providerRegistry.spec.ts
git commit -m "feat(swap): add ProviderRegistry with best-quote selection"
```

---

## Task 9: Refactor swapRouter to use the provider registry for 3USD/ICP routes

**Files:**
- Modify: `src/vault_frontend/src/lib/services/swapRouter.ts`

Scope: replace direct `ammService` calls for routes involving the 3USD/ICP hop (`amm_swap`, `stable_to_icp`, `icp_to_stable`) with `ProviderRegistry.bestQuote()`. Routes that don't touch 3USD/ICP (`three_pool_swap`, `three_pool_deposit`, `three_pool_redeem`) are untouched.

- [ ] **Step 1: Add module-level provider registry**

At the top of `src/vault_frontend/src/lib/services/swapRouter.ts`, after the existing imports, add:

```typescript
import { RumiAmmProvider } from './providers/rumiAmmProvider';
import { IcpswapProvider } from './providers/icpswapProvider';
import { ProviderRegistry } from './providers/providerRegistry';
import { CANISTER_IDS } from '../config';

const _threeUsdIcpRegistry = new ProviderRegistry([
  new RumiAmmProvider(),
  new IcpswapProvider({
    id: 'icpswap_3usd_icp',
    poolCanisterId: CANISTER_IDS.ICPSWAP_3USD_ICP_POOL,
    // token0/token1 are filled in from the pool metadata; for now assume
    // 3USD is token0 and ICP is token1. If Task 1 Step 3 showed the opposite,
    // swap these two lines.
    token0LedgerId: CANISTER_IDS.THREEPOOL,
    token1LedgerId: CANISTER_IDS.ICP_LEDGER,
    feeBps: 30,
  }),
]);
```

- [ ] **Step 2: Update `resolveRoute` Case 4 (3USD <-> ICP direct)**

Replace the existing Case 4 block (lines 127-140 of the current `swapRouter.ts`) with:

```typescript
// Case 4: 3USD <-> ICP (best of Rumi AMM and ICPswap 3USD/ICP)
if ((is3USD(from) && isICP(to)) || (isICP(from) && is3USD(to))) {
  const quote = await _threeUsdIcpRegistry.bestQuote(from, to, amountIn);
  return {
    type: 'amm_swap',
    pathDisplay: quote.label,
    hops: 1,
    estimatedOutput: quote.amountOut,
    feeDisplay: quote.feeDisplay,
    providerQuote: quote,
  };
}
```

- [ ] **Step 3: Update `SwapRoute` to carry the winning provider quote**

In the `SwapRoute` interface near the top of `swapRouter.ts`, add:

```typescript
export interface SwapRoute {
  type: RouteType;
  pathDisplay: string;
  hops: number;
  estimatedOutput: bigint;
  feeDisplay: string;
  intermediateOutput?: bigint;
  poolId?: string;
  /** For routes using the 3USD/ICP hop, the chosen provider's quote. */
  providerQuote?: import('./providers/types').ProviderQuote;
  /** For two-hop routes, the 3USD/ICP hop's provider quote. */
  hopProviderQuote?: import('./providers/types').ProviderQuote;
}
```

Remove the `poolId` references where they are fully replaced by `providerQuote.meta`.

- [ ] **Step 4: Update `resolveRoute` Cases 5 and 6 (stable <-> ICP two-hop)**

Replace Cases 5 and 6 with:

```typescript
// Case 5: Stablecoin -> ICP (3pool deposit + best-of 3USD/ICP)
if (isStablecoin(from) && isICP(to)) {
  const amounts = [0n, 0n, 0n];
  amounts[from.threePoolIndex] = amountIn;
  const threeUsdOut = await threePoolService.calcAddLiquidity(amounts);

  const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
  const hopQuote = await _threeUsdIcpRegistry.bestQuote(threeUsdToken, to, threeUsdOut);

  return {
    type: 'stable_to_icp',
    pathDisplay: `${from.symbol} -> 3USD -> ICP (${hopQuote.provider})`,
    hops: 2,
    estimatedOutput: hopQuote.amountOut,
    feeDisplay: hopQuote.feeDisplay,
    intermediateOutput: threeUsdOut,
    hopProviderQuote: hopQuote,
  };
}

// Case 6: ICP -> Stablecoin (best-of 3USD/ICP + 3pool redeem)
if (isICP(from) && isStablecoin(to)) {
  const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
  const hopQuote = await _threeUsdIcpRegistry.bestQuote(from, threeUsdToken, amountIn);
  const stableOut = await threePoolService.calcRemoveOneCoin(hopQuote.amountOut, to.threePoolIndex);

  return {
    type: 'icp_to_stable',
    pathDisplay: `ICP -> 3USD -> ${to.symbol} (${hopQuote.provider})`,
    hops: 2,
    estimatedOutput: stableOut,
    feeDisplay: hopQuote.feeDisplay,
    intermediateOutput: hopQuote.amountOut,
    hopProviderQuote: hopQuote,
  };
}
```

- [ ] **Step 5: Update `executeRoute` to dispatch via the provider**

In `executeRoute`, replace Case `amm_swap` with:

```typescript
case 'amm_swap': {
  const q = route.providerQuote!;
  const provider = _threeUsdIcpRegistry.get(q.provider);
  const result = await provider.swap(from, to, amountIn, minOutput, q);
  return result.amountOut;
}
```

Replace the non-Oisy branches of `stable_to_icp` and `icp_to_stable` with:

```typescript
case 'stable_to_icp': {
  if (isOisyWallet()) return executeStableToIcpOisy(route, from, to, amountIn, slippageBps);

  // Non-Oisy sequential: 3pool deposit, then best-provider swap on 3USD -> ICP
  const amounts = [0n, 0n, 0n];
  amounts[from.threePoolIndex] = amountIn;
  const threeUsdMinOut = route.intermediateOutput! * BigInt(10000 - slippageBps) / 10000n;
  const threeUsdOut = await threePoolService.addLiquidity(amounts, threeUsdMinOut);

  const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
  const hopQuote = route.hopProviderQuote!;
  const provider = _threeUsdIcpRegistry.get(hopQuote.provider);
  const result = await provider.swap(threeUsdToken, to, threeUsdOut, minOutput, hopQuote);
  return result.amountOut;
}

case 'icp_to_stable': {
  if (isOisyWallet()) return executeIcpToStableOisy(route, from, to, amountIn, slippageBps);

  // Non-Oisy sequential: best-provider swap on ICP -> 3USD, then 3pool redeem
  const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
  const hopQuote = route.hopProviderQuote!;
  const provider = _threeUsdIcpRegistry.get(hopQuote.provider);
  const threeUsdMinOut = route.intermediateOutput! * BigInt(10000 - slippageBps) / 10000n;
  const swapResult = await provider.swap(from, threeUsdToken, amountIn, threeUsdMinOut, hopQuote);
  const stableOut = await threePoolService.removeOneCoin(
    swapResult.amountOut, to.threePoolIndex, minOutput,
  );
  return stableOut;
}
```

The Oisy branches call the existing `executeStableToIcpOisy` / `executeIcpToStableOisy` helpers; Task 10 extends them to handle the ICPswap provider.

- [ ] **Step 6: Verify all existing tests still pass**

```bash
cd src/vault_frontend && npm run test
npm run check
```

Expected: zero new failures. Existing routing tests (if any) still pass.

- [ ] **Step 7: Commit**

```bash
git add src/vault_frontend/src/lib/services/swapRouter.ts
git commit -m "refactor(swap): route 3USD/ICP hop through provider registry"
```

---

## Task 10: Extend Oisy batched flow for ICPswap's multi-step swap

**Files:**
- Modify: `src/vault_frontend/src/lib/services/swapRouter.ts`

The existing Oisy batched flow assumes a single-call swap on Rumi AMM. For ICPswap's 3-step flow (depositFrom -> swap -> withdraw), we need more batched operations. We handle this by branching inside the Oisy functions based on `hopProviderQuote.provider`.

- [ ] **Step 1: Add ICPswap branch to `executeStableToIcpOisy`**

Inside `executeStableToIcpOisy`, after computing the 3USD amounts and before calling `ammActor.swap`, add:

```typescript
if (route.hopProviderQuote!.provider === 'icpswap_3usd_icp') {
  // ICPswap branch: approve 3USD to ICPswap pool instead of Rumi AMM,
  // and batch depositFrom / swap / withdraw.
  const icpswapPoolId = route.hopProviderQuote!.meta.poolCanisterId as string;
  const zeroForOne = route.hopProviderQuote!.meta.zeroForOne as boolean;

  const icpswapPool = createOisyActor(
    icpswapPoolId, canisterIDLs.icpswap_pool, signerAgent,
  );

  // (The approvals for the stablecoin -> 3pool step remain as in the Rumi AMM
  // branch; only the 3USD hop changes.)

  // Re-approve 3USD -> ICPswap pool instead of Rumi AMM
  signerAgent.batch();
  const pIcpApprove = threeUsdLedger.icrc2_approve({
    amount: threeUsdEstimate * 101n / 100n,
    spender: { owner: Principal.fromText(icpswapPoolId), subaccount: [] },
    expires_at: [], expected_allowance: [], memo: [], fee: [],
    from_subaccount: [], created_at_time: [],
  });

  // Step 3 (unchanged): 3pool deposit
  signerAgent.batch();
  const pDeposit3Pool = threeUsdLedger.add_liquidity(amounts, threeUsdMinOutput);

  // Step 4a: ICPswap depositFrom
  signerAgent.batch();
  const pIcpDepositFrom = icpswapPool.depositFrom({
    token: CANISTER_IDS.THREEPOOL,
    amount: threeUsdEstimate.toString(),
    fee: 0,
  });

  // Step 4b: ICPswap swap
  signerAgent.batch();
  const pIcpSwap = icpswapPool.swap({
    amountIn: threeUsdEstimate.toString(),
    zeroForOne,
    amountOutMinimum: icpMinOutput.toString(),
  });

  // Step 4c: ICPswap withdraw
  signerAgent.batch();
  const pIcpWithdraw = icpswapPool.withdraw({
    token: CANISTER_IDS.ICP_LEDGER,
    amount: icpMinOutput.toString(), // conservative; actual amount comes from swap
    fee: 0,
  });

  await signerAgent.execute();
  const [r1, r2Approve, r3, r4a, r4b, r4c] = await Promise.all([
    p1, pIcpApprove, pDeposit3Pool, pIcpDepositFrom, pIcpSwap, pIcpWithdraw,
  ]);

  // Error checks analogous to the existing Rumi AMM branch ...
  if (r1 && 'Err' in r1) throw new Error(`Stablecoin approval failed: ${JSON.stringify(r1.Err)}`);
  if (r2Approve && 'Err' in r2Approve) throw new Error(`3USD approval failed: ${JSON.stringify(r2Approve.Err)}`);
  if ('Err' in r3) throw new Error(`3pool deposit failed: ${JSON.stringify(r3.Err)}`);
  if ('err' in r4a) throw new Error(`ICPswap depositFrom failed: ${JSON.stringify(r4a.err)}`);
  if ('err' in r4b) throw new Error(`ICPswap swap failed: ${JSON.stringify(r4b.err)}`);
  if ('err' in r4c) throw new Error(`ICPswap withdraw failed: ${JSON.stringify(r4c.err)}`);
  return BigInt((r4c as { ok: string }).ok);
}
```

Note: the `canisterIDLs.icpswap_pool` needs to be registered in `src/vault_frontend/src/lib/services/pnp.ts` - see next step.

- [ ] **Step 2: Register ICPswap pool IDL in pnp.ts**

Modify `src/vault_frontend/src/lib/services/pnp.ts`:

- Add the import at the top:

```typescript
import { idlFactory as icpswapPoolIDL } from '$declarations/icpswap_pool/icpswap_pool.did.js';
```

- Add to the `canisterIDLs` export:

```typescript
export const canisterIDLs = {
  // ... existing entries
  icpswap_pool: icpswapPoolIDL,
};
```

- Register the pool canister for ICRC-21 consent if the rest of the file does that pattern for other canisters. Use the same pattern that `rumi_amm` uses.

- [ ] **Step 3: Add ICPswap branch to `executeIcpToStableOisy`**

Inside `executeIcpToStableOisy`, after setting up actors and before the existing Rumi AMM branch, add the ICPswap branch:

```typescript
if (route.hopProviderQuote!.provider === 'icpswap_3usd_icp') {
  const icpswapPoolId = route.hopProviderQuote!.meta.poolCanisterId as string;
  const zeroForOne = route.hopProviderQuote!.meta.zeroForOne as boolean;
  const threeUsdEstimate = route.intermediateOutput!;
  const stableMinOutput = route.estimatedOutput * BigInt(10000 - slippageBps) / 10000n;
  const threeUsdMinFromSwap = threeUsdEstimate * BigInt(10000 - slippageBps) / 10000n;

  const icpswapPool = createOisyActor(
    icpswapPoolId, canisterIDLs.icpswap_pool, signerAgent,
  );

  // Step 1: approve ICP -> ICPswap pool (replaces Rumi AMM approval)
  signerAgent.batch();
  const pIcpApprove = icpLedger.icrc2_approve({
    amount: amountIn * 101n / 100n,
    spender: { owner: Principal.fromText(icpswapPoolId), subaccount: [] },
    expires_at: [], expected_allowance: [], memo: [], fee: [],
    from_subaccount: [], created_at_time: [],
  });

  // Step 2: ICPswap depositFrom (ICP)
  signerAgent.batch();
  const pIcpDepositFrom = icpswapPool.depositFrom({
    token: CANISTER_IDS.ICP_LEDGER,
    amount: amountIn.toString(),
    fee: 0,
  });

  // Step 3: ICPswap swap (ICP -> 3USD)
  signerAgent.batch();
  const pIcpSwap = icpswapPool.swap({
    amountIn: amountIn.toString(),
    zeroForOne,
    amountOutMinimum: threeUsdMinFromSwap.toString(),
  });

  // Step 4: ICPswap withdraw (3USD to caller)
  signerAgent.batch();
  const pIcpWithdraw = icpswapPool.withdraw({
    token: CANISTER_IDS.THREEPOOL,
    amount: threeUsdMinFromSwap.toString(),
    fee: 0,
  });

  // Step 5: approve 3USD -> 3pool for remove_one_coin
  signerAgent.batch();
  const pThreeUsdApprove = threeUsdLedger.icrc2_approve({
    amount: threeUsdEstimate * 101n / 100n,
    spender: { owner: Principal.fromText(CANISTER_IDS.THREEPOOL), subaccount: [] },
    expires_at: [], expected_allowance: [], memo: [], fee: [],
    from_subaccount: [], created_at_time: [],
  });

  // Step 6: 3pool remove_one_coin (3USD -> target stable)
  signerAgent.batch();
  const pRedeem = threePoolActor.remove_liquidity_one_coin(
    threeUsdEstimate, to.threePoolIndex, stableMinOutput,
  );

  await signerAgent.execute();
  const [r1, r2, r3, r4, r5, r6] = await Promise.all([
    pIcpApprove, pIcpDepositFrom, pIcpSwap, pIcpWithdraw, pThreeUsdApprove, pRedeem,
  ]);

  if ('Err' in r1) throw new Error(`ICP approval failed: ${JSON.stringify(r1.Err)}`);
  if ('err' in r2) throw new Error(`ICPswap depositFrom failed: ${JSON.stringify(r2.err)}`);
  if ('err' in r3) throw new Error(`ICPswap swap failed: ${JSON.stringify(r3.err)}`);
  if ('err' in r4) throw new Error(`ICPswap withdraw failed: ${JSON.stringify(r4.err)}`);
  if ('Err' in r5) throw new Error(`3USD approval failed: ${JSON.stringify(r5.Err)}`);
  if ('Err' in r6) throw new Error(`3pool redeem failed: ${JSON.stringify(r6.Err)}`);

  return BigInt((r6 as { Ok: string }).Ok);
}
```

- [ ] **Step 4: Manually test both paths on mainnet with small amounts**

Skip to Task 13 for the full verification matrix. For now, just verify the code compiles and unit tests pass.

```bash
cd src/vault_frontend && npm run check
npm run test
```

Expected: zero new type errors, all unit tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/services/swapRouter.ts src/vault_frontend/src/lib/services/pnp.ts
git commit -m "feat(swap): batch ICPswap 3-step flow for Oisy wallets"
```

---

## Task 11: Add direct icUSD <-> ICP route via ICPswap icUSD/ICP pool

**Files:**
- Modify: `src/vault_frontend/src/lib/services/swapRouter.ts`

Give users with icUSD the option to route directly through ICPswap's icUSD/ICP pool for small swaps (better price when 3USD/ICP depth doesn't matter). The router compares this against the 2-hop path and picks the best.

- [ ] **Step 1: Add a second provider registry for icUSD/ICP**

Near the existing `_threeUsdIcpRegistry` in `swapRouter.ts`, add:

```typescript
const _icUsdIcpRegistry = new ProviderRegistry([
  new IcpswapProvider({
    id: 'icpswap_icusd_icp',
    poolCanisterId: CANISTER_IDS.ICPSWAP_ICUSD_ICP_POOL,
    // Verified via Task 1: token0 = ICP, token1 = icUSD in this pool.
    token0LedgerId: CANISTER_IDS.ICP_LEDGER,
    token1LedgerId: CANISTER_IDS.ICUSD_LEDGER,
    feeBps: 30,
  }),
]);
```

- [ ] **Step 2: Add new route types**

Extend `RouteType`:

```typescript
export type RouteType =
  | 'three_pool_swap'
  | 'three_pool_deposit'
  | 'three_pool_redeem'
  | 'amm_swap'
  | 'stable_to_icp'
  | 'icp_to_stable'
  | 'icusd_icp_direct';  // NEW: icUSD <-> ICP direct via ICPswap
```

- [ ] **Step 3: Insert new Case 5a in `resolveRoute`**

Before Case 5 (Stablecoin -> ICP), add:

```typescript
// Case 5a: icUSD -> ICP (or ICP -> icUSD), compare direct vs 2-hop
const isIcUsd = (t: AmmToken) => t.symbol === 'icUSD';
if ((isIcUsd(from) && isICP(to)) || (isICP(from) && isIcUsd(to))) {
  // Option A: direct via ICPswap icUSD/ICP
  let directQuote: ProviderQuote | null = null;
  try {
    directQuote = await _icUsdIcpRegistry.bestQuote(from, to, amountIn);
  } catch {
    // Pool may be too thin or offline; continue with 2-hop only.
  }

  // Option B: 2-hop via 3pool + best-of(3USD/ICP)
  const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
  let twoHopOutput: bigint;
  let twoHopIntermediate: bigint;
  let twoHopQuote: ProviderQuote;

  if (isIcUsd(from)) {
    const amounts = [0n, 0n, 0n];
    amounts[from.threePoolIndex] = amountIn;
    twoHopIntermediate = await threePoolService.calcAddLiquidity(amounts);
    twoHopQuote = await _threeUsdIcpRegistry.bestQuote(threeUsdToken, to, twoHopIntermediate);
    twoHopOutput = twoHopQuote.amountOut;
  } else {
    twoHopQuote = await _threeUsdIcpRegistry.bestQuote(from, threeUsdToken, amountIn);
    twoHopIntermediate = twoHopQuote.amountOut;
    twoHopOutput = await threePoolService.calcRemoveOneCoin(twoHopIntermediate, to.threePoolIndex);
  }

  // Pick the better option
  if (directQuote && directQuote.amountOut >= twoHopOutput) {
    return {
      type: 'icusd_icp_direct',
      pathDisplay: directQuote.label,
      hops: 1,
      estimatedOutput: directQuote.amountOut,
      feeDisplay: directQuote.feeDisplay,
      providerQuote: directQuote,
    };
  }

  // Fall back to 2-hop (the existing stable_to_icp / icp_to_stable shape)
  return {
    type: isIcUsd(from) ? 'stable_to_icp' : 'icp_to_stable',
    pathDisplay: isIcUsd(from)
      ? `icUSD -> 3USD -> ICP (${twoHopQuote.provider})`
      : `ICP -> 3USD -> icUSD (${twoHopQuote.provider})`,
    hops: 2,
    estimatedOutput: twoHopOutput,
    feeDisplay: twoHopQuote.feeDisplay,
    intermediateOutput: twoHopIntermediate,
    hopProviderQuote: twoHopQuote,
  };
}
```

Place this block BEFORE the general Case 5 (stable -> ICP) so icUSD gets special-cased.

- [ ] **Step 4: Handle `icusd_icp_direct` in `executeRoute`**

Add a new case in the switch:

```typescript
case 'icusd_icp_direct': {
  const q = route.providerQuote!;
  const provider = _icUsdIcpRegistry.get(q.provider);

  if (isOisyWallet()) {
    // Batch: approve icUSD -> ICPswap pool, then depositFrom/swap/withdraw.
    return await executeIcpswapDirectOisy(route, from, to, amountIn, slippageBps, _icUsdIcpRegistry);
  }

  // Non-Oisy: pre-approve icUSD, then call provider.swap
  // (Approval code similar to the existing AMM flow, but targeted at the
  // ICPswap pool canister from q.meta.poolCanisterId.)
  const result = await provider.swap(from, to, amountIn, minOutput, q);
  return result.amountOut;
}
```

Implement `executeIcpswapDirectOisy` as a helper in the same file:

```typescript
async function executeIcpswapDirectOisy(
  route: SwapRoute,
  from: AmmToken,
  to: AmmToken,
  amountIn: bigint,
  slippageBps: number,
): Promise<bigint> {
  const q = route.providerQuote!;
  const icpswapPoolId = q.meta.poolCanisterId as string;
  const zeroForOne = q.meta.zeroForOne as boolean;
  const minOut = q.amountOut * BigInt(10000 - slippageBps) / 10000n;

  const signerAgent = getSignerAgent();
  const fromLedger = createOisyActor(from.ledgerId, canisterIDLs.icrc2_ledger, signerAgent);
  const icpswapPool = createOisyActor(icpswapPoolId, canisterIDLs.icpswap_pool, signerAgent);

  // Step 1: approve input token -> ICPswap pool
  signerAgent.batch();
  const pApprove = fromLedger.icrc2_approve({
    amount: amountIn * 101n / 100n,
    spender: { owner: Principal.fromText(icpswapPoolId), subaccount: [] },
    expires_at: [], expected_allowance: [], memo: [], fee: [],
    from_subaccount: [], created_at_time: [],
  });

  // Step 2: depositFrom
  signerAgent.batch();
  const pDeposit = icpswapPool.depositFrom({
    token: from.ledgerId,
    amount: amountIn.toString(),
    fee: 0,
  });

  // Step 3: swap
  signerAgent.batch();
  const pSwap = icpswapPool.swap({
    amountIn: amountIn.toString(),
    zeroForOne,
    amountOutMinimum: minOut.toString(),
  });

  // Step 4: withdraw to caller
  signerAgent.batch();
  const pWithdraw = icpswapPool.withdraw({
    token: to.ledgerId,
    amount: minOut.toString(),
    fee: 0,
  });

  await signerAgent.execute();
  const [r1, r2, r3, r4] = await Promise.all([pApprove, pDeposit, pSwap, pWithdraw]);

  if ('Err' in r1) throw new Error(`${from.symbol} approval failed: ${JSON.stringify(r1.Err)}`);
  if ('err' in r2) throw new Error(`ICPswap depositFrom failed: ${JSON.stringify(r2.err)}`);
  if ('err' in r3) throw new Error(`ICPswap swap failed: ${JSON.stringify(r3.err)}`);
  if ('err' in r4) throw new Error(`ICPswap withdraw failed: ${JSON.stringify(r4.err)}`);

  return BigInt((r4 as { ok: string }).ok);
}
```

- [ ] **Step 5: Verify**

```bash
cd src/vault_frontend && npm run test && npm run check
```

Expected: all tests pass, no type errors.

- [ ] **Step 6: Commit**

```bash
git add src/vault_frontend/src/lib/services/swapRouter.ts
git commit -m "feat(swap): add direct icUSD/ICP route via ICPswap, compared against 2-hop"
```

---

## Task 12: Show the selected provider in the SwapInterface UI

**Files:**
- Modify: `src/vault_frontend/src/lib/components/swap/SwapInterface.svelte`

- [ ] **Step 1: Locate the route-preview block**

Find the section where `pathDisplay` is already shown (search for `route.pathDisplay`). It's the block that renders the route summary above the Swap button.

- [ ] **Step 2: Add a provider pill next to the path**

Replace the existing `pathDisplay` rendering with something like:

```svelte
{#if route}
  <div class="flex items-center gap-2 text-sm text-zinc-400">
    <span>{route.pathDisplay}</span>
    {#if route.providerQuote}
      <span class="rounded-full bg-zinc-800 px-2 py-0.5 text-xs text-zinc-300">
        {providerLabel(route.providerQuote.provider)}
      </span>
    {:else if route.hopProviderQuote}
      <span class="rounded-full bg-zinc-800 px-2 py-0.5 text-xs text-zinc-300">
        via {providerLabel(route.hopProviderQuote.provider)}
      </span>
    {/if}
  </div>
{/if}
```

Add the helper near the top of the `<script>` block:

```typescript
function providerLabel(id: string): string {
  switch (id) {
    case 'rumi_amm': return 'Rumi AMM';
    case 'icpswap_3usd_icp': return 'ICPswap 3USD/ICP';
    case 'icpswap_icusd_icp': return 'ICPswap icUSD/ICP';
    default: return id;
  }
}
```

- [ ] **Step 3: Verify visually on a local dev server**

Rob verifies changes directly on mainnet (per CLAUDE.md). Type-check only:

```bash
cd src/vault_frontend && npm run check
```

Expected: zero type errors.

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/lib/components/swap/SwapInterface.svelte
git commit -m "feat(swap-ui): show selected provider in route preview"
```

---

## Task 13: Mainnet verification of every route type

This is not a code task; it's a verification matrix run on mainnet with small amounts after deploying the frontend. Rob executes this personally.

- [ ] **Step 1: Deploy the frontend to mainnet**

```bash
dfx deploy vault_frontend --network ic
```

- [ ] **Step 2: Test matrix**

For each pair below, connect a test wallet, request a quote, and verify the amountOut is reasonable against app.icpswap.com and the Rumi AMM directly. Then execute with small amounts (~1-10 USD equivalent) and verify the chosen provider matches the UI pill.

| From | To | Expected best provider (given current depth) |
|---|---|---|
| 3USD | ICP | ICPswap 3USD/ICP (if migrated) or Rumi AMM |
| ICP | 3USD | same as above |
| icUSD | ICP | ICPswap icUSD/ICP direct for small swaps; 3USD/ICP 2-hop for large |
| ICP | icUSD | same, inverse |
| ckUSDT | ICP | Always 2-hop (3pool + best-of 3USD/ICP) |
| ICP | ckUSDT | same |
| ckUSDC | icUSD | 3pool only, unchanged |
| icUSD | ckUSDT | 3pool only, unchanged |
| icUSD | 3USD | 3pool deposit, unchanged |
| 3USD | ckUSDC | 3pool redeem, unchanged |

- [ ] **Step 3: Oisy-specific verification**

Repeat the ICPswap-involving rows above with an Oisy wallet. Verify:
- One signer popup fires per swap (not multiple).
- All batched operations either succeed together or fail cleanly.
- No orphaned deposits in the ICPswap pool canister (if a swap fails after deposit, the withdraw step should run in the same batch).

- [ ] **Step 4: Document results**

Append a "Verification results" section to `docs/superpowers/plans/2026-04-15-icpswap-routing-research.md` with the measured outputs for each route type, comparing our quoted output to a direct app.icpswap.com swap.

- [ ] **Step 5: Commit verification notes**

```bash
git add docs/superpowers/plans/2026-04-15-icpswap-routing-research.md
git commit -m "docs: mainnet verification results for ICPswap routing"
```

---

## Task 14: Move this plan to the project docs directory

**Files:**
- Copy: `/Users/robertripley/.claude/plans/iterative-crafting-barto.md` -> `docs/superpowers/plans/2026-04-15-icpswap-routing-integration.md`

This can only happen after exiting plan mode (plan mode constrains writes).

- [ ] **Step 1: Copy the plan file**

```bash
cp /Users/robertripley/.claude/plans/iterative-crafting-barto.md \
   docs/superpowers/plans/2026-04-15-icpswap-routing-integration.md
```

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/plans/2026-04-15-icpswap-routing-integration.md
git commit -m "docs: add ICPswap routing integration plan"
```

---

## Verification Summary

After all tasks complete, the system should:

1. Quote both Rumi AMM and ICPswap 3USD/ICP for every 3USD <-> ICP hop and pick the better one per swap.
2. Quote ICPswap icUSD/ICP directly for icUSD <-> ICP swaps and pick it when it beats the 2-hop path.
3. Show the user which provider was chosen ("via Rumi AMM" / "via ICPswap 3USD/ICP") in the route preview.
4. Work for both Internet Identity and Oisy wallets (single-popup batched execution on Oisy).
5. Leave the 3pool, the Rumi AMM canister, and all non-ICP-involving routes unchanged.
6. Allow Rob to migrate liquidity between Rumi AMM and ICPswap 3USD/ICP freely (more capital on ICPswap naturally wins more routes via better quotes).

Unit test coverage target:
- RumiAmmProvider: 3 tests (support, quote shape, pool ID caching)
- IcpswapProvider: 5 tests (id, support, quote direction, full swap, slippage check)
- ProviderRegistry: 4 tests (quote fan-out, best selection, error resilience, no-supporting-provider error)

Total code impact (excluding tests):
- 4 new files, ~600 lines
- 3 modified files, ~200 line-changes
- 0 canister changes
