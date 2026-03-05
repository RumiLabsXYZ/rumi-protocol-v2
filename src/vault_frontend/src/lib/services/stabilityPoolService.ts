import { Principal } from '@dfinity/principal';
import { pnp, canisterIDLs } from './pnp';
import { walletStore } from '../stores/wallet';
import { get } from 'svelte/store';
import { CANISTER_IDS } from '../config';

// ──────────────────────────────────────────────────────────────
// Types — mirrors the Candid interface
// ──────────────────────────────────────────────────────────────

export interface StablecoinConfig {
  ledger_id: Principal;
  symbol: string;
  decimals: number;
  priority: number;
  is_active: boolean;
}

export interface CollateralInfo {
  ledger_id: Principal;
  symbol: string;
  decimals: number;
  status: { Active: null } | { Paused: null } | { Frozen: null } | { Sunset: null } | { Deprecated: null };
}

export interface PoolStatus {
  total_deposits_e8s: bigint;
  total_depositors: bigint;
  total_liquidations_executed: bigint;
  stablecoin_balances: Array<[Principal, bigint]>;
  collateral_gains: Array<[Principal, bigint]>;
  stablecoin_registry: StablecoinConfig[];
  collateral_registry: CollateralInfo[];
  emergency_paused: boolean;
}

export interface UserPosition {
  stablecoin_balances: Array<[Principal, bigint]>;
  collateral_gains: Array<[Principal, bigint]>;
  opted_out_collateral: Principal[];
  deposit_timestamp: bigint;
  total_claimed_gains: Array<[Principal, bigint]>;
  total_usd_value_e8s: bigint;
}

export interface LiquidationRecord {
  vault_id: bigint;
  timestamp: bigint;
  stables_consumed: Array<[Principal, bigint]>;
  collateral_gained: bigint;
  collateral_type: Principal;
  depositors_count: bigint;
}

// ──────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────

const E8S = 100_000_000;

/** Convert raw token amount from native decimals to a display string. */
export function formatTokenAmount(amount: bigint, decimals: number, maxFractionDigits?: number): string {
  const divisor = Math.pow(10, decimals);
  const value = Number(amount) / divisor;
  const fracDigits = maxFractionDigits ?? Math.min(decimals, 6);
  // Strip trailing zeros
  const fixed = value.toFixed(fracDigits);
  if (fixed.includes('.')) {
    let trimmed = fixed.replace(/0+$/, '');
    if (trimmed.endsWith('.')) trimmed = trimmed.slice(0, -1);
    return trimmed;
  }
  return fixed;
}

/** Convert raw e8s to display USD value. */
export function formatE8s(amount: bigint, maxFractionDigits: number = 2): string {
  return formatTokenAmount(amount, 8, maxFractionDigits);
}

/** Parse a user-entered amount string to raw token units. */
export function parseTokenAmount(amount: string, decimals: number): bigint {
  const value = parseFloat(amount);
  if (isNaN(value) || value < 0) throw new Error('Invalid amount');
  return BigInt(Math.floor(value * Math.pow(10, decimals)));
}

/** Normalize an amount to e8s for consistent comparison. */
export function normalizeToE8s(amount: bigint, decimals: number): bigint {
  if (decimals === 8) return amount;
  if (decimals < 8) return amount * BigInt(Math.pow(10, 8 - decimals));
  return amount / BigInt(Math.pow(10, decimals - 8));
}

/** Get collateral status as a readable string. */
export function getCollateralStatusLabel(status: CollateralInfo['status']): string {
  if ('Active' in status) return 'Active';
  if ('Paused' in status) return 'Paused';
  if ('Frozen' in status) return 'Frozen';
  if ('Sunset' in status) return 'Sunset';
  if ('Deprecated' in status) return 'Deprecated';
  return 'Unknown';
}

/** Map well-known ledger principals to token symbols for display. */
const KNOWN_SYMBOLS: Record<string, string> = {
  [CANISTER_IDS.ICUSD_LEDGER]: 'icUSD',
  [CANISTER_IDS.CKUSDT_LEDGER]: 'ckUSDT',
  [CANISTER_IDS.CKUSDC_LEDGER]: 'ckUSDC',
  [CANISTER_IDS.ICP_LEDGER]: 'ICP',
};

export function symbolForLedger(ledger: Principal, registries?: { stablecoins?: StablecoinConfig[]; collateral?: CollateralInfo[] }): string {
  const text = ledger.toText();
  if (KNOWN_SYMBOLS[text]) return KNOWN_SYMBOLS[text];
  // Fall back to registry lookups
  if (registries?.stablecoins) {
    const sc = registries.stablecoins.find(s => s.ledger_id.toText() === text);
    if (sc) return sc.symbol;
  }
  if (registries?.collateral) {
    const ci = registries.collateral.find(c => c.ledger_id.toText() === text);
    if (ci) return ci.symbol;
  }
  return text.slice(0, 5) + '…';
}

export function decimalsForLedger(ledger: Principal, registries?: { stablecoins?: StablecoinConfig[]; collateral?: CollateralInfo[] }): number {
  if (registries?.stablecoins) {
    const sc = registries.stablecoins.find(s => s.ledger_id.toText() === ledger.toText());
    if (sc) return sc.decimals;
  }
  if (registries?.collateral) {
    const ci = registries.collateral.find(c => c.ledger_id.toText() === ledger.toText());
    if (ci) return ci.decimals;
  }
  // Defaults for well-known tokens
  const text = ledger.toText();
  if (text === CANISTER_IDS.CKUSDT_LEDGER || text === CANISTER_IDS.CKUSDC_LEDGER) return 6;
  return 8; // ICP, icUSD, ckBTC all use 8
}

// ──────────────────────────────────────────────────────────────
// Service
// ──────────────────────────────────────────────────────────────

const STABILITY_POOL_CANISTER_ID = CANISTER_IDS.STABILITY_POOL;

class StabilityPoolService {
  // Returns the canister actor. PNP returns a generic Actor type,
  // so callers use (actor as any).method() — standard ICP pattern.
  private async getActor(): Promise<any> {
    const actor = await pnp.getActor(
      STABILITY_POOL_CANISTER_ID,
      canisterIDLs.stability_pool,
    );
    if (!actor) throw new Error('Failed to connect to stability pool');
    return actor;
  }

  // ── Queries ──

  async getPoolStatus(): Promise<PoolStatus> {
    const actor = await this.getActor();
    return await actor.get_pool_status() as PoolStatus;
  }

  async getUserPosition(userPrincipal?: Principal): Promise<UserPosition | null> {
    const actor = await this.getActor();
    const arg = userPrincipal ? [userPrincipal] : [];
    const result = await actor.get_user_position(arg) as [UserPosition] | [];
    return result.length > 0 ? result[0] ?? null : null;
  }

  async getLiquidationHistory(limit?: number): Promise<LiquidationRecord[]> {
    const actor = await this.getActor();
    const arg = limit !== undefined ? [BigInt(limit)] : [];
    return await actor.get_liquidation_history(arg) as LiquidationRecord[];
  }

  async checkPoolCapacity(tokenLedger: Principal, amount: bigint): Promise<boolean> {
    const actor = await this.getActor();
    return await actor.check_pool_capacity(tokenLedger, amount) as boolean;
  }

  // ── Mutations ──

  async deposit(tokenLedger: Principal, amount: bigint): Promise<void> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const actor = await this.getActor();
    const result = await actor.deposit(tokenLedger, amount) as { Ok: null } | { Err: any };
    if ('Err' in result) {
      throw new Error(this.formatError(result.Err));
    }
  }

  async withdraw(tokenLedger: Principal, amount: bigint): Promise<void> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const actor = await this.getActor();
    const result = await actor.withdraw(tokenLedger, amount) as { Ok: null } | { Err: any };
    if ('Err' in result) {
      throw new Error(this.formatError(result.Err));
    }
  }

  async claimCollateral(collateralLedger: Principal): Promise<bigint> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const actor = await this.getActor();
    const result = await actor.claim_collateral(collateralLedger) as { Ok: bigint } | { Err: any };
    if ('Err' in result) {
      throw new Error(this.formatError(result.Err));
    }
    return result.Ok;
  }

  async claimAllCollateral(): Promise<Array<[Principal, bigint]>> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const actor = await this.getActor();
    const result = await actor.claim_all_collateral() as { Ok: Array<[Principal, bigint]> } | { Err: any };
    if ('Err' in result) {
      throw new Error(this.formatError(result.Err));
    }
    return result.Ok;
  }

  async optOutCollateral(collateralType: Principal): Promise<void> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const actor = await this.getActor();
    const result = await actor.opt_out_collateral(collateralType) as { Ok: null } | { Err: any };
    if ('Err' in result) {
      throw new Error(this.formatError(result.Err));
    }
  }

  async optInCollateral(collateralType: Principal): Promise<void> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const actor = await this.getActor();
    const result = await actor.opt_in_collateral(collateralType) as { Ok: null } | { Err: any };
    if ('Err' in result) {
      throw new Error(this.formatError(result.Err));
    }
  }

  async executeLiquidation(vaultId: bigint): Promise<any> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const actor = await this.getActor();
    const result = await actor.execute_liquidation(vaultId) as { Ok: any } | { Err: any };
    if ('Err' in result) {
      throw new Error(this.formatError(result.Err));
    }
    return result.Ok;
  }

  // ── Error formatting ──

  private formatError(err: any): string {
    if ('InsufficientBalance' in err) {
      return `Insufficient balance: need ${err.InsufficientBalance.required}, have ${err.InsufficientBalance.available}`;
    }
    if ('AmountTooLow' in err) {
      return `Amount too low (minimum: ${formatE8s(err.AmountTooLow.minimum_e8s)} USD)`;
    }
    if ('NoPositionFound' in err) return 'No deposit position found';
    if ('InsufficientPoolBalance' in err) return 'Pool has insufficient balance';
    if ('Unauthorized' in err) return 'Unauthorized';
    if ('TokenNotAccepted' in err) return 'Token not accepted by the pool';
    if ('TokenNotActive' in err) return 'Token is not currently active';
    if ('CollateralNotFound' in err) return 'Collateral type not found';
    if ('LedgerTransferFailed' in err) return `Transfer failed: ${err.LedgerTransferFailed.reason}`;
    if ('InterCanisterCallFailed' in err) return `Inter-canister call failed: ${err.InterCanisterCallFailed.method}`;
    if ('LiquidationFailed' in err) return `Liquidation failed: ${err.LiquidationFailed.reason}`;
    if ('EmergencyPaused' in err) return 'Pool is currently paused';
    if ('SystemBusy' in err) return 'System is busy, try again';
    if ('AlreadyOptedOut' in err) return 'Already opted out of this collateral';
    if ('AlreadyOptedIn' in err) return 'Already opted in for this collateral';
    return 'Unknown error';
  }
}

export const stabilityPoolService = new StabilityPoolService();
