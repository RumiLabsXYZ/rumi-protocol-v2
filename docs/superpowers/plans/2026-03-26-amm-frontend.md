# AMM Frontend Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add AMM swap + liquidity UI to the existing Rumi frontend, supporting 5 tokens (icUSD, ckUSDC, ckUSDT, 3USD, ICP) with automatic routing through 3pool and/or the AMM canister.

**Architecture:** The swap page gets a `[Swap] [Liquidity]` toggle. The swap interface is extended with ICP and 3USD tokens, plus a routing layer that determines the cheapest path (direct 3pool swap, direct AMM swap, or two-hop via 3pool+AMM). The liquidity view shows a pool list (3pool + 3USD/ICP AMM) with add/remove interfaces. Frontend routing only — no router canister.

**Tech Stack:** SvelteKit, TypeScript, @dfinity/agent, @dfinity/principal, Candid IDL, ICRC-2 approvals, Oisy ICRC-112 batching

---

## File Structure

### New files
| File | Responsibility |
|------|---------------|
| `src/declarations/rumi_amm/rumi_amm.did.js` | Candid IDL factory for the AMM canister |
| `src/declarations/rumi_amm/rumi_amm.did.d.ts` | TypeScript types for AMM IDL |
| `src/declarations/rumi_amm/index.js` | Re-export of IDL + canister ID |
| `src/declarations/rumi_amm/index.d.ts` | Type re-export |
| `src/vault_frontend/src/lib/services/ammService.ts` | AMM canister query/mutation service (mirrors threePoolService pattern) |
| `src/vault_frontend/src/lib/services/swapRouter.ts` | Route resolver: given token pair, returns path + quote |
| `src/vault_frontend/src/lib/components/swap/SwapLiquidityToggle.svelte` | `[Swap] [Liquidity]` toggle buttons at top of swap card |
| `src/vault_frontend/src/lib/components/swap/PoolListView.svelte` | Pool list with TVL/APY cards, links to add liquidity |
| `src/vault_frontend/src/lib/components/swap/AmmLiquidityPanel.svelte` | Add/remove liquidity for AMM pools (3USD/ICP) |

### Modified files
| File | Changes |
|------|---------|
| `src/vault_frontend/src/lib/config.ts` | Add `RUMI_AMM` canister ID to `CANISTER_IDS` |
| `src/vault_frontend/src/lib/services/pnp.ts` | Add AMM IDL import, AMM canister to delegation targets |
| `src/vault_frontend/src/lib/components/swap/SwapInterface.svelte` | Replace 3pool-only token list with unified 5-token list, integrate swapRouter, add price impact warnings (2% yellow, 5% red) |
| `src/vault_frontend/src/routes/swap/+page.svelte` | Add SwapLiquidityToggle, conditionally render SwapInterface or PoolListView |
| `src/vault_frontend/src/lib/stores/wallet.ts` | Add AMM LP balance fetching in refreshBalance |

---

## Decisions (locked in by Rob)

- **Slippage:** User-configurable, 0.5% default (already exists)
- **Tokens:** icUSD, ckUSDC, ckUSDT, 3USD, ICP
- **Stablecoin-to-stablecoin "swaps":** Route through 3pool (deposit/redeem), presented as swaps
- **3USD-to-stablecoin:** 3pool redeem
- **Stablecoin-to-3USD:** 3pool deposit
- **3USD-to-ICP / ICP-to-3USD:** Direct AMM swap
- **Stablecoin-to-ICP:** 3pool deposit (mint 3USD) then AMM swap
- **ICP-to-stablecoin:** AMM swap (get 3USD) then 3pool redeem
- **Remove liquidity:** Percentage slider (25/50/75/100%)
- **Price impact warnings:** Yellow at 2%, red "are you sure" at 5%
- **AMM canister:** Separate canister (already built as `rumi_amm`)
- **LP accounting:** Internal (not ICRC-1 tokens)

## Routing Table

| From | To | Route | Hops |
|------|----|-------|------|
| Stablecoin | Stablecoin | 3pool swap | 1 |
| Stablecoin | 3USD | 3pool deposit | 1 |
| 3USD | Stablecoin | 3pool redeem | 1 |
| 3USD | ICP | AMM swap | 1 |
| ICP | 3USD | AMM swap | 1 |
| Stablecoin | ICP | 3pool deposit + AMM swap | 2 |
| ICP | Stablecoin | AMM swap + 3pool redeem | 2 |

---

### Task 1: AMM Candid Declarations

Generate the JavaScript IDL files for the `rumi_amm` canister so the frontend can create actors.

**Files:**
- Create: `src/declarations/rumi_amm/rumi_amm.did.js`
- Create: `src/declarations/rumi_amm/rumi_amm.did.d.ts`
- Create: `src/declarations/rumi_amm/index.js`
- Create: `src/declarations/rumi_amm/index.d.ts`
- Reference: `src/rumi_amm/rumi_amm.did` (the Candid source)
- Reference: `src/declarations/rumi_3pool/` (pattern to follow)

- [ ] **Step 1: Generate rumi_amm.did.js**

Create the IDL factory matching the Candid interface in `src/rumi_amm/rumi_amm.did`. The AMM Candid types are:

```
AmmInitArgs = record { admin : principal }
CurveType = variant { ConstantProduct }
CreatePoolArgs = record { token_a : principal; token_b : principal; fee_bps : nat16; curve : CurveType }
PoolInfo = record { pool_id : text; token_a : principal; token_b : principal; reserve_a : nat; reserve_b : nat; fee_bps : nat16; protocol_fee_bps : nat16; curve : CurveType; total_lp_shares : nat; paused : bool }
SwapResult = record { amount_out : nat; fee : nat }
AmmError = variant { PoolNotFound; PoolAlreadyExists; PoolPaused; ZeroAmount; InsufficientOutput : record { expected_min : nat; actual : nat }; InsufficientLiquidity; InsufficientLpShares : record { required : nat; available : nat }; InvalidToken; TransferFailed : record { token : text; reason : text }; Unauthorized; MathOverflow; DisproportionateLiquidity }
```

Service methods:
```
health : () -> (text) query
swap : (text, principal, nat, nat) -> (variant { Ok : SwapResult; Err : AmmError })
add_liquidity : (text, nat, nat, nat) -> (variant { Ok : nat; Err : AmmError })
remove_liquidity : (text, nat, nat, nat) -> (variant { Ok : record { nat; nat }; Err : AmmError })
get_pool : (text) -> (opt PoolInfo) query
get_pools : () -> (vec PoolInfo) query
get_quote : (text, principal, nat) -> (variant { Ok : nat; Err : AmmError }) query
get_lp_balance : (text, principal) -> (nat) query
create_pool : (CreatePoolArgs) -> (variant { Ok : text; Err : AmmError })
set_fee : (text, nat16) -> (variant { Ok; Err : AmmError })
set_protocol_fee : (text, nat16) -> (variant { Ok; Err : AmmError })
withdraw_protocol_fees : (text) -> (variant { Ok : record { nat; nat }; Err : AmmError })
pause_pool : (text) -> (variant { Ok; Err : AmmError })
unpause_pool : (text) -> (variant { Ok; Err : AmmError })
```

Write the `rumi_amm.did.js` following the exact pattern of `src/declarations/rumi_3pool/rumi_3pool.did.js`:

```javascript
export const idlFactory = ({ IDL }) => {
  const AmmInitArgs = IDL.Record({ 'admin': IDL.Principal });
  const CurveType = IDL.Variant({ 'ConstantProduct': IDL.Null });
  const CreatePoolArgs = IDL.Record({
    'token_a': IDL.Principal,
    'token_b': IDL.Principal,
    'fee_bps': IDL.Nat16,
    'curve': CurveType,
  });
  const SwapResult = IDL.Record({ 'amount_out': IDL.Nat, 'fee': IDL.Nat });
  const AmmError = IDL.Variant({
    'PoolNotFound': IDL.Null,
    'PoolAlreadyExists': IDL.Null,
    'PoolPaused': IDL.Null,
    'ZeroAmount': IDL.Null,
    'InsufficientOutput': IDL.Record({ 'expected_min': IDL.Nat, 'actual': IDL.Nat }),
    'InsufficientLiquidity': IDL.Null,
    'InsufficientLpShares': IDL.Record({ 'required': IDL.Nat, 'available': IDL.Nat }),
    'InvalidToken': IDL.Null,
    'TransferFailed': IDL.Record({ 'token': IDL.Text, 'reason': IDL.Text }),
    'Unauthorized': IDL.Null,
    'MathOverflow': IDL.Null,
    'DisproportionateLiquidity': IDL.Null,
  });
  const PoolInfo = IDL.Record({
    'pool_id': IDL.Text,
    'token_a': IDL.Principal,
    'token_b': IDL.Principal,
    'reserve_a': IDL.Nat,
    'reserve_b': IDL.Nat,
    'fee_bps': IDL.Nat16,
    'protocol_fee_bps': IDL.Nat16,
    'curve': CurveType,
    'total_lp_shares': IDL.Nat,
    'paused': IDL.Bool,
  });
  return IDL.Service({
    'health': IDL.Func([], [IDL.Text], ['query']),
    'swap': IDL.Func([IDL.Text, IDL.Principal, IDL.Nat, IDL.Nat], [IDL.Variant({ 'Ok': SwapResult, 'Err': AmmError })], []),
    'add_liquidity': IDL.Func([IDL.Text, IDL.Nat, IDL.Nat, IDL.Nat], [IDL.Variant({ 'Ok': IDL.Nat, 'Err': AmmError })], []),
    'remove_liquidity': IDL.Func([IDL.Text, IDL.Nat, IDL.Nat, IDL.Nat], [IDL.Variant({ 'Ok': IDL.Tuple(IDL.Nat, IDL.Nat), 'Err': AmmError })], []),
    'get_pool': IDL.Func([IDL.Text], [IDL.Opt(PoolInfo)], ['query']),
    'get_pools': IDL.Func([], [IDL.Vec(PoolInfo)], ['query']),
    'get_quote': IDL.Func([IDL.Text, IDL.Principal, IDL.Nat], [IDL.Variant({ 'Ok': IDL.Nat, 'Err': AmmError })], ['query']),
    'get_lp_balance': IDL.Func([IDL.Text, IDL.Principal], [IDL.Nat], ['query']),
    'create_pool': IDL.Func([CreatePoolArgs], [IDL.Variant({ 'Ok': IDL.Text, 'Err': AmmError })], []),
    'set_fee': IDL.Func([IDL.Text, IDL.Nat16], [IDL.Variant({ 'Ok': IDL.Null, 'Err': AmmError })], []),
    'set_protocol_fee': IDL.Func([IDL.Text, IDL.Nat16], [IDL.Variant({ 'Ok': IDL.Null, 'Err': AmmError })], []),
    'withdraw_protocol_fees': IDL.Func([IDL.Text], [IDL.Variant({ 'Ok': IDL.Tuple(IDL.Nat, IDL.Nat), 'Err': AmmError })], []),
    'pause_pool': IDL.Func([IDL.Text], [IDL.Variant({ 'Ok': IDL.Null, 'Err': AmmError })], []),
    'unpause_pool': IDL.Func([IDL.Text], [IDL.Variant({ 'Ok': IDL.Null, 'Err': AmmError })], []),
  });
};
export const init = ({ IDL }) => {
  const AmmInitArgs = IDL.Record({ 'admin': IDL.Principal });
  return [AmmInitArgs];
};
```

- [ ] **Step 2: Create index.js and TypeScript declaration files**

`index.js`:
```javascript
export { idlFactory } from './rumi_amm.did.js';
```

`index.d.ts`:
```typescript
export { idlFactory } from './rumi_amm.did.js';
```

`rumi_amm.did.d.ts`:
```typescript
import type { IDL } from '@dfinity/candid';
export const idlFactory: IDL.InterfaceFactory;
export const init: IDL.InterfaceFactory;
```

- [ ] **Step 3: Verify the declarations import correctly**

Run: `cd /Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend && npx tsc --noEmit 2>&1 | head -20`

If there are import errors related to `rumi_amm`, fix them. Minor pre-existing errors in other files are fine.

- [ ] **Step 4: Commit**

```bash
git add src/declarations/rumi_amm/
git commit -m "feat(frontend): add AMM canister Candid declarations"
```

---

### Task 2: Config + PNP Integration

Register the AMM canister ID and IDL so wallet delegation includes it.

**Files:**
- Modify: `src/vault_frontend/src/lib/config.ts`
- Modify: `src/vault_frontend/src/lib/services/pnp.ts`

- [ ] **Step 1: Add AMM canister ID to config.ts**

In `src/vault_frontend/src/lib/config.ts`, add to `CANISTER_IDS`:

```typescript
// Rumi AMM (3USD/ICP constant-product pool)
RUMI_AMM: "PLACEHOLDER_CANISTER_ID",
```

**IMPORTANT:** The AMM canister hasn't been deployed yet. Use a placeholder string. When deploying, replace it with the real canister ID.

Also add a getter in `CONFIG`:

```typescript
get ammCanisterId() {
  return CANISTER_IDS.RUMI_AMM;
},
```

And add to the IDL exports at the bottom alongside the existing ones:

```typescript
import { idlFactory as rumiAmmIDL } from '$declarations/rumi_amm/rumi_amm.did.js';
```

Add `rumiAmmIDL` to the CONFIG object:

```typescript
rumiAmmIDL,
```

- [ ] **Step 2: Add AMM to PNP delegation targets and IDLs**

In `src/vault_frontend/src/lib/services/pnp.ts`:

1. Add the import at the top:
```typescript
import { idlFactory as rumiAmmIDL } from '$declarations/rumi_amm/rumi_amm.did.js';
```

2. Add to `canisterIDLs`:
```typescript
rumi_amm: rumiAmmIDL,
```

3. Add to `CanisterType`:
```typescript
| "rumi_amm"
```

4. Add `CANISTER_IDS.RUMI_AMM` to `getAllDelegationTargets()`:
```typescript
CANISTER_IDS.RUMI_AMM,         // AMM pool
```

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/config.ts src/vault_frontend/src/lib/services/pnp.ts
git commit -m "feat(frontend): register AMM canister in config and wallet delegation"
```

---

### Task 3: AMM Service

Create the service layer for interacting with the AMM canister. Follows the exact pattern of `threePoolService.ts`.

**Files:**
- Create: `src/vault_frontend/src/lib/services/ammService.ts`
- Reference: `src/vault_frontend/src/lib/services/threePoolService.ts` (pattern to follow exactly)

- [ ] **Step 1: Create ammService.ts**

```typescript
import { Principal } from '@dfinity/principal';
import { Actor, HttpAgent, AnonymousIdentity } from '@dfinity/agent';
import { canisterIDLs } from './pnp';
import { walletStore } from '../stores/wallet';
import { get } from 'svelte/store';
import { CANISTER_IDS, CONFIG } from '../config';
import { isOisyWallet } from './protocol/walletOperations';
import { getOisySignerAgent, createOisyActor } from './oisySigner';

// ──────────────────────────────────────────────────────────────
// Types — mirrors the AMM Candid interface
// ──────────────────────────────────────────────────────────────

export interface PoolInfo {
  pool_id: string;
  token_a: Principal;
  token_b: Principal;
  reserve_a: bigint;
  reserve_b: bigint;
  fee_bps: number;
  protocol_fee_bps: number;
  curve: { ConstantProduct: null };
  total_lp_shares: bigint;
  paused: boolean;
}

export interface SwapResult {
  amount_out: bigint;
  fee: bigint;
}

export interface AmmError {
  [key: string]: any;
}

// ──────────────────────────────────────────────────────────────
// Token metadata for AMM-tradeable tokens
// ──────────────────────────────────────────────────────────────

export interface AmmToken {
  symbol: string;
  ledgerId: string;
  decimals: number;
  color: string;
  /** Wallet store key for balance lookup */
  balanceKey: string;
  /** Whether this is the 3pool LP token (special handling for deposits/redeems) */
  is3USD: boolean;
  /** 3pool index, if this is a stablecoin in the 3pool (-1 if not) */
  threePoolIndex: number;
}

export const AMM_TOKENS: AmmToken[] = [
  {
    symbol: 'ICP',
    ledgerId: CANISTER_IDS.ICP_LEDGER,
    decimals: 8,
    color: '#29abe2',
    balanceKey: 'ICP',
    is3USD: false,
    threePoolIndex: -1,
  },
  {
    symbol: '3USD',
    ledgerId: CANISTER_IDS.THREEPOOL,
    decimals: 8,
    color: '#34d399',
    balanceKey: 'THREEUSD',
    is3USD: true,
    threePoolIndex: -1,
  },
  {
    symbol: 'icUSD',
    ledgerId: CANISTER_IDS.ICUSD_LEDGER,
    decimals: 8,
    color: '#818cf8',
    balanceKey: 'ICUSD',
    is3USD: false,
    threePoolIndex: 0,
  },
  {
    symbol: 'ckUSDT',
    ledgerId: CANISTER_IDS.CKUSDT_LEDGER,
    decimals: 6,
    color: '#26A17B',
    balanceKey: 'CKUSDT',
    is3USD: false,
    threePoolIndex: 1,
  },
  {
    symbol: 'ckUSDC',
    ledgerId: CANISTER_IDS.CKUSDC_LEDGER,
    decimals: 6,
    color: '#2775CA',
    balanceKey: 'CKUSDC',
    is3USD: false,
    threePoolIndex: 2,
  },
];

/** Ledger transfer fee per token. */
export function getLedgerFee(token: AmmToken): bigint {
  // ICP: 10_000 e8s = 0.0001 ICP
  // icUSD / 3USD (8 decimals): 100_000 e8s = 0.001
  // ckUSDT / ckUSDC (6 decimals): 10_000
  if (token.symbol === 'ICP') return 10_000n;
  return token.decimals === 8 ? 100_000n : 10_000n;
}

/** Compute approval amount: transfer amount + ledger fee. */
export function approvalAmount(amount: bigint, token: AmmToken): bigint {
  return amount + getLedgerFee(token);
}

export function parseTokenAmount(amount: string, decimals: number): bigint {
  const value = parseFloat(amount);
  if (isNaN(value) || value < 0) throw new Error('Invalid amount');
  return BigInt(Math.floor(value * Math.pow(10, decimals)));
}

export function formatTokenAmount(amount: bigint, decimals: number): string {
  const divisor = Math.pow(10, decimals);
  const value = Number(amount) / divisor;
  if (value > 0 && value < 0.01) return value.toFixed(Math.min(decimals, 6));
  const fixed = value.toFixed(4);
  let trimmed = fixed.replace(/0+$/, '');
  if (trimmed.endsWith('.')) trimmed += '00';
  else if (trimmed.indexOf('.') !== -1 && trimmed.split('.')[1].length < 2) {
    trimmed += '0';
  }
  return trimmed;
}

// ──────────────────────────────────────────────────────────────
// Service
// ──────────────────────────────────────────────────────────────

const AMM_CANISTER_ID = CANISTER_IDS.RUMI_AMM;

class AmmService {
  private _anonAgent: HttpAgent | null = null;

  private async getQueryActor(): Promise<any> {
    if (!this._anonAgent) {
      this._anonAgent = new HttpAgent({
        host: CONFIG.host,
        identity: new AnonymousIdentity(),
      });
      if (CONFIG.isLocal) {
        await this._anonAgent.fetchRootKey();
      }
    }
    return Actor.createActor(canisterIDLs.rumi_amm as any, {
      agent: this._anonAgent,
      canisterId: AMM_CANISTER_ID,
    });
  }

  // ── Queries (anonymous) ──

  async getPool(poolId: string): Promise<PoolInfo | null> {
    const actor = await this.getQueryActor();
    const result = await actor.get_pool(poolId);
    return result.length > 0 ? result[0] : null;
  }

  async getPools(): Promise<PoolInfo[]> {
    const actor = await this.getQueryActor();
    return await actor.get_pools();
  }

  async getQuote(poolId: string, tokenIn: Principal, amountIn: bigint): Promise<bigint> {
    const actor = await this.getQueryActor();
    const result = await actor.get_quote(poolId, tokenIn, amountIn) as { Ok: bigint } | { Err: any };
    if ('Err' in result) throw new Error(this.formatError(result.Err));
    return result.Ok;
  }

  async getLpBalance(poolId: string, owner: Principal): Promise<bigint> {
    const actor = await this.getQueryActor();
    return await actor.get_lp_balance(poolId, owner);
  }

  // ── Mutations ──

  /**
   * Execute an AMM swap.
   * @param poolId Pool identifier string
   * @param tokenIn Ledger principal of the input token
   * @param amountIn Raw amount of input token
   * @param minAmountOut Minimum output (slippage-protected)
   */
  async swap(
    poolId: string,
    tokenIn: Principal,
    amountIn: bigint,
    minAmountOut: bigint,
    inputToken: AmmToken
  ): Promise<SwapResult> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const oisyDetected = isOisyWallet();

    if (oisyDetected && wallet.principal) {
      const signerAgent = await getOisySignerAgent(wallet.principal);

      const ledgerActor = createOisyActor(
        inputToken.ledgerId, CONFIG.icusd_ledgerIDL, signerAgent
      );
      const ammActor = createOisyActor(
        AMM_CANISTER_ID, canisterIDLs.rumi_amm, signerAgent
      );

      signerAgent.batch();
      const approvePromise = ledgerActor.icrc2_approve({
        amount: approvalAmount(amountIn, inputToken),
        spender: { owner: Principal.fromText(AMM_CANISTER_ID), subaccount: [] },
        expires_at: [], expected_allowance: [], memo: [], fee: [],
        from_subaccount: [], created_at_time: [],
      });

      signerAgent.batch();
      const swapPromise = ammActor.swap(poolId, tokenIn, amountIn, minAmountOut);

      await signerAgent.execute();
      const [approveResult, swapResult] = await Promise.all([approvePromise, swapPromise]);

      if (approveResult && 'Err' in approveResult) {
        throw new Error(`Approval failed: ${JSON.stringify(approveResult.Err)}`);
      }
      if ('Err' in swapResult) throw new Error(this.formatError(swapResult.Err));
      return swapResult.Ok;
    } else {
      const ledgerActor = await walletStore.getActor(
        inputToken.ledgerId, CONFIG.icusd_ledgerIDL
      ) as any;

      const approveResult = await ledgerActor.icrc2_approve({
        amount: approvalAmount(amountIn, inputToken),
        spender: { owner: Principal.fromText(AMM_CANISTER_ID), subaccount: [] },
        expires_at: [], expected_allowance: [], memo: [], fee: [],
        from_subaccount: [], created_at_time: [],
      });

      if (approveResult && 'Err' in approveResult) {
        throw new Error(`Approval failed: ${JSON.stringify(approveResult.Err)}`);
      }

      await new Promise(r => setTimeout(r, 2000));

      const ammActor = await walletStore.getActor(
        AMM_CANISTER_ID, canisterIDLs.rumi_amm
      ) as any;
      const result = await ammActor.swap(poolId, tokenIn, amountIn, minAmountOut);
      if ('Err' in result) throw new Error(this.formatError(result.Err));
      return result.Ok;
    }
  }

  /**
   * Add liquidity to an AMM pool.
   * Requires ICRC-2 approval for both tokens.
   */
  async addLiquidity(
    poolId: string,
    amountA: bigint,
    amountB: bigint,
    minLpShares: bigint,
    tokenA: AmmToken,
    tokenB: AmmToken
  ): Promise<bigint> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const oisyDetected = isOisyWallet();

    if (oisyDetected && wallet.principal) {
      const signerAgent = await getOisySignerAgent(wallet.principal);
      const approvePromises: Promise<any>[] = [];

      // Approve token A
      if (amountA > 0n) {
        const ledgerA = createOisyActor(tokenA.ledgerId, CONFIG.icusd_ledgerIDL, signerAgent);
        signerAgent.batch();
        approvePromises.push(ledgerA.icrc2_approve({
          amount: approvalAmount(amountA, tokenA),
          spender: { owner: Principal.fromText(AMM_CANISTER_ID), subaccount: [] },
          expires_at: [], expected_allowance: [], memo: [], fee: [],
          from_subaccount: [], created_at_time: [],
        }));
      }

      // Approve token B
      if (amountB > 0n) {
        const ledgerB = createOisyActor(tokenB.ledgerId, CONFIG.icusd_ledgerIDL, signerAgent);
        signerAgent.batch();
        approvePromises.push(ledgerB.icrc2_approve({
          amount: approvalAmount(amountB, tokenB),
          spender: { owner: Principal.fromText(AMM_CANISTER_ID), subaccount: [] },
          expires_at: [], expected_allowance: [], memo: [], fee: [],
          from_subaccount: [], created_at_time: [],
        }));
      }

      // Add liquidity call
      const ammActor = createOisyActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm, signerAgent);
      signerAgent.batch();
      const addPromise = ammActor.add_liquidity(poolId, amountA, amountB, minLpShares);

      await signerAgent.execute();
      const results = await Promise.all([...approvePromises, addPromise]);

      for (let i = 0; i < approvePromises.length; i++) {
        const r = results[i];
        if (r && 'Err' in r) throw new Error(`Approval failed: ${JSON.stringify(r.Err)}`);
      }

      const addResult = results[results.length - 1];
      if ('Err' in addResult) throw new Error(this.formatError(addResult.Err));
      return addResult.Ok;
    } else {
      const spender = { owner: Principal.fromText(AMM_CANISTER_ID), subaccount: [] };

      // Approve token A
      if (amountA > 0n) {
        const ledgerA = await walletStore.getActor(tokenA.ledgerId, CONFIG.icusd_ledgerIDL) as any;
        const r = await ledgerA.icrc2_approve({
          amount: approvalAmount(amountA, tokenA), spender,
          expires_at: [], expected_allowance: [], memo: [], fee: [],
          from_subaccount: [], created_at_time: [],
        });
        if (r && 'Err' in r) throw new Error(`Approval failed for ${tokenA.symbol}: ${JSON.stringify(r.Err)}`);
        await new Promise(r => setTimeout(r, 2000));
      }

      // Approve token B
      if (amountB > 0n) {
        const ledgerB = await walletStore.getActor(tokenB.ledgerId, CONFIG.icusd_ledgerIDL) as any;
        const r = await ledgerB.icrc2_approve({
          amount: approvalAmount(amountB, tokenB), spender,
          expires_at: [], expected_allowance: [], memo: [], fee: [],
          from_subaccount: [], created_at_time: [],
        });
        if (r && 'Err' in r) throw new Error(`Approval failed for ${tokenB.symbol}: ${JSON.stringify(r.Err)}`);
        await new Promise(r => setTimeout(r, 2000));
      }

      const ammActor = await walletStore.getActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm) as any;
      const result = await ammActor.add_liquidity(poolId, amountA, amountB, minLpShares);
      if ('Err' in result) throw new Error(this.formatError(result.Err));
      return result.Ok;
    }
  }

  /**
   * Remove liquidity from an AMM pool.
   * No approvals needed — LP shares are internal, canister just checks caller's balance.
   */
  async removeLiquidity(
    poolId: string,
    lpShares: bigint,
    minAmountA: bigint,
    minAmountB: bigint
  ): Promise<{ amountA: bigint; amountB: bigint }> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const ammActor = await walletStore.getActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm) as any;
    const result = await ammActor.remove_liquidity(poolId, lpShares, minAmountA, minAmountB);
    if ('Err' in result) throw new Error(this.formatError(result.Err));
    const [amountA, amountB] = result.Ok;
    return { amountA, amountB };
  }

  // ── Error formatting ──

  private formatError(err: any): string {
    if ('InsufficientOutput' in err) {
      return `Insufficient output: expected at least ${err.InsufficientOutput.expected_min}, got ${err.InsufficientOutput.actual}`;
    }
    if ('InsufficientLiquidity' in err) return 'Insufficient liquidity in the pool';
    if ('InsufficientLpShares' in err) return `Insufficient LP shares: need ${err.InsufficientLpShares.required}, have ${err.InsufficientLpShares.available}`;
    if ('PoolNotFound' in err) return 'Pool not found';
    if ('PoolAlreadyExists' in err) return 'Pool already exists';
    if ('PoolPaused' in err) return 'Pool is paused';
    if ('ZeroAmount' in err) return 'Amount must be greater than zero';
    if ('InvalidToken' in err) return 'Invalid token';
    if ('TransferFailed' in err) return `Transfer failed (${err.TransferFailed.token}): ${err.TransferFailed.reason}`;
    if ('Unauthorized' in err) return 'Unauthorized';
    if ('MathOverflow' in err) return 'Math overflow';
    if ('DisproportionateLiquidity' in err) return 'Amounts must be proportional to pool reserves';
    return 'Unknown AMM error';
  }
}

export const ammService = new AmmService();
```

- [ ] **Step 2: Commit**

```bash
git add src/vault_frontend/src/lib/services/ammService.ts
git commit -m "feat(frontend): add AMM canister service layer"
```

---

### Task 4: Swap Router

The routing brain. Given a `fromToken` and `toToken`, determines the path and fetches a combined quote.

**Files:**
- Create: `src/vault_frontend/src/lib/services/swapRouter.ts`
- Reference: `src/vault_frontend/src/lib/services/threePoolService.ts` (for 3pool quote methods)
- Reference: `src/vault_frontend/src/lib/services/ammService.ts` (for AMM quote methods)

- [ ] **Step 1: Create swapRouter.ts**

```typescript
import { Principal } from '@dfinity/principal';
import { threePoolService, POOL_TOKENS, parseTokenAmount, getLedgerFee as get3PoolLedgerFee } from './threePoolService';
import { ammService, type AmmToken, AMM_TOKENS, getLedgerFee as getAmmLedgerFee } from './ammService';
import { CANISTER_IDS } from '../config';

// ──────────────────────────────────────────────────────────────
// Route types
// ──────────────────────────────────────────────────────────────

export type RouteType =
  | 'three_pool_swap'       // Stablecoin <-> Stablecoin (direct 3pool)
  | 'three_pool_deposit'    // Stablecoin -> 3USD (mint via 3pool)
  | 'three_pool_redeem'     // 3USD -> Stablecoin (redeem via 3pool)
  | 'amm_swap'              // 3USD <-> ICP (direct AMM)
  | 'stable_to_icp'         // Stablecoin -> ICP (3pool deposit + AMM swap)
  | 'icp_to_stable';        // ICP -> Stablecoin (AMM swap + 3pool redeem)

export interface SwapRoute {
  type: RouteType;
  /** Human-readable path, e.g. "ckUSDC -> 3USD -> ICP" */
  pathDisplay: string;
  /** Number of on-chain hops */
  hops: number;
  /** Estimated output in raw units of the output token */
  estimatedOutput: bigint;
  /** Combined fee display (percentage) */
  feeDisplay: string;
}

// The 3USD/ICP pool ID — derived from the two token ledger principals
// This will be set after the pool is created. For now, we query dynamically.
let _cachedPoolId: string | null = null;

async function getAmmPoolId(): Promise<string> {
  if (_cachedPoolId) return _cachedPoolId;
  const pools = await ammService.getPools();
  // Find the 3USD/ICP pool
  const threeUsdPrincipal = CANISTER_IDS.THREEPOOL;
  const icpPrincipal = CANISTER_IDS.ICP_LEDGER;
  const pool = pools.find(p => {
    const a = p.token_a.toText();
    const b = p.token_b.toText();
    return (a === threeUsdPrincipal && b === icpPrincipal) ||
           (a === icpPrincipal && b === threeUsdPrincipal);
  });
  if (!pool) throw new Error('3USD/ICP AMM pool not found');
  _cachedPoolId = pool.pool_id;
  return _cachedPoolId;
}

/** Reset cached pool ID (e.g. when pools change) */
export function clearPoolIdCache() {
  _cachedPoolId = null;
}

// ──────────────────────────────────────────────────────────────
// Route resolver
// ──────────────────────────────────────────────────────────────

function isStablecoin(token: AmmToken): boolean {
  return token.threePoolIndex >= 0;
}

function is3USD(token: AmmToken): boolean {
  return token.is3USD;
}

function isICP(token: AmmToken): boolean {
  return token.symbol === 'ICP';
}

/**
 * Determine the swap route and fetch a combined quote.
 */
export async function resolveRoute(
  from: AmmToken,
  to: AmmToken,
  amountIn: bigint,
): Promise<SwapRoute> {

  // ─── Case 1: Stablecoin <-> Stablecoin (3pool swap) ───
  if (isStablecoin(from) && isStablecoin(to)) {
    const output = await threePoolService.calcSwap(from.threePoolIndex, to.threePoolIndex, amountIn);
    return {
      type: 'three_pool_swap',
      pathDisplay: `${from.symbol} -> ${to.symbol}`,
      hops: 1,
      estimatedOutput: output,
      feeDisplay: '0.20%', // 3pool swap fee
    };
  }

  // ─── Case 2: Stablecoin -> 3USD (3pool deposit) ───
  if (isStablecoin(from) && is3USD(to)) {
    const amounts = [0n, 0n, 0n];
    amounts[from.threePoolIndex] = amountIn;
    const output = await threePoolService.calcAddLiquidity(amounts);
    return {
      type: 'three_pool_deposit',
      pathDisplay: `${from.symbol} -> 3USD`,
      hops: 1,
      estimatedOutput: output,
      feeDisplay: '~0%', // Deposit has minimal slippage, no explicit fee
    };
  }

  // ─── Case 3: 3USD -> Stablecoin (3pool redeem) ───
  if (is3USD(from) && isStablecoin(to)) {
    const output = await threePoolService.calcRemoveOneCoin(amountIn, to.threePoolIndex);
    return {
      type: 'three_pool_redeem',
      pathDisplay: `3USD -> ${to.symbol}`,
      hops: 1,
      estimatedOutput: output,
      feeDisplay: '~0%',
    };
  }

  // ─── Case 4: 3USD <-> ICP (direct AMM swap) ───
  if ((is3USD(from) && isICP(to)) || (isICP(from) && is3USD(to))) {
    const poolId = await getAmmPoolId();
    const tokenIn = Principal.fromText(from.ledgerId);
    const output = await ammService.getQuote(poolId, tokenIn, amountIn);
    return {
      type: 'amm_swap',
      pathDisplay: `${from.symbol} -> ${to.symbol}`,
      hops: 1,
      estimatedOutput: output,
      feeDisplay: '0.30%', // AMM 30bps fee
    };
  }

  // ─── Case 5: Stablecoin -> ICP (two-hop: deposit + AMM swap) ───
  if (isStablecoin(from) && isICP(to)) {
    // Step 1: How much 3USD from depositing the stablecoin
    const amounts = [0n, 0n, 0n];
    amounts[from.threePoolIndex] = amountIn;
    const threeUsdOut = await threePoolService.calcAddLiquidity(amounts);

    // Step 2: How much ICP from swapping that 3USD
    const poolId = await getAmmPoolId();
    const threeUsdPrincipal = Principal.fromText(CANISTER_IDS.THREEPOOL);
    const icpOut = await ammService.getQuote(poolId, threeUsdPrincipal, threeUsdOut);

    return {
      type: 'stable_to_icp',
      pathDisplay: `${from.symbol} -> 3USD -> ICP`,
      hops: 2,
      estimatedOutput: icpOut,
      feeDisplay: '~0.30%',
    };
  }

  // ─── Case 6: ICP -> Stablecoin (two-hop: AMM swap + redeem) ───
  if (isICP(from) && isStablecoin(to)) {
    // Step 1: How much 3USD from swapping ICP
    const poolId = await getAmmPoolId();
    const icpPrincipal = Principal.fromText(CANISTER_IDS.ICP_LEDGER);
    const threeUsdOut = await ammService.getQuote(poolId, icpPrincipal, amountIn);

    // Step 2: How much stablecoin from redeeming that 3USD
    const stableOut = await threePoolService.calcRemoveOneCoin(threeUsdOut, to.threePoolIndex);

    return {
      type: 'icp_to_stable',
      pathDisplay: `ICP -> 3USD -> ${to.symbol}`,
      hops: 2,
      estimatedOutput: stableOut,
      feeDisplay: '~0.30%',
    };
  }

  throw new Error(`No route found for ${from.symbol} -> ${to.symbol}`);
}

// ──────────────────────────────────────────────────────────────
// Route execution
// ──────────────────────────────────────────────────────────────

/**
 * Execute a resolved route. This is called after the user confirms the swap.
 *
 * For single-hop routes, it delegates to the appropriate service.
 * For two-hop routes, it executes sequentially (hop 1 then hop 2).
 *
 * @returns The final output amount received
 */
export async function executeRoute(
  route: SwapRoute,
  from: AmmToken,
  to: AmmToken,
  amountIn: bigint,
  slippageBps: number,
): Promise<bigint> {
  const minOutput = route.estimatedOutput * BigInt(10000 - slippageBps) / 10000n;

  switch (route.type) {
    case 'three_pool_swap': {
      return await threePoolService.swap(
        from.threePoolIndex, to.threePoolIndex, amountIn, minOutput
      );
    }

    case 'three_pool_deposit': {
      const amounts = [0n, 0n, 0n];
      amounts[from.threePoolIndex] = amountIn;
      return await threePoolService.addLiquidity(amounts, minOutput);
    }

    case 'three_pool_redeem': {
      return await threePoolService.removeOneCoin(amountIn, to.threePoolIndex, minOutput);
    }

    case 'amm_swap': {
      const poolId = await getAmmPoolId();
      const tokenIn = Principal.fromText(from.ledgerId);
      const result = await ammService.swap(poolId, tokenIn, amountIn, minOutput, from);
      return result.amount_out;
    }

    case 'stable_to_icp': {
      // Hop 1: Deposit stablecoin -> 3USD
      const amounts = [0n, 0n, 0n];
      amounts[from.threePoolIndex] = amountIn;
      // Split slippage budget across two hops
      const threeUsdEstimate = await threePoolService.calcAddLiquidity(amounts);
      const threeUsdMinOutput = threeUsdEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      const threeUsdReceived = await threePoolService.addLiquidity(amounts, threeUsdMinOutput);

      // Hop 2: Swap 3USD -> ICP
      const poolId = await getAmmPoolId();
      const threeUsdPrincipal = Principal.fromText(CANISTER_IDS.THREEPOOL);
      // Use remaining slippage budget for hop 2
      const icpEstimate = await ammService.getQuote(poolId, threeUsdPrincipal, threeUsdReceived);
      const icpMinOutput = icpEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
      const result = await ammService.swap(poolId, threeUsdPrincipal, threeUsdReceived, icpMinOutput, threeUsdToken);
      return result.amount_out;
    }

    case 'icp_to_stable': {
      // Hop 1: Swap ICP -> 3USD
      const poolId = await getAmmPoolId();
      const icpPrincipal = Principal.fromText(CANISTER_IDS.ICP_LEDGER);
      const threeUsdEstimate = await ammService.getQuote(poolId, icpPrincipal, amountIn);
      const threeUsdMinOutput = threeUsdEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      const icpToken = AMM_TOKENS.find(t => t.symbol === 'ICP')!;
      const hop1 = await ammService.swap(poolId, icpPrincipal, amountIn, threeUsdMinOutput, icpToken);

      // Hop 2: Redeem 3USD -> Stablecoin
      const stableEstimate = await threePoolService.calcRemoveOneCoin(hop1.amount_out, to.threePoolIndex);
      const stableMinOutput = stableEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      return await threePoolService.removeOneCoin(hop1.amount_out, to.threePoolIndex, stableMinOutput);
    }

    default:
      throw new Error(`Unknown route type: ${route.type}`);
  }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/vault_frontend/src/lib/services/swapRouter.ts
git commit -m "feat(frontend): add swap router with 6 route types and multi-hop execution"
```

---

### Task 5: Swap/Liquidity Toggle Component

Simple stateless toggle at the top of the swap card.

**Files:**
- Create: `src/vault_frontend/src/lib/components/swap/SwapLiquidityToggle.svelte`

- [ ] **Step 1: Create SwapLiquidityToggle.svelte**

```svelte
<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  export let mode: 'swap' | 'liquidity' = 'swap';
  const dispatch = createEventDispatcher();

  function setMode(m: 'swap' | 'liquidity') {
    mode = m;
    dispatch('change', { mode: m });
  }
</script>

<div class="toggle-bar">
  <button
    class="toggle-btn"
    class:active={mode === 'swap'}
    on:click={() => setMode('swap')}
  >
    Swap
  </button>
  <button
    class="toggle-btn"
    class:active={mode === 'liquidity'}
    on:click={() => setMode('liquidity')}
  >
    Liquidity
  </button>
</div>

<style>
  .toggle-bar {
    display: flex;
    gap: 0.25rem;
    padding: 0.1875rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    margin-bottom: 1.25rem;
  }

  .toggle-btn {
    flex: 1;
    padding: 0.5rem 0;
    border: none;
    border-radius: 0.375rem;
    font-family: 'Inter', sans-serif;
    font-size: 0.8125rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    background: transparent;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .toggle-btn:hover:not(.active) {
    color: var(--rumi-text-secondary);
  }

  .toggle-btn.active {
    background: var(--rumi-bg-surface1);
    color: var(--rumi-text-primary);
    font-weight: 600;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.15);
  }
</style>
```

- [ ] **Step 2: Commit**

```bash
git add src/vault_frontend/src/lib/components/swap/SwapLiquidityToggle.svelte
git commit -m "feat(frontend): add swap/liquidity toggle component"
```

---

### Task 6: Update SwapInterface for Unified Token List + Router

This is the biggest task. Replace the 3-token-only swap interface with the full 5-token version that uses the swap router.

**Files:**
- Modify: `src/vault_frontend/src/lib/components/swap/SwapInterface.svelte`

- [ ] **Step 1: Rewrite SwapInterface.svelte script section**

Replace the entire `<script>` block. Key changes:
- Import from `ammService` and `swapRouter` instead of only `threePoolService`
- Use `AMM_TOKENS` (5 tokens) instead of `POOL_TOKENS` (3 tokens)
- Use `resolveRoute()` for quotes instead of direct `calcSwap()`
- Use `executeRoute()` for execution instead of direct `threePoolService.swap()`
- Add price impact warning thresholds: 2% yellow, 5% red
- Show route path (e.g. "ckUSDC -> 3USD -> ICP") in the info section

The full updated `<script>` block:

```svelte
<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { walletStore } from '../../stores/wallet';
  import { AMM_TOKENS, parseTokenAmount, formatTokenAmount, getLedgerFee, type AmmToken } from '../../services/ammService';
  import { resolveRoute, executeRoute, type SwapRoute } from '../../services/swapRouter';
  import { formatStableTokenDisplay } from '../../utils/format';

  const dispatch = createEventDispatcher();

  let fromIdx = 0; // Index into AMM_TOKENS
  let toIdx = 1;
  let amount = '';
  let loading = false;
  let quoting = false;
  let error = '';
  let currentRoute: SwapRoute | null = null;
  let slippageBps = 50;
  let showSlippage = false;
  let showFromDropdown = false;
  let showToDropdown = false;

  let quoteTimer: ReturnType<typeof setTimeout> | null = null;

  $: isConnected = $walletStore.isConnected;
  $: fromToken = AMM_TOKENS[fromIdx];
  $: toToken = AMM_TOKENS[toIdx];

  // Wallet balance for "from" token
  $: walletBalance = (() => {
    if (!$walletStore.tokenBalances) return 0n;
    const key = fromToken.balanceKey;
    if (!key) return 0n;
    return $walletStore.tokenBalances[key]?.raw ?? 0n;
  })();

  $: walletBalanceFormatted = formatTokenAmount(walletBalance, fromToken.decimals);

  // Formatted output
  $: outputFormatted = currentRoute
    ? formatTokenAmount(currentRoute.estimatedOutput, toToken.decimals)
    : '';

  // Effective rate
  $: effectiveRate = (() => {
    if (!currentRoute || !amount || parseFloat(amount) <= 0) return null;
    const inputValue = parseFloat(amount);
    const outputValue = Number(currentRoute.estimatedOutput) / Math.pow(10, toToken.decimals);
    return (outputValue / inputValue).toFixed(6);
  })();

  // Price impact (approximate)
  $: priceImpact = (() => {
    if (!currentRoute || !amount || parseFloat(amount) <= 0) return null;
    const inputValue = parseFloat(amount);
    const outputValue = Number(currentRoute.estimatedOutput) / Math.pow(10, toToken.decimals);
    // For stablecoin-stablecoin, fair rate is ~1:1
    // For ICP pairs, we can't easily determine "fair" rate without more data
    // So we use the fee-adjusted rate approach
    const feeBps = parseFloat(currentRoute.feeDisplay.replace(/[~%]/g, '')) * 100;
    const feeRate = feeBps / 10000;
    const rateAfterFeeRemoval = (outputValue / inputValue) / (1 - feeRate);
    const impact = (1 - rateAfterFeeRemoval) * 100;
    if (Math.abs(impact) < 0.005) return '0.00';
    return impact.toFixed(2);
  })();

  // Price impact warning levels
  $: impactLevel = (() => {
    if (priceImpact === null) return 'none';
    const val = Math.abs(parseFloat(priceImpact));
    if (val >= 5) return 'danger';
    if (val >= 2) return 'warn';
    return 'none';
  })();

  // Debounced quote
  $: if (amount && parseFloat(amount) > 0) {
    currentRoute = null;
    debouncedQuote();
  } else {
    currentRoute = null;
  }

  function debouncedQuote() {
    if (quoteTimer) clearTimeout(quoteTimer);
    quoteTimer = setTimeout(fetchQuote, 400);
  }

  async function fetchQuote() {
    const val = parseFloat(amount);
    if (!val || val <= 0) { currentRoute = null; return; }
    try {
      quoting = true;
      const amountRaw = parseTokenAmount(amount, fromToken.decimals);
      currentRoute = await resolveRoute(fromToken, toToken, amountRaw);
    } catch (err: any) {
      currentRoute = null;
      console.warn('Quote failed:', err.message);
    } finally {
      quoting = false;
    }
  }

  function flipTokens() {
    const tmp = fromIdx;
    fromIdx = toIdx;
    toIdx = tmp;
    amount = '';
    currentRoute = null;
    error = '';
  }

  function selectFrom(index: number) {
    if (index === toIdx) toIdx = fromIdx;
    fromIdx = index;
    showFromDropdown = false;
    amount = '';
    currentRoute = null;
    error = '';
  }

  function selectTo(index: number) {
    if (index === fromIdx) fromIdx = toIdx;
    toIdx = index;
    showToDropdown = false;
    currentRoute = null;
    error = '';
  }

  function setMax() {
    const totalFees = getLedgerFee(fromToken) * 2n;
    const adjusted = walletBalance > totalFees ? walletBalance - totalFees : 0n;
    const divisor = Math.pow(10, fromToken.decimals);
    amount = (Number(adjusted) / divisor).toFixed(fromToken.decimals);
  }

  function setSlippage(bps: number) {
    slippageBps = bps;
  }

  async function handleSwap() {
    if (!amount || parseFloat(amount) <= 0) {
      error = 'Enter a valid amount';
      return;
    }
    if (!currentRoute) {
      error = 'Waiting for quote';
      return;
    }

    // Red warning gate
    if (impactLevel === 'danger') {
      const confirmed = confirm(`Price impact is ${priceImpact}%. Are you sure you want to proceed?`);
      if (!confirmed) return;
    }

    try {
      loading = true;
      error = '';
      const amountRaw = parseTokenAmount(amount, fromToken.decimals);

      const totalFees = getLedgerFee(fromToken) * 2n;
      if (amountRaw + totalFees > walletBalance) {
        error = 'Insufficient balance (amount + fees)';
        return;
      }

      await executeRoute(currentRoute, fromToken, toToken, amountRaw, slippageBps);
      dispatch('success', { action: 'swap' });
      amount = '';
      currentRoute = null;
    } catch (err: any) {
      error = err.message || 'Swap failed';
    } finally {
      loading = false;
    }
  }

  function closeDropdowns() {
    showFromDropdown = false;
    showToDropdown = false;
  }
</script>
```

- [ ] **Step 2: Update the template**

Key template changes (in the `{:else}` branch after the connect gate):
1. Token dropdowns iterate over `AMM_TOKENS` instead of `POOL_TOKENS`
2. Add route path display in the info rows section
3. Add price impact warning styling with the new `impactLevel`
4. Update the swap button text to use `fromToken.symbol` and `toToken.symbol`

In the info-rows section, update the price impact row:
```svelte
{#if priceImpact !== null}
  <div class="info-row">
    <span class="info-label">Price impact</span>
    <span class="info-value"
      class:impact-warn={impactLevel === 'warn'}
      class:impact-danger={impactLevel === 'danger'}
      class:impact-favorable={parseFloat(priceImpact) < 0}>
      {priceImpact}%
    </span>
  </div>
{/if}
```

Add route display row:
```svelte
{#if currentRoute && currentRoute.hops > 1}
  <div class="info-row">
    <span class="info-label">Route</span>
    <span class="info-value route-path">{currentRoute.pathDisplay}</span>
  </div>
{/if}
```

Update the swap fee row:
```svelte
<div class="info-row">
  <span class="info-label">Swap fee</span>
  <span class="info-value">{currentRoute?.feeDisplay ?? '—'}</span>
</div>
```

- [ ] **Step 3: Update styles**

Add to the `<style>` block:
```css
.info-value.impact-warn {
  color: #fbbf24; /* amber/yellow */
}

.info-value.impact-danger {
  color: var(--rumi-danger);
  font-weight: 600;
}

.route-path {
  font-family: 'SF Mono', 'Fira Code', monospace;
  letter-spacing: -0.01em;
}
```

- [ ] **Step 4: Verify the component compiles**

Run: `cd /Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend && npm run build 2>&1 | tail -20`

Fix any TypeScript or Svelte compilation errors.

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/components/swap/SwapInterface.svelte
git commit -m "feat(frontend): unified 5-token swap with router + price impact warnings"
```

---

### Task 7: Pool List View

Shows available pools with TVL, fee info, and links to add liquidity.

**Files:**
- Create: `src/vault_frontend/src/lib/components/swap/PoolListView.svelte`

- [ ] **Step 1: Create PoolListView.svelte**

This component:
- Fetches pool data from both 3pool and AMM on mount
- Shows two cards: "3pool (icUSD/ckUSDT/ckUSDC)" and "3USD/ICP"
- Each card shows: pair, TVL, fee, user's LP position if connected
- Clicking a card opens the respective add-liquidity interface

```svelte
<script lang="ts">
  import { createEventDispatcher, onMount } from 'svelte';
  import { walletStore } from '../../stores/wallet';
  import { threePoolService, POOL_TOKENS, formatTokenAmount } from '../../services/threePoolService';
  import { ammService, type PoolInfo } from '../../services/ammService';
  import { CANISTER_IDS } from '../../config';
  import type { PoolStatus } from '../../services/threePoolService';

  const dispatch = createEventDispatcher();

  let threePoolStatus: PoolStatus | null = null;
  let ammPool: PoolInfo | null = null;
  let userThreePoolLp = 0n;
  let userAmmLp = 0n;
  let loading = true;

  $: isConnected = $walletStore.isConnected;

  onMount(loadData);

  async function loadData() {
    loading = true;
    try {
      const [tpStatus, ammPools] = await Promise.all([
        threePoolService.getPoolStatus(),
        ammService.getPools(),
      ]);
      threePoolStatus = tpStatus;
      // Find the 3USD/ICP pool specifically
      const threePoolId = CANISTER_IDS.THREEPOOL;
      const icpLedgerId = CANISTER_IDS.ICP_LEDGER;
      ammPool = ammPools.find(p => {
        const a = p.token_a.toText();
        const b = p.token_b.toText();
        return (a === threePoolId && b === icpLedgerId) || (a === icpLedgerId && b === threePoolId);
      }) ?? null;

      if (isConnected && $walletStore.principal) {
        const promises: Promise<any>[] = [
          threePoolService.getLpBalance($walletStore.principal),
        ];
        if (ammPool) {
          promises.push(ammService.getLpBalance(ammPool.pool_id, $walletStore.principal));
        }
        const [tpLp, ammLpResult] = await Promise.all(promises);
        userThreePoolLp = tpLp;
        userAmmLp = ammLpResult ?? 0n;
      }
    } catch (e) {
      console.error('Failed to load pool data:', e);
    } finally {
      loading = false;
    }
  }

  function threePoolTvl(): string {
    if (!threePoolStatus) return '$0.00';
    let total = 0;
    for (let i = 0; i < 3; i++) {
      const bal = Number(threePoolStatus.balances[i]);
      total += bal / Math.pow(10, POOL_TOKENS[i].decimals);
    }
    return '$' + total.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  }

  function ammTvl(): string {
    if (!ammPool) return '$0.00';
    // Determine which reserve is 3USD by checking token principals
    const threePoolId = CANISTER_IDS.THREEPOOL;
    const isTokenA3USD = ammPool.token_a.toText() === threePoolId;
    const threeUsdReserve = isTokenA3USD ? ammPool.reserve_a : ammPool.reserve_b;
    // 3USD ~= $1, and for a balanced pool TVL ~= 2x the stablecoin side
    const threeUsdValue = Number(threeUsdReserve) / 1e8;
    return '~$' + (threeUsdValue * 2).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  }

  function selectPool(pool: 'threepool' | 'amm') {
    dispatch('select', { pool });
  }
</script>

{#if loading}
  <div class="loading">Loading pools...</div>
{:else}
  <div class="pool-list">
    <!-- 3pool card -->
    <button class="pool-card" on:click={() => selectPool('threepool')}>
      <div class="pool-pair">
        <div class="pool-dots">
          {#each POOL_TOKENS as t}
            <span class="pool-dot" style="background:{t.color}"></span>
          {/each}
        </div>
        <span class="pool-name">3pool</span>
        <span class="pool-tokens">icUSD / ckUSDT / ckUSDC</span>
      </div>
      <div class="pool-stats">
        <div class="pool-stat">
          <span class="stat-label">TVL</span>
          <span class="stat-value">{threePoolTvl()}</span>
        </div>
        <div class="pool-stat">
          <span class="stat-label">Fee</span>
          <span class="stat-value">{threePoolStatus ? (Number(threePoolStatus.swap_fee_bps) / 100).toFixed(2) + '%' : '—'}</span>
        </div>
        {#if isConnected && userThreePoolLp > 0n}
          <div class="pool-stat">
            <span class="stat-label">Your LP</span>
            <span class="stat-value lp-value">{formatTokenAmount(userThreePoolLp, 8)}</span>
          </div>
        {/if}
      </div>
      <div class="pool-action">Add Liquidity →</div>
    </button>

    <!-- AMM pool card -->
    {#if ammPool}
      <button class="pool-card" on:click={() => selectPool('amm')}>
        <div class="pool-pair">
          <div class="pool-dots">
            <span class="pool-dot" style="background:#34d399"></span>
            <span class="pool-dot" style="background:#29abe2"></span>
          </div>
          <span class="pool-name">3USD / ICP</span>
        </div>
        <div class="pool-stats">
          <div class="pool-stat">
            <span class="stat-label">TVL</span>
            <span class="stat-value">{ammTvl()}</span>
          </div>
          <div class="pool-stat">
            <span class="stat-label">Fee</span>
            <span class="stat-value">{(ammPool.fee_bps / 100).toFixed(2)}%</span>
          </div>
          {#if isConnected && userAmmLp > 0n}
            <div class="pool-stat">
              <span class="stat-label">Your LP</span>
              <span class="stat-value lp-value">{formatTokenAmount(userAmmLp, 8)}</span>
            </div>
          {/if}
        </div>
        <div class="pool-action">Add Liquidity →</div>
      </button>
    {:else}
      <div class="pool-card pool-card-empty">
        <span class="pool-name">3USD / ICP</span>
        <span class="pool-empty-text">Pool not yet created</span>
      </div>
    {/if}
  </div>
{/if}

<style>
  .loading {
    text-align: center;
    padding: 2rem;
    color: var(--rumi-text-muted);
    font-size: 0.875rem;
  }

  .pool-list {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .pool-card {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    padding: 1rem 1.25rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    cursor: pointer;
    transition: all 0.15s ease;
    text-align: left;
    width: 100%;
    color: inherit;
    font-family: inherit;
  }

  .pool-card:hover {
    border-color: var(--rumi-teal);
    box-shadow: 0 0 0 1px rgba(45, 212, 191, 0.1);
  }

  .pool-card-empty {
    opacity: 0.5;
    cursor: default;
  }

  .pool-card-empty:hover {
    border-color: var(--rumi-border);
    box-shadow: none;
  }

  .pool-pair {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .pool-dots {
    display: flex;
    gap: 0.125rem;
  }

  .pool-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    display: inline-block;
  }

  .pool-name {
    font-size: 0.9375rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
  }

  .pool-tokens {
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
  }

  .pool-empty-text {
    font-size: 0.8125rem;
    color: var(--rumi-text-muted);
  }

  .pool-stats {
    display: flex;
    gap: 1.5rem;
  }

  .pool-stat {
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
  }

  .stat-label {
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .stat-value {
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    font-variant-numeric: tabular-nums;
  }

  .lp-value {
    color: var(--rumi-teal);
  }

  .pool-action {
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-teal);
  }
</style>
```

- [ ] **Step 2: Commit**

```bash
git add src/vault_frontend/src/lib/components/swap/PoolListView.svelte
git commit -m "feat(frontend): add pool list view for liquidity tab"
```

---

### Task 8: AMM Liquidity Panel

Add/remove liquidity for the 3USD/ICP AMM pool with percentage slider for removal.

**Files:**
- Create: `src/vault_frontend/src/lib/components/swap/AmmLiquidityPanel.svelte`

- [ ] **Step 1: Create AmmLiquidityPanel.svelte**

This component:
- Has "Add" and "Remove" sub-tabs
- Add: two input fields (3USD amount + ICP amount), proportional to pool reserves
- Remove: percentage slider (25/50/75/100%), shows estimated output amounts
- Uses `ammService` for all operations

```svelte
<script lang="ts">
  import { createEventDispatcher, onMount } from 'svelte';
  import { walletStore } from '../../stores/wallet';
  import { ammService, AMM_TOKENS, parseTokenAmount, formatTokenAmount, getLedgerFee, approvalAmount } from '../../services/ammService';
  import type { PoolInfo } from '../../services/ammService';
  import { CANISTER_IDS } from '../../config';

  const dispatch = createEventDispatcher();

  type Tab = 'add' | 'remove';
  let activeTab: Tab = 'add';

  // Pool data
  let pool: PoolInfo | null = null;
  let userLpShares = 0n;
  let loading = false;
  let error = '';
  let poolLoading = true;

  // Add liquidity state
  let addAmountA = ''; // 3USD
  let addAmountB = ''; // ICP
  let addLoading = false;
  let slippageBps = 50;

  // Remove liquidity state
  let removePercent = 0;
  let removeLoading = false;

  $: isConnected = $walletStore.isConnected;

  // Token references
  const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
  const icpToken = AMM_TOKENS.find(t => t.symbol === 'ICP')!;

  // Balances
  $: threeUsdBalance = $walletStore.tokenBalances?.THREEUSD?.raw ?? 0n;
  $: icpBalance = $walletStore.tokenBalances?.ICP?.raw ?? 0n;

  // Estimated removal amounts
  $: removeEstimateA = (() => {
    if (!pool || pool.total_lp_shares === 0n || userLpShares === 0n || removePercent === 0) return 0n;
    const sharesToBurn = userLpShares * BigInt(removePercent) / 100n;
    return pool.reserve_a * sharesToBurn / pool.total_lp_shares;
  })();

  $: removeEstimateB = (() => {
    if (!pool || pool.total_lp_shares === 0n || userLpShares === 0n || removePercent === 0) return 0n;
    const sharesToBurn = userLpShares * BigInt(removePercent) / 100n;
    return pool.reserve_b * sharesToBurn / pool.total_lp_shares;
  })();

  onMount(loadPool);

  async function loadPool() {
    poolLoading = true;
    try {
      const pools = await ammService.getPools();
      const threePoolId = CANISTER_IDS.THREEPOOL;
      const icpLedgerId = CANISTER_IDS.ICP_LEDGER;
      pool = pools.find(p => {
        const a = p.token_a.toText();
        const b = p.token_b.toText();
        return (a === threePoolId && b === icpLedgerId) || (a === icpLedgerId && b === threePoolId);
      }) ?? null;
      if (pool && isConnected && $walletStore.principal) {
        userLpShares = await ammService.getLpBalance(pool.pool_id, $walletStore.principal);
      }
    } catch (e) {
      console.error('Failed to load AMM pool:', e);
    } finally {
      poolLoading = false;
    }
  }

  async function handleAdd() {
    if (!pool) return;
    const amtA = addAmountA ? parseTokenAmount(addAmountA, threeUsdToken.decimals) : 0n;
    const amtB = addAmountB ? parseTokenAmount(addAmountB, icpToken.decimals) : 0n;
    if (amtA === 0n && amtB === 0n) {
      error = 'Enter at least one amount';
      return;
    }

    try {
      addLoading = true;
      error = '';
      // Estimate LP shares and apply slippage protection
      // For initial deposit (empty pool), minLp=0 is fine since there's nothing to front-run
      let minLp = 0n;
      if (pool.total_lp_shares > 0n) {
        // Proportional estimate: min(amtA * total / reserveA, amtB * total / reserveB)
        const estA = amtA > 0n ? amtA * pool.total_lp_shares / pool.reserve_a : BigInt(Number.MAX_SAFE_INTEGER);
        const estB = amtB > 0n ? amtB * pool.total_lp_shares / pool.reserve_b : BigInt(Number.MAX_SAFE_INTEGER);
        const lpEstimate = estA < estB ? estA : estB;
        minLp = lpEstimate * BigInt(10000 - slippageBps) / 10000n;
      }
      await ammService.addLiquidity(pool.pool_id, amtA, amtB, minLp, threeUsdToken, icpToken);
      dispatch('success', { action: 'add_liquidity' });
      addAmountA = '';
      addAmountB = '';
      await loadPool();
    } catch (err: any) {
      error = err.message || 'Add liquidity failed';
    } finally {
      addLoading = false;
    }
  }

  async function handleRemove() {
    if (!pool || removePercent === 0 || userLpShares === 0n) return;

    try {
      removeLoading = true;
      error = '';
      const sharesToBurn = userLpShares * BigInt(removePercent) / 100n;
      // Apply slippage to estimates
      const minA = removeEstimateA * BigInt(10000 - slippageBps) / 10000n;
      const minB = removeEstimateB * BigInt(10000 - slippageBps) / 10000n;
      await ammService.removeLiquidity(pool.pool_id, sharesToBurn, minA, minB);
      dispatch('success', { action: 'remove_liquidity' });
      removePercent = 0;
      await loadPool();
    } catch (err: any) {
      error = err.message || 'Remove liquidity failed';
    } finally {
      removeLoading = false;
    }
  }

  function goBack() {
    dispatch('back');
  }
</script>

{#if poolLoading}
  <div class="loading-text">Loading pool...</div>
{:else if !pool}
  <div class="empty-text">3USD/ICP pool not yet created.</div>
{:else}
  <button class="back-btn" on:click={goBack}>← All pools</button>

  <!-- Pool overview -->
  <div class="pool-overview">
    <div class="overview-pair">
      <span class="pool-dot" style="background:#34d399"></span>
      <span class="pool-dot" style="background:#29abe2"></span>
      <span class="overview-name">3USD / ICP</span>
    </div>
    <div class="overview-stats">
      <span>3USD: {formatTokenAmount(pool.reserve_a, 8)}</span>
      <span>ICP: {formatTokenAmount(pool.reserve_b, 8)}</span>
    </div>
    {#if isConnected && userLpShares > 0n}
      <div class="user-position">
        Your LP: {formatTokenAmount(userLpShares, 8)} shares
      </div>
    {/if}
  </div>

  <!-- Tabs -->
  <div class="sub-tabs">
    <button class="sub-tab" class:active={activeTab === 'add'} on:click={() => { activeTab = 'add'; error = ''; }}>Add</button>
    <button class="sub-tab" class:active={activeTab === 'remove'} on:click={() => { activeTab = 'remove'; error = ''; }}>Remove</button>
  </div>

  {#if activeTab === 'add'}
    {#if !isConnected}
      <p class="connect-text">Connect your wallet to add liquidity</p>
    {:else}
      <div class="input-group">
        <label class="input-label">3USD</label>
        <input type="number" step="any" min="0" placeholder="0.00" bind:value={addAmountA} disabled={addLoading} class="token-input" />
        <span class="input-balance">Bal: {formatTokenAmount(threeUsdBalance, 8)}</span>
      </div>
      <div class="input-group">
        <label class="input-label">ICP</label>
        <input type="number" step="any" min="0" placeholder="0.00" bind:value={addAmountB} disabled={addLoading} class="token-input" />
        <span class="input-balance">Bal: {formatTokenAmount(icpBalance, 8)}</span>
      </div>
      <button class="submit-btn" on:click={handleAdd} disabled={addLoading}>
        {#if addLoading}
          <span class="spinner"></span> Adding...
        {:else}
          Add Liquidity
        {/if}
      </button>
    {/if}

  {:else}
    {#if !isConnected}
      <p class="connect-text">Connect your wallet to remove liquidity</p>
    {:else if userLpShares === 0n}
      <p class="connect-text">You have no LP shares in this pool</p>
    {:else}
      <!-- Percentage slider -->
      <div class="slider-section">
        <div class="slider-header">
          <span class="slider-label">Amount to remove</span>
          <span class="slider-value">{removePercent}%</span>
        </div>
        <input type="range" min="0" max="100" step="1" bind:value={removePercent} class="slider" />
        <div class="slider-presets">
          <button class="preset-btn" class:active={removePercent === 25} on:click={() => { removePercent = 25; }}>25%</button>
          <button class="preset-btn" class:active={removePercent === 50} on:click={() => { removePercent = 50; }}>50%</button>
          <button class="preset-btn" class:active={removePercent === 75} on:click={() => { removePercent = 75; }}>75%</button>
          <button class="preset-btn" class:active={removePercent === 100} on:click={() => { removePercent = 100; }}>100%</button>
        </div>
      </div>

      {#if removePercent > 0}
        <div class="remove-estimates">
          <div class="estimate-row">
            <span>3USD</span>
            <span>{formatTokenAmount(removeEstimateA, 8)}</span>
          </div>
          <div class="estimate-row">
            <span>ICP</span>
            <span>{formatTokenAmount(removeEstimateB, 8)}</span>
          </div>
        </div>
      {/if}

      <button class="submit-btn remove-btn" on:click={handleRemove} disabled={removeLoading || removePercent === 0}>
        {#if removeLoading}
          <span class="spinner"></span> Removing...
        {:else}
          Remove {removePercent}% Liquidity
        {/if}
      </button>
    {/if}
  {/if}

  {#if error}
    <div class="error-bar">
      <svg viewBox="0 0 16 16" fill="currentColor" width="14" height="14">
        <path d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1zm0 10.5a.75.75 0 1 1 0-1.5.75.75 0 0 1 0 1.5zM8.75 8a.75.75 0 0 1-1.5 0V5a.75.75 0 0 1 1.5 0v3z"/>
      </svg>
      {error}
    </div>
  {/if}
{/if}

<style>
  .loading-text, .empty-text, .connect-text {
    text-align: center;
    padding: 1.5rem;
    color: var(--rumi-text-muted);
    font-size: 0.8125rem;
  }

  .back-btn {
    background: none;
    border: none;
    color: var(--rumi-teal);
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    padding: 0;
    margin-bottom: 1rem;
  }

  .back-btn:hover { text-decoration: underline; }

  .pool-overview {
    padding: 0.75rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    margin-bottom: 1rem;
  }

  .overview-pair {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    margin-bottom: 0.5rem;
  }

  .pool-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    display: inline-block;
  }

  .overview-name {
    font-size: 0.9375rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
  }

  .overview-stats {
    display: flex;
    gap: 1rem;
    font-size: 0.75rem;
    color: var(--rumi-text-secondary);
    font-variant-numeric: tabular-nums;
  }

  .user-position {
    margin-top: 0.5rem;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--rumi-teal);
  }

  /* Sub tabs */
  .sub-tabs {
    display: flex;
    gap: 0.25rem;
    margin-bottom: 1rem;
  }

  .sub-tab {
    flex: 1;
    padding: 0.375rem 0;
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    background: transparent;
    color: var(--rumi-text-muted);
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
  }

  .sub-tab.active {
    background: var(--rumi-bg-surface2);
    color: var(--rumi-text-primary);
    border-color: var(--rumi-teal);
    font-weight: 600;
  }

  /* Input groups */
  .input-group {
    margin-bottom: 0.75rem;
  }

  .input-label {
    display: block;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    margin-bottom: 0.25rem;
  }

  .token-input {
    width: 100%;
    padding: 0.75rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    color: var(--rumi-text-primary);
    font-size: 1rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    -moz-appearance: textfield;
    appearance: textfield;
  }

  .token-input::-webkit-inner-spin-button,
  .token-input::-webkit-outer-spin-button {
    -webkit-appearance: none;
  }

  .token-input:focus {
    outline: none;
    border-color: var(--rumi-teal);
  }

  .input-balance {
    display: block;
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
    margin-top: 0.25rem;
    text-align: right;
  }

  /* Slider */
  .slider-section {
    margin-bottom: 1rem;
  }

  .slider-header {
    display: flex;
    justify-content: space-between;
    margin-bottom: 0.5rem;
  }

  .slider-label {
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
  }

  .slider-value {
    font-size: 1.25rem;
    font-weight: 700;
    color: var(--rumi-text-primary);
  }

  .slider {
    width: 100%;
    -webkit-appearance: none;
    height: 4px;
    background: var(--rumi-border);
    border-radius: 2px;
    outline: none;
  }

  .slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--rumi-teal);
    cursor: pointer;
  }

  .slider-presets {
    display: flex;
    gap: 0.375rem;
    margin-top: 0.5rem;
    justify-content: center;
  }

  .preset-btn {
    padding: 0.25rem 0.75rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-text-secondary);
    cursor: pointer;
    transition: all 0.15s;
  }

  .preset-btn:hover { border-color: var(--rumi-teal); color: var(--rumi-teal); }
  .preset-btn.active {
    background: var(--rumi-teal-dim);
    border-color: var(--rumi-border-teal);
    color: var(--rumi-teal);
    font-weight: 600;
  }

  .remove-estimates {
    padding: 0.75rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    margin-bottom: 1rem;
  }

  .estimate-row {
    display: flex;
    justify-content: space-between;
    font-size: 0.8125rem;
    font-variant-numeric: tabular-nums;
    padding: 0.25rem 0;
  }

  .estimate-row span:first-child { color: var(--rumi-text-muted); }
  .estimate-row span:last-child { color: var(--rumi-text-primary); font-weight: 600; }

  /* Submit button */
  .submit-btn {
    width: 100%;
    padding: 0.875rem;
    margin-top: 0.5rem;
    background: var(--rumi-action);
    color: var(--rumi-bg-primary);
    border: none;
    border-radius: 0.5rem;
    font-size: 0.9375rem;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.2s;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
  }

  .submit-btn:hover:not(:disabled) {
    background: var(--rumi-action-bright);
    box-shadow: 0 0 20px rgba(52, 211, 153, 0.15);
  }

  .submit-btn:disabled { opacity: 0.4; cursor: not-allowed; }

  .remove-btn { background: var(--rumi-danger, #e06b9f); }
  .remove-btn:hover:not(:disabled) { background: #c85a8a; box-shadow: none; }

  .spinner {
    width: 1rem;
    height: 1rem;
    border: 2px solid transparent;
    border-top-color: currentColor;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin { to { transform: rotate(360deg); } }

  .error-bar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 0.75rem;
    padding: 0.625rem 0.75rem;
    background: rgba(224, 107, 159, 0.08);
    border: 1px solid rgba(224, 107, 159, 0.2);
    border-radius: 0.375rem;
    color: var(--rumi-danger);
    font-size: 0.8125rem;
  }
</style>
```

- [ ] **Step 2: Commit**

```bash
git add src/vault_frontend/src/lib/components/swap/AmmLiquidityPanel.svelte
git commit -m "feat(frontend): add AMM liquidity panel with percentage slider removal"
```

---

### Task 9: Update Swap Page with Toggle + Routing

Wire everything together in the swap page route.

**Files:**
- Modify: `src/vault_frontend/src/routes/swap/+page.svelte`

- [ ] **Step 1: Rewrite swap/+page.svelte**

Replace the entire file:

```svelte
<script lang="ts">
  import { walletStore } from '../../lib/stores/wallet';
  import SwapInterface from '../../lib/components/swap/SwapInterface.svelte';
  import SwapLiquidityToggle from '../../lib/components/swap/SwapLiquidityToggle.svelte';
  import PoolListView from '../../lib/components/swap/PoolListView.svelte';
  import AmmLiquidityPanel from '../../lib/components/swap/AmmLiquidityPanel.svelte';
  import LiquidityInterface from '../../lib/components/swap/LiquidityInterface.svelte';

  let mode: 'swap' | 'liquidity' = 'swap';
  let liquidityView: 'list' | 'threepool' | 'amm' = 'list';

  function handleSuccess() {
    walletStore.refreshBalance();
  }

  function handlePoolSelect(e: CustomEvent<{ pool: 'threepool' | 'amm' }>) {
    liquidityView = e.detail.pool;
  }

  function handleBack() {
    liquidityView = 'list';
  }
</script>

<svelte:head>
  <title>{mode === 'swap' ? 'Swap' : 'Liquidity'} | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <div class="page-header">
    <h1 class="page-title">{mode === 'swap' ? 'Swap' : 'Liquidity'}</h1>
  </div>

  <div class="action-column">
    <div class="action-panel">
      <SwapLiquidityToggle bind:mode on:change={() => { liquidityView = 'list'; }} />

      {#if mode === 'swap'}
        <SwapInterface on:success={handleSuccess} />
      {:else if liquidityView === 'list'}
        <PoolListView on:select={handlePoolSelect} />
      {:else if liquidityView === 'threepool'}
        <div>
          <button class="back-link" on:click={handleBack}>← All pools</button>
          <p class="explainer">Deposit stablecoins to mint 3USD</p>
          <LiquidityInterface on:success={handleSuccess} />
        </div>
      {:else if liquidityView === 'amm'}
        <AmmLiquidityPanel on:success={handleSuccess} on:back={handleBack} />
      {/if}
    </div>
  </div>
</div>

<style>
  .page-container {
    max-width: 420px;
    margin: 0 auto;
    padding-bottom: 4rem;
  }

  .page-header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 1.75rem;
    animation: fadeSlideIn 0.5s ease-out both;
  }

  @keyframes fadeSlideIn {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .action-column {
    display: flex;
    flex-direction: column;
    align-items: center;
  }

  .action-column > :global(*) { width: 100%; }

  .action-panel {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
    box-shadow:
      inset 0 1px 0 0 rgba(200, 210, 240, 0.03),
      0 2px 8px -2px rgba(8, 11, 22, 0.6);
  }

  .back-link {
    background: none;
    border: none;
    color: var(--rumi-teal);
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    padding: 0;
    margin-bottom: 1rem;
  }

  .back-link:hover { text-decoration: underline; }

  .explainer {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    margin: 0 0 1.25rem;
    line-height: 1.5;
  }

  @media (max-width: 520px) {
    .page-container {
      padding-left: 0.5rem;
      padding-right: 0.5rem;
    }
  }
</style>
```

- [ ] **Step 2: Verify the build compiles**

Run: `cd /Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend && npm run build 2>&1 | tail -30`

Fix any compilation errors.

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/routes/swap/+page.svelte
git commit -m "feat(frontend): wire swap page with toggle, pool list, and AMM liquidity"
```

---

### Task 10: Final Integration Verification

End-to-end verification that everything compiles and the token routing logic is correct.

**Files:**
- All files from tasks 1-9

- [ ] **Step 1: Full build check**

```bash
cd /Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend && npm run build
```

Must complete without errors.

- [ ] **Step 2: Verify the AMM .did.js matches the Candid source**

Compare `src/declarations/rumi_amm/rumi_amm.did.js` method signatures against `src/rumi_amm/rumi_amm.did` to ensure they match exactly. Pay special attention to:
- `get_quote` parameter count (pool_id, token_in, amount_in)
- `remove_liquidity` return type (tuple of two nats)
- `swap` parameter order (pool_id, token_in, amount_in, min_amount_out)

- [ ] **Step 3: Verify routing table completeness**

Manually trace each row of the routing table through `swapRouter.ts`:
- icUSD -> ckUSDT: hits `isStablecoin(from) && isStablecoin(to)` → `three_pool_swap` ✓
- ckUSDC -> 3USD: hits `isStablecoin(from) && is3USD(to)` → `three_pool_deposit` ✓
- 3USD -> ckUSDT: hits `is3USD(from) && isStablecoin(to)` → `three_pool_redeem` ✓
- 3USD -> ICP: hits `is3USD(from) && isICP(to)` → `amm_swap` ✓
- ICP -> 3USD: hits `isICP(from) && is3USD(to)` → `amm_swap` ✓
- ckUSDC -> ICP: hits `isStablecoin(from) && isICP(to)` → `stable_to_icp` ✓
- ICP -> icUSD: hits `isICP(from) && isStablecoin(to)` → `icp_to_stable` ✓

- [ ] **Step 4: Commit any fixes**

If any fixes were needed during verification:
```bash
git add -A
git commit -m "fix(frontend): integration fixes from build verification"
```
