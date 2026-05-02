/**
 * Live ICRC-1 ledger transfer fee lookup with per-session caching.
 *
 * Audit ICRC-005 (frontend half): the previous design hardcoded fees in
 * `getLedgerFee()` switches across the codebase, which silently drifted
 * out of sync any time a ledger bumped its fee on chain. This service
 * queries `icrc1_fee()` directly and caches per-ledger for the session,
 * so a fee bump propagates without a frontend redeploy.
 *
 * `fetchLedgerFee` is async + cached. `getCachedLedgerFee` is the sync
 * accessor for batched signer flows that can't await mid-batch — pair it
 * with an upstream `await fetchLedgerFee(...)` to ensure the cache is
 * warm before the batch runs.
 *
 * Failure mode: on query error the helper logs a warning and falls back
 * to a value derived from the token's decimals/symbol. Failures are NOT
 * cached, so the next call re-tries. If the fallback diverges from the
 * actual on-chain fee, the ICRC-1 ledger will surface BadFee on transfer
 * — the user sees the error there rather than this code silently
 * undercharging or overcharging.
 */

import { Actor, HttpAgent, AnonymousIdentity } from '@dfinity/agent';
import { CONFIG } from '../config';
import { ICRC1_IDL } from '../idls/ledger.idl.js';

const TTL_MS = 5 * 60 * 1000; // 5-minute cache window

const FALLBACK_FEE_BY_DECIMALS: Record<number, bigint> = {
  6: 10_000n,   // ckUSDC, ckUSDT
  8: 100_000n,  // icUSD, 3USD
};

const ICP_FALLBACK_FEE = 10_000n;

const DEFAULT_FALLBACK_FEE = 10_000n;

interface CacheEntry {
  fee: bigint;
  fetchedAt: number;
}

const cache = new Map<string, CacheEntry>();
let _anonAgent: HttpAgent | null = null;

export interface LedgerFeeRef {
  /** ICRC-1 ledger canister ID (text form). */
  ledgerId: string;
  /** Token decimals — used for fallback selection if the live query fails. */
  decimals?: number;
  /** Token symbol — used for fallback selection (ICP gets a different default than 8-decimal stables). */
  symbol?: string;
}

async function getAnonAgent(): Promise<HttpAgent> {
  if (!_anonAgent) {
    _anonAgent = new HttpAgent({
      host: CONFIG.host,
      identity: new AnonymousIdentity(),
    });
    if (CONFIG.isLocal) {
      await _anonAgent.fetchRootKey();
    }
  }
  return _anonAgent;
}

function fallbackFee(ref: LedgerFeeRef): bigint {
  if (ref.symbol === 'ICP') return ICP_FALLBACK_FEE;
  if (ref.decimals !== undefined) {
    const f = FALLBACK_FEE_BY_DECIMALS[ref.decimals];
    if (f !== undefined) return f;
  }
  return DEFAULT_FALLBACK_FEE;
}

/**
 * Query the ICRC-1 transfer fee for a ledger, returning a cached value when fresh.
 * On error, logs a warning and returns a fallback derived from the token metadata.
 * Failures are NOT cached — the next call retries.
 */
export async function fetchLedgerFee(ref: LedgerFeeRef): Promise<bigint> {
  const cached = cache.get(ref.ledgerId);
  const now = Date.now();
  if (cached && now - cached.fetchedAt < TTL_MS) {
    return cached.fee;
  }

  try {
    const agent = await getAnonAgent();
    const actor = Actor.createActor(ICRC1_IDL as any, {
      agent,
      canisterId: ref.ledgerId,
    });
    const fee = (await (actor as any).icrc1_fee()) as bigint;
    cache.set(ref.ledgerId, { fee, fetchedAt: now });
    return fee;
  } catch (err) {
    console.warn(
      `[ledgerFeeService] icrc1_fee query failed for ${ref.ledgerId}; using fallback. ` +
        'The displayed transfer fee may be stale until the ledger responds again.',
      err,
    );
    return fallbackFee(ref);
  }
}

/**
 * Synchronous accessor — returns the cached fee if present, else the fallback.
 * Use only after `fetchLedgerFee` has been awaited at least once for this ledger,
 * or in code paths where falling back to the hardcoded default is acceptable.
 */
export function getCachedLedgerFee(ref: LedgerFeeRef): bigint {
  const cached = cache.get(ref.ledgerId);
  if (cached) return cached.fee;
  return fallbackFee(ref);
}

/** Test hook. Resets the cache so tests are deterministic. */
export function _clearLedgerFeeCache(): void {
  cache.clear();
}
