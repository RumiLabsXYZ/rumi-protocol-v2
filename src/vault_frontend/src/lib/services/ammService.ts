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
  /** Whether this is the 3pool LP token (3USD) */
  is3USD: boolean;
  /** 3pool index if this is a stablecoin in the 3pool (-1 if not) */
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
      const ledgerActor = createOisyActor(inputToken.ledgerId, CONFIG.icusd_ledgerIDL, signerAgent);
      const ammActor = createOisyActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm, signerAgent);

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
      const ledgerActor = await walletStore.getActor(inputToken.ledgerId, CONFIG.icusd_ledgerIDL) as any;
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

      const ammActor = await walletStore.getActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm) as any;
      const result = await ammActor.swap(poolId, tokenIn, amountIn, minAmountOut);
      if ('Err' in result) throw new Error(this.formatError(result.Err));
      return result.Ok;
    }
  }

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
    if ('PoolCreationClosed' in err) return 'Pool creation is currently closed';
    if ('FeeBpsOutOfRange' in err) return 'Fee must be between 0.01% and 10%';
    if ('MaintenanceMode' in err) return 'AMM is in maintenance mode — swaps and deposits are temporarily disabled';
    return 'Unknown AMM error';
  }
}

export const ammService = new AmmService();
