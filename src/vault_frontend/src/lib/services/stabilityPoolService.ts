import { Principal } from '@dfinity/principal';
import { Actor, HttpAgent, AnonymousIdentity } from '@dfinity/agent';
import { pnp, canisterIDLs } from './pnp';
import { walletStore } from '../stores/wallet';
import { walletSessionGeneration } from './auth';
import { get } from 'svelte/store';
import { CANISTER_IDS, CONFIG } from '../config';
import { isOisyWallet } from './protocol/walletOperations';
import { getOisySignerAgent, createOisyActor } from './oisySigner';
import {
  ackNativeXrpPayoutSettledWithActor,
  getMyNativeXrpPayoutsWithActor,
  optInNativeCollateralWithTagUsingActor,
  type NativeXrpPendingPayout,
  type StabilityPoolNativeXrpActor,
} from './stabilityPoolNativeXrp';
import type { CandidOpt, XrpClaimId } from './xrpPayoutHelpers';

// ──────────────────────────────────────────────────────────────
// Types — mirrors the Candid interface
// ──────────────────────────────────────────────────────────────

export interface StablecoinConfig {
  ledger_id: Principal;
  symbol: string;
  decimals: number;
  priority: number;
  is_active: boolean;
  transfer_fee?: bigint;
  is_lp_token?: boolean;
  underlying_pool?: Principal;
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
  eligible_icusd_per_collateral: Array<[Principal, bigint]>;
  eligible_usd_per_collateral?: CandidOpt<Array<[Principal, bigint]>>;
}

export interface UserPosition {
  stablecoin_balances: Array<[Principal, bigint]>;
  collateral_gains: Array<[Principal, bigint]>;
  opted_out_collateral: Principal[];
  eligible_interest_collateral?: CandidOpt<Principal[]>;
  // Candid `opt vec` — decodes as `[]` (absent / older canister) or `[[...]]`.
  // Unwrap with `native_payout_addresses?.[0] ?? []` before use.
  native_payout_addresses?: [] | [Array<[Principal, string]>];
  native_payout_destination_tags?: CandidOpt<Array<[Principal, number]>>;
  pending_native_xrp_payouts?: CandidOpt<Array<[bigint, NativeXrpPendingPayout]>>;
  deposit_timestamp: bigint;
  total_claimed_gains: Array<[Principal, bigint]>;
  total_usd_value_e8s: bigint;
  total_interest_earned_e8s?: bigint;
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

/**
 * Convert raw token amount to a display string.
 * App rule: max 2 decimal places unless the value is tiny (e.g. 0.001 BTC).
 * Always rounds DOWN (floor) to avoid overstating balances.
 */
export function formatTokenAmount(amount: bigint, decimals: number, maxFractionDigits?: number): string {
  const divisor = Math.pow(10, decimals);
  const value = Number(amount) / divisor;

  // Determine precision: caller override → auto (2 unless tiny value needs more)
  let fracDigits: number;
  if (maxFractionDigits !== undefined) {
    fracDigits = maxFractionDigits;
  } else if (value > 0 && value < 0.01) {
    fracDigits = Math.min(decimals, 6);
  } else {
    fracDigits = 2;
  }

  // Round DOWN (floor) at the chosen precision
  const multiplier = Math.pow(10, fracDigits);
  const floored = Math.floor(value * multiplier) / multiplier;

  const fixed = floored.toFixed(fracDigits);
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
  [CANISTER_IDS.THREEPOOL]: '3USD',
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

export type StabilityPoolAction = 'deposit' | 'withdraw';

export interface StabilityPoolActionContext {
  readonly principalText: string;
  readonly walletIcon: string;
  readonly sessionGeneration: number;
  readonly ledgerText: string;
  readonly action: StabilityPoolAction;
  readonly oisy: boolean;
}

export interface PendingStabilityPoolDeposit {
  owner: string;
  ledger: string;
  action: 'deposit';
  amount: string;
  intentSeq: string;
  walletIcon: string;
  oisy: boolean;
  createdAt: number;
  status: 'pending';
}

type DepositIntentResult =
  | { Completed: { intent_seq: bigint; token_ledger: Principal; amount: bigint; block_index: bigint } }
  | { NoEffect: { intent_seq: bigint; token_ledger: Principal; amount: bigint; reason: string } }
  | { Pending: { intent_seq: bigint; token_ledger: Principal; amount: bigint; phase: unknown; reason: [] | [string] } };

interface DepositIntentStatus {
  high_watermark: bigint;
  next_seq: [] | [bigint];
  intent: [] | [DepositIntentResult];
  active_intent: [] | [DepositIntentResult];
}

const candidOption = <T>(value: [] | [T] | undefined): T | undefined => value?.[0];

function intentPayload(result: DepositIntentResult): { seq: bigint; ledger: string; amount: bigint; state: 'completed' | 'no-effect' | 'pending'; reason?: string } {
  if ('Completed' in result) {
    const value = result.Completed;
    return { seq: value.intent_seq, ledger: value.token_ledger.toText(), amount: value.amount, state: 'completed' };
  }
  if ('NoEffect' in result) {
    const value = result.NoEffect;
    return { seq: value.intent_seq, ledger: value.token_ledger.toText(), amount: value.amount, state: 'no-effect', reason: value.reason };
  }
  const value = result.Pending;
  return { seq: value.intent_seq, ledger: value.token_ledger.toText(), amount: value.amount, state: 'pending', reason: candidOption(value.reason) };
}

function assertIntentPayload(record: PendingStabilityPoolDeposit, result: DepositIntentResult): void {
  const observed = intentPayload(result);
  if (observed.seq !== BigInt(record.intentSeq) || observed.ledger !== record.ledger || observed.amount !== BigInt(record.amount)) {
    throw new PendingStabilityPoolDepositError('The canister returned a different deposit intent. Keep the lock and contact support.');
  }
}

function bindToAuthoritativePendingIntent(
  record: PendingStabilityPoolDeposit,
  active: DepositIntentResult,
  context: StabilityPoolActionContext,
): void {
  try {
    assertIntentPayload(record, active);
  } catch {
    const authoritative = pendingRecord(record.owner, active, context);
    persistAuthoritativePendingIntent(authoritative);
    throw new PendingStabilityPoolDepositError(
      `The canister reports active intent ${authoritative.intentSeq} for ledger ${authoritative.ledger} and ${authoritative.amount} raw units. The local retry has been bound to that exact intent.`,
    );
  }
}

function pendingRecord(owner: string, result: DepositIntentResult, context: StabilityPoolActionContext): PendingStabilityPoolDeposit {
  const payload = intentPayload(result);
  if (payload.state !== 'pending') throw new Error('Expected a pending Stability Pool deposit intent.');
  return {
    owner,
    ledger: payload.ledger,
    action: 'deposit',
    amount: payload.amount.toString(),
    intentSeq: payload.seq.toString(),
    walletIcon: context.walletIcon,
    oisy: context.oisy,
    createdAt: Date.now(),
    status: 'pending',
  };
}

interface StabilityPoolLockManager {
  request<T>(
    name: string,
    options: { mode: 'exclusive'; ifAvailable: true },
    callback: (lock: unknown | null) => Promise<T>,
  ): Promise<T>;
}

export class PendingStabilityPoolDepositError extends Error {
  constructor(message = 'A prior Stability Pool deposit intent is unresolved. Resume its exact sequence, ledger, and amount before starting another deposit.') {
    super(message);
    this.name = 'PendingStabilityPoolDepositError';
  }
}

const pendingDepositKey = (owner: string) => `rumi:stability-pool:pending-deposit:${owner}`;

function localStorageOrThrow(): Storage {
  if (typeof localStorage === 'undefined') {
    throw new Error('Persistent browser storage is unavailable. The deposit was not submitted.');
  }
  return localStorage;
}

export function readPendingStabilityPoolDeposit(owner: string): PendingStabilityPoolDeposit | null {
  const serialized = localStorageOrThrow().getItem(pendingDepositKey(owner));
  if (serialized === null) return null;
  try {
    const record = JSON.parse(serialized) as Partial<PendingStabilityPoolDeposit>;
    const validLedger = typeof record.ledger === 'string' && Principal.fromText(record.ledger).toText() === record.ledger;
    if (record.owner === owner && validLedger && record.action === 'deposit' &&
        typeof record.amount === 'string' && /^\d+$/.test(record.amount) &&
        typeof record.intentSeq === 'string' && /^\d+$/.test(record.intentSeq) &&
        typeof record.walletIcon === 'string' && typeof record.oisy === 'boolean' &&
        typeof record.createdAt === 'number' && record.status === 'pending') {
      return record as PendingStabilityPoolDeposit;
    }
  } catch {
    // A damaged marker remains a lock; unreadable state must never permit a retry.
  }
  throw new PendingStabilityPoolDepositError(
    'A Stability Pool intent record is unreadable. Keep deposits locked and contact support for recovery.',
  );
}

function savePendingStabilityPoolDeposit(record: PendingStabilityPoolDeposit): void {
  const storage = localStorageOrThrow();
  if (readPendingStabilityPoolDeposit(record.owner)) {
    throw new PendingStabilityPoolDepositError();
  }
  const key = pendingDepositKey(record.owner);
  const serialized = JSON.stringify(record);
  storage.setItem(key, serialized);
  if (storage.getItem(key) !== serialized) {
    throw new Error('Could not persist the Stability Pool recovery lock. The deposit was not submitted.');
  }
}

function persistAuthoritativePendingIntent(record: PendingStabilityPoolDeposit): void {
  const storage = localStorageOrThrow();
  const key = pendingDepositKey(record.owner);
  const serialized = JSON.stringify(record);
  storage.setItem(key, serialized);
  if (storage.getItem(key) !== serialized) {
    throw new PendingStabilityPoolDepositError('Could not persist the canister-reported active intent. Keep deposits locked and contact support.');
  }
}

function refreshPendingWalletContext(record: PendingStabilityPoolDeposit, context: StabilityPoolActionContext): PendingStabilityPoolDeposit {
  const updated = { ...record, walletIcon: context.walletIcon, oisy: context.oisy };
  if (updated.walletIcon !== record.walletIcon || updated.oisy !== record.oisy) {
    persistAuthoritativePendingIntent(updated);
  }
  return updated;
}

function clearPendingStabilityPoolDeposit(owner: string, intentSeq: string): void {
  const storage = localStorageOrThrow();
  const current = readPendingStabilityPoolDeposit(owner);
  if (!current || current.intentSeq !== intentSeq) return;
  const key = pendingDepositKey(owner);
  storage.removeItem(key);
  if (storage.getItem(key) !== null) throw new Error('Could not clear the Stability Pool recovery lock.');
}

function isKnownSignerAbort(error: unknown): boolean {
  return !!error && typeof error === 'object' && (error as { code?: number }).code === 3001;
}

const inFlightActions = new Set<string>();

export function captureStabilityPoolActionContext(
  tokenLedger: Principal,
  action: StabilityPoolAction,
): StabilityPoolActionContext {
  const wallet = get(walletStore);
  if (wallet.loading) throw new Error('Wallet session is changing. Wait for the wallet refresh to finish.');
  const principalText = wallet.isConnected ? wallet.principal?.toText() : undefined;
  if (!principalText) throw new Error('Wallet not connected');
  return Object.freeze({
    principalText,
    walletIcon: wallet.icon,
    sessionGeneration: get(walletSessionGeneration),
    ledgerText: tokenLedger.toText(),
    action,
    oisy: isOisyWallet(),
  });
}

/** Pure validation for the click-time identity and operation parameters. */
export function assertStabilityPoolActionContext(
  context: StabilityPoolActionContext,
  livePrincipalText: string | null,
  liveWalletIcon: string,
  liveSessionGeneration: number,
  liveOisy: boolean,
  tokenLedger: Principal,
  action: StabilityPoolAction,
): void {
  if (context.principalText !== livePrincipalText || context.walletIcon !== liveWalletIcon ||
      context.sessionGeneration !== liveSessionGeneration ||
      context.oisy !== liveOisy ||
      context.ledgerText !== tokenLedger.toText() || context.action !== action) {
    throw new Error('Wallet session or Stability Pool action changed. Nothing further was submitted.');
  }
}

function assertCurrentAction(
  context: StabilityPoolActionContext,
  tokenLedger: Principal,
  action: StabilityPoolAction,
): void {
  const wallet = get(walletStore);
  if (wallet.loading) {
    throw new Error('Wallet session is changing. Nothing further was submitted.');
  }
  assertStabilityPoolActionContext(
    context,
    wallet.isConnected ? wallet.principal?.toText() ?? null : null,
    wallet.icon,
    get(walletSessionGeneration),
    isOisyWallet(),
    tokenLedger,
    action,
  );
}

function actionLockKey(context: StabilityPoolActionContext): string {
  return `${context.principalText}:${context.ledgerText}:${context.action}`;
}

async function withActionLock<T>(context: StabilityPoolActionContext, run: () => Promise<T>): Promise<T> {
  const key = actionLockKey(context);
  if (inFlightActions.has(key)) {
    throw new Error('A Stability Pool action for this token is already in progress.');
  }
  inFlightActions.add(key);
  try {
    return await run();
  } finally {
    inFlightActions.delete(key);
  }
}

async function withCrossTabDepositLock<T>(context: StabilityPoolActionContext, run: () => Promise<T>): Promise<T> {
  const locks = typeof navigator === 'undefined'
    ? undefined
    : navigator.locks as unknown as StabilityPoolLockManager;
  if (!locks) {
    throw new Error('This browser cannot safely coordinate Stability Pool deposits across tabs. No deposit was submitted.');
  }
  localStorageOrThrow();
  return locks.request(
    `rumi:stability-pool:deposit:${context.principalText}`,
    { mode: 'exclusive', ifAvailable: true },
    async lock => {
      if (!lock) {
        throw new PendingStabilityPoolDepositError('A Stability Pool deposit is active in another tab. Wait for its result before retrying.');
      }
      return run();
    },
  );
}

class StabilityPoolService {
  private _anonAgent: HttpAgent | null = null;

  /**
   * Anonymous actor for read-only queries. Bypasses wallet/ICRC-21 signer
   * so queries like get_pool_status don't trigger consent popups or fail
   * on canisters that don't implement icrc21_canister_call_consent_message.
   */
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
    return Actor.createActor(canisterIDLs.stability_pool as any, {
      agent: this._anonAgent,
      canisterId: STABILITY_POOL_CANISTER_ID,
    });
  }

  // ── Queries (anonymous, no wallet needed) ──

  async getPoolStatus(): Promise<PoolStatus> {
    const actor = await this.getQueryActor();
    return await actor.get_pool_status() as PoolStatus;
  }

  async getUserPosition(userPrincipal?: Principal): Promise<UserPosition | null> {
    const actor = await this.getQueryActor();
    const arg = userPrincipal ? [userPrincipal] : [];
    const result = await actor.get_user_position(arg) as [UserPosition] | [];
    return result.length > 0 ? result[0] ?? null : null;
  }

  async getLiquidationHistory(limit?: number): Promise<LiquidationRecord[]> {
    const actor = await this.getQueryActor();
    const arg = limit !== undefined ? [BigInt(limit)] : [];
    return await actor.get_liquidation_history(arg) as LiquidationRecord[];
  }

  async getPoolEvents(start: bigint, length: bigint): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_pool_events(start, length) as any[];
  }

  async getPoolEventCount(): Promise<bigint> {
    const actor = await this.getQueryActor();
    return await actor.get_pool_event_count() as bigint;
  }

  async checkPoolCapacity(tokenLedger: Principal, amount: bigint): Promise<boolean> {
    const actor = await this.getQueryActor();
    return await actor.check_pool_capacity(tokenLedger, amount) as boolean;
  }

  private async getMutationActor(): Promise<StabilityPoolNativeXrpActor> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    if (isOisyWallet() && wallet.principal) {
      const signerAgent = await getOisySignerAgent(wallet.principal);
      return createOisyActor(
        STABILITY_POOL_CANISTER_ID,
        canisterIDLs.stability_pool,
        signerAgent
      ) as StabilityPoolNativeXrpActor;
    }

    return await walletStore.getActor(
      STABILITY_POOL_CANISTER_ID,
      canisterIDLs.stability_pool
    ) as StabilityPoolNativeXrpActor;
  }

  // ── Mutations ──

  async deposit(
    tokenLedger: Principal,
    amount: bigint,
    context = captureStabilityPoolActionContext(tokenLedger, 'deposit'),
  ): Promise<void> {
    assertCurrentAction(context, tokenLedger, 'deposit');
    return withActionLock(context, () => withCrossTabDepositLock(context, async () => {
      assertCurrentAction(context, tokenLedger, 'deposit');
      const marker = readPendingStabilityPoolDeposit(context.principalText);
      if (marker && (marker.ledger !== context.ledgerText || marker.amount !== amount.toString())) {
        throw new PendingStabilityPoolDepositError(
          `Deposit intent ${marker.intentSeq} is locked for ledger ${marker.ledger} and ${marker.amount} raw units. Resume that exact intent before starting another deposit.`,
        );
      }

      let ledgerActor: any;
      let poolActor: any;
      if (context.oisy) {
        const signerAgent = await getOisySignerAgent(Principal.fromText(context.principalText));
        assertCurrentAction(context, tokenLedger, 'deposit');
        ledgerActor = createOisyActor(tokenLedger.toText(), CONFIG.icusd_ledgerIDL, signerAgent);
        poolActor = createOisyActor(STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool, signerAgent);
      } else {
        ledgerActor = await walletStore.getActor(tokenLedger.toText(), CONFIG.icusd_ledgerIDL) as any;
        assertCurrentAction(context, tokenLedger, 'deposit');
        poolActor = await walletStore.getActor(STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool) as any;
        assertCurrentAction(context, tokenLedger, 'deposit');
      }

      // An update status read lets a reload or another device recover the exact
      // caller-scoped sequence. Passing zero discovers any caller-wide active intent.
      let status = await poolActor.get_deposit_intent(BigInt(marker?.intentSeq ?? '0')) as DepositIntentStatus;
      assertCurrentAction(context, tokenLedger, 'deposit');
      let record = marker;
      const active = candidOption(status.active_intent);
      if (record && active) bindToAuthoritativePendingIntent(record, active, context);
      const known = candidOption(status.intent);
      if (record && known) {
        assertIntentPayload(record, known);
        const outcome = intentPayload(known);
        if (outcome.state === 'completed') {
          clearPendingStabilityPoolDeposit(record.owner, record.intentSeq);
          return;
        }
        if (outcome.state === 'no-effect') {
          clearPendingStabilityPoolDeposit(record.owner, record.intentSeq);
          throw new Error(outcome.reason ?? 'The Stability Pool confirmed that this deposit had no effect.');
        }
      }

      if (!record && active) {
        record = pendingRecord(context.principalText, active, context);
        savePendingStabilityPoolDeposit(record);
        if (record.ledger !== context.ledgerText || record.amount !== amount.toString()) {
          throw new PendingStabilityPoolDepositError(
            `Another device has pending deposit intent ${record.intentSeq} for ledger ${record.ledger} and ${record.amount} raw units. Resume that exact intent first.`,
          );
        }
      }

      if (record && !active && !known) {
        const nextSeq = candidOption(status.next_seq);
        if (nextSeq !== BigInt(record.intentSeq)) {
          throw new PendingStabilityPoolDepositError(
            'The canister no longer has this exact intent in retained status. Keep the local lock and contact support; do not allocate a new sequence.',
          );
        }
      }

      if (!record) {
        const nextSeq = candidOption(status.next_seq);
        if (active || nextSeq === undefined) {
          throw new PendingStabilityPoolDepositError('The Stability Pool has an unresolved deposit. Refresh status and resume that exact intent first.');
        }
        record = {
          owner: context.principalText,
          ledger: context.ledgerText,
          action: 'deposit',
          amount: amount.toString(),
          intentSeq: nextSeq.toString(),
          walletIcon: context.walletIcon,
          oisy: context.oisy,
          createdAt: Date.now(),
          status: 'pending',
        };
        savePendingStabilityPoolDeposit(record);
      }
      record = refreshPendingWalletContext(record, context);

      assertCurrentAction(context, tokenLedger, 'deposit');
      const approveResult = await ledgerActor.icrc2_approve({
        amount: amount * 105n / 100n,
        spender: { owner: Principal.fromText(STABILITY_POOL_CANISTER_ID), subaccount: [] },
        expires_at: [], expected_allowance: [], memo: [], fee: [],
        from_subaccount: [], created_at_time: []
      });
      assertCurrentAction(context, tokenLedger, 'deposit');
      if (approveResult && 'Err' in approveResult) {
        // Keep the exact intent marker. The next click reuses the same sequence
        // and payload after approval succeeds; approval itself cannot deposit.
        throw new Error(`Approval failed: ${JSON.stringify(approveResult.Err)}`);
      }

      if (!context.oisy) {
        await new Promise(r => setTimeout(r, 2000));
        assertCurrentAction(context, tokenLedger, 'deposit');
      }
      assertCurrentAction(context, tokenLedger, 'deposit');

      let result: { Ok: DepositIntentResult } | { Err: any };
      try {
        result = await poolActor.deposit_with_intent(BigInt(record.intentSeq), tokenLedger, amount) as { Ok: DepositIntentResult } | { Err: any };
        assertCurrentAction(context, tokenLedger, 'deposit');
      } catch (error) {
        if (context.oisy && isKnownSignerAbort(error)) {
          throw new Error('The wallet canceled before the Stability Pool intent was submitted. The same intent remains available to resume.');
        }
        // Status can prove a terminal result after a lost reply. A missing or
        // pending result never unlocks: replay uses this exact seq and payload.
        try {
          assertCurrentAction(context, tokenLedger, 'deposit');
          status = await poolActor.get_deposit_intent(BigInt(record.intentSeq)) as DepositIntentStatus;
          assertCurrentAction(context, tokenLedger, 'deposit');
          const recovered = candidOption(status.intent);
          const activeNow = candidOption(status.active_intent);
          if (activeNow) bindToAuthoritativePendingIntent(record, activeNow, context);
          if (recovered) {
            assertIntentPayload(record, recovered);
            const outcome = intentPayload(recovered);
            if (outcome.state === 'completed') {
              clearPendingStabilityPoolDeposit(record.owner, record.intentSeq);
              return;
            }
            if (outcome.state === 'no-effect') {
              clearPendingStabilityPoolDeposit(record.owner, record.intentSeq);
              throw new Error(outcome.reason ?? 'The Stability Pool confirmed that this deposit had no effect.');
            }
          }
        } catch (statusError) {
          if (statusError instanceof Error && !/reject|fetch|network|timeout|connection/i.test(statusError.message)) throw statusError;
        }
        throw new PendingStabilityPoolDepositError(
          `Deposit intent ${record.intentSeq} has no terminal update result yet. Retry only this exact ledger and amount to resume it.`,
        );
      }

      if (result && 'Ok' in result) {
        assertIntentPayload(record, result.Ok);
        const outcome = intentPayload(result.Ok);
        if (outcome.state === 'completed') {
          clearPendingStabilityPoolDeposit(record.owner, record.intentSeq);
          return;
        }
        if (outcome.state === 'no-effect') {
          clearPendingStabilityPoolDeposit(record.owner, record.intentSeq);
          throw new Error(outcome.reason ?? 'The Stability Pool confirmed that this deposit had no effect.');
        }
        throw new PendingStabilityPoolDepositError(
          `Deposit intent ${record.intentSeq} is still pending (${outcome.reason ?? 'reconciliation continues'}). Resume the same intent; do not start a new deposit.`,
        );
      }

      if (result && 'Err' in result) {
        // The update has returned a definite Candid error. Ask the same
        // authenticated canister for current intent state before unlocking.
        try {
          status = await poolActor.get_deposit_intent(BigInt(record.intentSeq)) as DepositIntentStatus;
          assertCurrentAction(context, tokenLedger, 'deposit');
          const recovered = candidOption(status.intent);
          const activeNow = candidOption(status.active_intent);
          if (activeNow) bindToAuthoritativePendingIntent(record, activeNow, context);
          if (recovered) {
            assertIntentPayload(record, recovered);
            const outcome = intentPayload(recovered);
            if (outcome.state === 'completed') {
              clearPendingStabilityPoolDeposit(record.owner, record.intentSeq);
              return;
            }
            if (outcome.state === 'no-effect') {
              clearPendingStabilityPoolDeposit(record.owner, record.intentSeq);
              throw new Error(outcome.reason ?? 'The Stability Pool confirmed that this deposit had no effect.');
            }
          } else if (!activeNow && candidOption(status.next_seq) === BigInt(record.intentSeq)) {
            clearPendingStabilityPoolDeposit(record.owner, record.intentSeq);
            throw new Error(this.formatError(result.Err));
          }
        } catch (statusError) {
          if (statusError instanceof Error && !/reject|fetch|network|timeout|connection/i.test(statusError.message)) throw statusError;
        }
        throw new PendingStabilityPoolDepositError(
          `The canister returned an error for intent ${record.intentSeq}, but its exact state is unresolved. Keep the lock and resume that same intent.`,
        );
      }

      throw new PendingStabilityPoolDepositError('The Stability Pool returned an unrecognized intent result. Keep the lock and resume only this exact intent.');
    }));
  }

  async withdraw(
    tokenLedger: Principal,
    amount: bigint,
    context = captureStabilityPoolActionContext(tokenLedger, 'withdraw'),
  ): Promise<void> {
    assertCurrentAction(context, tokenLedger, 'withdraw');
    return withActionLock(context, async () => {
      assertCurrentAction(context, tokenLedger, 'withdraw');

      if (context.oisy) {
        console.log('[Oisy] Sequential SP withdraw via @icp-sdk/signer v5');
        const signerAgent = await getOisySignerAgent(Principal.fromText(context.principalText));
        assertCurrentAction(context, tokenLedger, 'withdraw');
        const poolActor = createOisyActor(
          STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool, signerAgent
        );
        assertCurrentAction(context, tokenLedger, 'withdraw');
        const result = await poolActor.withdraw(tokenLedger, amount);
        assertCurrentAction(context, tokenLedger, 'withdraw');
        if ('Err' in result) {
          throw new Error(this.formatError(result.Err));
        }
      } else {
        const poolActor = await walletStore.getActor(
          STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool
        ) as any;
        assertCurrentAction(context, tokenLedger, 'withdraw');
        const result = await poolActor.withdraw(tokenLedger, amount) as { Ok: null } | { Err: any };
        assertCurrentAction(context, tokenLedger, 'withdraw');
        if ('Err' in result) {
          throw new Error(this.formatError(result.Err));
        }
      }
    });
  }

  async claimCollateral(collateralLedger: Principal): Promise<bigint> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    if (isOisyWallet() && wallet.principal) {
      console.log(`[Oisy] Sequential SP claim_collateral via @icp-sdk/signer v5`);
      const signerAgent = await getOisySignerAgent(wallet.principal);
      const poolActor = createOisyActor(
        STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool, signerAgent
      );
      const result = await poolActor.claim_collateral(collateralLedger);
      if ('Err' in result) {
        throw new Error(this.formatError(result.Err));
      }
      return result.Ok;
    } else {
      const poolActor = await walletStore.getActor(
        STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool
      ) as any;
      const result = await poolActor.claim_collateral(collateralLedger) as { Ok: bigint } | { Err: any };
      if ('Err' in result) {
        throw new Error(this.formatError(result.Err));
      }
      return result.Ok;
    }
  }

  async claimAllCollateral(): Promise<Array<[Principal, bigint]>> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    if (isOisyWallet() && wallet.principal) {
      console.log(`[Oisy] Sequential SP claim_all_collateral via @icp-sdk/signer v5`);
      const signerAgent = await getOisySignerAgent(wallet.principal);
      const poolActor = createOisyActor(
        STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool, signerAgent
      );
      const result = await poolActor.claim_all_collateral();
      if ('Err' in result) {
        throw new Error(this.formatError(result.Err));
      }
      return result.Ok;
    } else {
      const poolActor = await walletStore.getActor(
        STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool
      ) as any;
      const result = await poolActor.claim_all_collateral() as { Ok: Array<[Principal, bigint]> } | { Err: any };
      if ('Err' in result) {
        throw new Error(this.formatError(result.Err));
      }
      return result.Ok;
    }
  }

  async optOutCollateral(collateralType: Principal): Promise<void> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    if (isOisyWallet() && wallet.principal) {
      console.log(`[Oisy] Sequential SP opt_out_collateral via @icp-sdk/signer v5`);
      const signerAgent = await getOisySignerAgent(wallet.principal);
      const poolActor = createOisyActor(
        STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool, signerAgent
      );
      const result = await poolActor.opt_out_collateral(collateralType);
      if ('Err' in result) {
        throw new Error(this.formatError(result.Err));
      }
    } else {
      const poolActor = await walletStore.getActor(
        STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool
      ) as any;
      const result = await poolActor.opt_out_collateral(collateralType) as { Ok: null } | { Err: any };
      if ('Err' in result) {
        throw new Error(this.formatError(result.Err));
      }
    }
  }

  async optInCollateral(collateralType: Principal): Promise<void> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    if (isOisyWallet() && wallet.principal) {
      console.log(`[Oisy] Sequential SP opt_in_collateral via @icp-sdk/signer v5`);
      const signerAgent = await getOisySignerAgent(wallet.principal);
      const poolActor = createOisyActor(
        STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool, signerAgent
      );
      const result = await poolActor.opt_in_collateral(collateralType);
      if ('Err' in result) {
        throw new Error(this.formatError(result.Err));
      }
    } else {
      const poolActor = await walletStore.getActor(
        STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool
      ) as any;
      const result = await poolActor.opt_in_collateral(collateralType) as { Ok: null } | { Err: any };
      if ('Err' in result) {
        throw new Error(this.formatError(result.Err));
      }
    }
  }

  async optInNativeCollateral(collateralType: Principal, payoutAddress: string): Promise<void> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const address = payoutAddress.trim();
    if (!address) throw new Error('Enter an XRP address');

    if (isOisyWallet() && wallet.principal) {
      console.log(`[Oisy] Sequential SP opt_in_native_collateral via @icp-sdk/signer v5`);
      const signerAgent = await getOisySignerAgent(wallet.principal);
      const poolActor = createOisyActor(
        STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool, signerAgent
      );
      const result = await poolActor.opt_in_native_collateral(collateralType, address);
      if ('Err' in result) {
        throw new Error(this.formatError(result.Err));
      }
    } else {
      const poolActor = await walletStore.getActor(
        STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool
      ) as any;
      const result = await poolActor.opt_in_native_collateral(collateralType, address) as { Ok: null } | { Err: any };
      if ('Err' in result) {
        throw new Error(this.formatError(result.Err));
      }
    }
  }

  async optInNativeCollateralWithTag(
    collateralType: Principal,
    payoutAddress: string,
    destinationTag?: number
  ): Promise<void> {
    const actor = await this.getMutationActor();
    await optInNativeCollateralWithTagUsingActor(
      actor,
      collateralType,
      payoutAddress,
      destinationTag,
      (err) => this.formatError(err)
    );
  }

  async getMyNativeXrpPayouts(options: { allowSigner?: boolean } = {}): Promise<NativeXrpPendingPayout[]> {
    if (isOisyWallet() && !options.allowSigner) {
      return [];
    }

    const actor = await this.getMutationActor();
    return getMyNativeXrpPayoutsWithActor(actor);
  }

  async ackNativeXrpPayoutSettled(claimId: XrpClaimId | number | bigint): Promise<void> {
    const actor = await this.getMutationActor();
    await ackNativeXrpPayoutSettledWithActor(actor, claimId, (err) => this.formatError(err));
  }

  async executeLiquidation(vaultId: bigint): Promise<any> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    if (isOisyWallet() && wallet.principal) {
      console.log(`[Oisy] Sequential SP execute_liquidation via @icp-sdk/signer v5`);
      const signerAgent = await getOisySignerAgent(wallet.principal);
      const poolActor = createOisyActor(
        STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool, signerAgent
      );
      const result = await poolActor.execute_liquidation(vaultId);
      if ('Err' in result) {
        throw new Error(this.formatError(result.Err));
      }
      return result.Ok;
    } else {
      const poolActor = await walletStore.getActor(
        STABILITY_POOL_CANISTER_ID, canisterIDLs.stability_pool
      ) as any;
      const result = await poolActor.execute_liquidation(vaultId) as { Ok: any } | { Err: any };
      if ('Err' in result) {
        throw new Error(this.formatError(result.Err));
      }
      return result.Ok;
    }
  }

  // ── Error formatting ──

  private formatError(err: any): string {
    if ('DepositIntentCapacityReached' in err) {
      return 'Stability Pool deposit capacity is full (100,000 caller limit). No deposit was submitted; deposits cannot be retried until capacity changes. Existing withdrawals remain available.';
    }
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
    if ('PayoutAddressRequired' in err) return 'XRP requires a payout address';
    if ('InvalidPayoutAddress' in err) return `Invalid XRP address: ${err.InvalidPayoutAddress.reason}`;
    return 'Unknown error';
  }
}

export const stabilityPoolService = new StabilityPoolService();
