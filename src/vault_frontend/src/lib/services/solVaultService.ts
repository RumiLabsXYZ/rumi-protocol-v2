/**
 * Native-SOL collateral service (P5).
 *
 * Wraps the backend's SOL CDP endpoints behind clean, UI-friendly types. The flow a
 * native-SOL vault goes through:
 *
 *   1. openSolVault()       -> reserves a vault id + derives a per-vault Solana
 *                              custody address (threshold Ed25519). No collateral yet.
 *   2. user sends SOL to that custody address from any Solana wallet (off-chain).
 *   3. confirmSolDeposit()  -> the protocol verifies the on-chain balance and credits
 *                              the vault; from here it is a normal CDP vault (borrow /
 *                              repay / withdraw use the generic vault endpoints).
 *   4. withdraw / close / liquidation produces a SolClaim (SOL owed back out of the
 *      custody address). settleSolClaim() signs + broadcasts the durable-nonce
 *      System Transfer. There is no destination-tag variant: Solana has no analogue
 *      (exchanges use unique deposit addresses per user), unlike XRP.
 *
 * Mutations go through `ApiClient.executeSequentialOperation` (single in-flight
 * protocol op) and `callWithOisyFalseNegativeGuard` (Oisy signer false-negative
 * resilience), mirroring the XRP and ICP vault paths. Reads use the AUTHENTICATED
 * actor because `get_my_sol_*` filter by caller.
 */

import type { _SERVICE } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did';
import { idlFactory as rumi_backendIDL } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
import { browser } from '$app/environment';
import { get } from 'svelte/store';
import { CONFIG } from '../config';
import { walletStore } from '../stores/wallet';
import { currentWalletType, WALLET_TYPES } from './auth';
import { ApiClient } from './protocol/apiClient';
import { callWithOisyFalseNegativeGuard, isOisyLandedSentinel } from './protocol/oisyResilience';
import { mapOptionalSolClaimId, solClaimIdToBigInt, type SolClaimId } from './solPayoutHelpers';

/** 1 SOL = 1,000,000,000 lamports (9 decimals). */
export const LAMPORTS_PER_SOL = 1_000_000_000;

/** Convert lamports (the on-wire integer unit) to a whole-SOL number for display. */
export function lamportsToSol(lamports: bigint | number): number {
  return Number(lamports) / LAMPORTS_PER_SOL;
}

// ─── UI-friendly view types ──────────────────────────────────────────────────

export interface SolVaultOpenView {
  /** Reserved vault id (also the threshold-derivation nonce). */
  vaultId: number;
  /** The per-vault Solana custody address (base58 pubkey). */
  custodyAddress: string;
  /** Rent-exempt minimum for a 0-byte system account, fetched live by the backend. */
  rentExemptLamports: bigint;
}

export interface SolPendingDepositView {
  vaultId: number;
  custodyAddress: string;
  openedAtMs: number;
  rentExemptLamports: bigint;
}

export interface SolClaimView {
  claimId: SolClaimId;
  /** SOL owed to the claimant, in lamports. */
  lamports: bigint;
  /** SOL owed, as a whole-SOL number (display). */
  sol: number;
  createdAtMs: number;
  /** Custody nonce used by native-SOL vault claims; matches vault id for manual recovery. */
  custodyNonce: number;
  vaultId: number;
  /** True once a settlement transfer has been signed + submitted (awaiting confirm). */
  inFlight: boolean;
  /** The in-flight transfer's local base58 signature, if any. */
  inFlightSignature: string | null;
  quarantineReason: string | null;
}

export interface SolOpResult<T> {
  success: boolean;
  data?: T;
  error?: string;
  /** Set when the Oisy false-negative guard confirmed the op landed despite a signer error. */
  oisyResilient?: boolean;
}

interface CachedSolPendingDeposit extends SolPendingDepositView {
  updatedAtMs: number;
}

interface SolReadOptions {
  /**
   * Oisy calls route through the popup signer. Passive UI refreshes must keep
   * this false; explicit user-triggered refreshes can opt in.
   */
  allowSigner?: boolean;
}

const SOL_PENDING_CACHE_PREFIX = 'rumi_sol_pending_deposits:';
const SOL_HIDDEN_PENDING_PREFIX = 'rumi_sol_hidden_pending_deposits:';
export const SOL_PENDING_DEPOSITS_CHANGED = 'rumi:sol-pending-deposits-changed';

function currentPrincipalText(): string | null {
  return get(walletStore).principal?.toText?.() ?? null;
}

function isOisySignerWallet(): boolean {
  return get(currentWalletType) === WALLET_TYPES.OISY;
}

function pendingCacheKey(owner: string): string {
  return `${SOL_PENDING_CACHE_PREFIX}${owner}`;
}

function hiddenPendingCacheKey(owner: string): string {
  return `${SOL_HIDDEN_PENDING_PREFIX}${owner}`;
}

function readHiddenPendingIds(owner = currentPrincipalText()): Set<number> {
  if (!browser || !owner) return new Set();
  try {
    const raw = localStorage.getItem(hiddenPendingCacheKey(owner));
    if (!raw) return new Set();
    const parsed = JSON.parse(raw) as number[];
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((id) => Number.isFinite(id)));
  } catch {
    return new Set();
  }
}

function writeHiddenPendingIds(ids: Set<number>, owner = currentPrincipalText()) {
  if (!browser || !owner) return;
  localStorage.setItem(hiddenPendingCacheKey(owner), JSON.stringify([...ids].sort((a, b) => a - b)));
}

function emitPendingDepositsChanged() {
  if (browser) window.dispatchEvent(new CustomEvent(SOL_PENDING_DEPOSITS_CHANGED));
}

function visiblePendingDeposits(pending: SolPendingDepositView[], owner = currentPrincipalText()): SolPendingDepositView[] {
  const hidden = readHiddenPendingIds(owner);
  return pending.filter((p) => !hidden.has(p.vaultId));
}

function readCachedPendingDeposits(owner = currentPrincipalText()): SolPendingDepositView[] {
  if (!browser || !owner) return [];
  try {
    const raw = localStorage.getItem(pendingCacheKey(owner));
    if (!raw) return [];
    const parsed = JSON.parse(raw) as CachedSolPendingDeposit[];
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((p) => Number.isFinite(p.vaultId) && typeof p.custodyAddress === 'string')
      .map((p) => ({
        vaultId: p.vaultId,
        custodyAddress: p.custodyAddress,
        openedAtMs: Number.isFinite(p.openedAtMs) ? p.openedAtMs : p.updatedAtMs,
        rentExemptLamports: p.rentExemptLamports ?? 0n,
      }));
  } catch {
    return [];
  }
}

function writeCachedPendingDeposits(pending: SolPendingDepositView[], owner = currentPrincipalText()) {
  if (!browser || !owner) return;
  const deduped = new Map<number, CachedSolPendingDeposit>();
  for (const p of pending) {
    deduped.set(p.vaultId, {
      ...p,
      updatedAtMs: Date.now(),
    });
  }
  localStorage.setItem(pendingCacheKey(owner), JSON.stringify([...deduped.values()]));
}

function rememberPendingDeposit(pending: SolPendingDepositView, owner = currentPrincipalText()) {
  const existing = readCachedPendingDeposits(owner).filter((p) => p.vaultId !== pending.vaultId);
  writeCachedPendingDeposits([...existing, pending], owner);
  const hidden = readHiddenPendingIds(owner);
  hidden.delete(pending.vaultId);
  writeHiddenPendingIds(hidden, owner);
  emitPendingDepositsChanged();
}

function forgetPendingDeposit(vaultId: number, owner = currentPrincipalText()) {
  writeCachedPendingDeposits(
    readCachedPendingDeposits(owner).filter((p) => p.vaultId !== vaultId),
    owner
  );
  const hidden = readHiddenPendingIds(owner);
  hidden.delete(vaultId);
  writeHiddenPendingIds(hidden, owner);
  emitPendingDepositsChanged();
}

function normalizeSolError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  const lower = message.toLowerCase();
  if (lower.includes('sol') && (lower.includes('unreachable') || lower.includes('rpc'))) {
    return 'Could not reach the Solana network to verify this deposit. Your SOL is not lost; wait a minute and try Confirm again.';
  }
  if (lower.includes('unfunded')) {
    return 'No SOL has reached this custody address yet. If you just sent it, wait for the Solana transaction to finalize and try again.';
  }
  return message;
}

// ─── Service ─────────────────────────────────────────────────────────────────

export class SolVaultService {
  private static async actor(): Promise<_SERVICE> {
    return (await walletStore.getActor(CONFIG.currentCanisterId, rumi_backendIDL)) as _SERVICE;
  }

  /**
   * Open a native-SOL vault: reserves a vault id and returns the Solana custody
   * address the user must fund. No collateral is credited and no icUSD is minted
   * until {@link confirmSolDeposit}.
   */
  static async openSolVault(): Promise<SolOpResult<SolVaultOpenView>> {
    return ApiClient.executeSequentialOperation(async () => {
      try {
        const actor = await this.actor();
        const result = await callWithOisyFalseNegativeGuard(
          () => actor.open_sol_vault(),
          // Verifier: a fresh pending deposit now exists for this caller.
          async () => {
            const pending = await actor.get_my_sol_pending_deposits();
            return pending.length > 0;
          },
          'open_sol_vault'
        );
        // Oisy guard confirmed it landed but we don't have the reply: re-read the
        // newest pending deposit so the UI still gets the custody address.
        if (isOisyLandedSentinel(result)) {
          const pending = await actor.get_my_sol_pending_deposits();
          if (pending.length === 0) {
            return { success: false, error: 'Vault opened but no pending deposit found; refresh and retry.' };
          }
          const [vaultId, dep] = pending[pending.length - 1];
          const view = {
            vaultId: Number(vaultId),
            custodyAddress: dep.custody_address,
            openedAtMs: Number(dep.opened_at_ns / 1_000_000n),
            rentExemptLamports: dep.rent_exempt_lamports,
          };
          rememberPendingDeposit(view);
          return {
            success: true,
            oisyResilient: true,
            data: {
              vaultId: Number(vaultId),
              custodyAddress: dep.custody_address,
              rentExemptLamports: dep.rent_exempt_lamports,
            },
          };
        }
        if ('Ok' in result) {
          rememberPendingDeposit({
            vaultId: Number(result.Ok.vault_id),
            custodyAddress: result.Ok.custody_address,
            openedAtMs: Date.now(),
            rentExemptLamports: result.Ok.rent_exempt_lamports,
          });
          return {
            success: true,
            data: {
              vaultId: Number(result.Ok.vault_id),
              custodyAddress: result.Ok.custody_address,
              rentExemptLamports: result.Ok.rent_exempt_lamports,
            },
          };
        }
        return { success: false, error: normalizeSolError(ApiClient.formatProtocolError(result.Err)) };
      } catch (e) {
        return { success: false, error: normalizeSolError(e) };
      }
    });
  }

  /**
   * Verify the user's deposit landed on the custody address and credit the vault.
   * Returns the credited collateral in lamports. NOT idempotent: a successful confirm
   * removes the pending deposit on the backend, so a repeat call errors with
   * "No pending SOL deposit for this vault". (The Oisy verifier below relies on this:
   * pending-deposit-gone === confirmed.)
   */
  static async confirmSolDeposit(vaultId: number): Promise<SolOpResult<{ creditedLamports: bigint }>> {
    return ApiClient.executeSequentialOperation(async () => {
      try {
        const actor = await this.actor();
        const result = await callWithOisyFalseNegativeGuard(
          () => actor.confirm_sol_deposit(BigInt(vaultId)),
          // Verifier: the pending deposit for this vault is gone (it was confirmed).
          async () => {
            const pending = await actor.get_my_sol_pending_deposits();
            return !pending.some(([id]) => Number(id) === vaultId);
          },
          `confirm_sol_deposit #${vaultId}`
        );
        if (isOisyLandedSentinel(result)) {
          forgetPendingDeposit(vaultId);
          return { success: true, oisyResilient: true, data: { creditedLamports: 0n } };
        }
        if ('Ok' in result) {
          forgetPendingDeposit(vaultId);
          return { success: true, data: { creditedLamports: result.Ok } };
        }
        return { success: false, error: normalizeSolError(ApiClient.formatProtocolError(result.Err)) };
      } catch (e) {
        return { success: false, error: normalizeSolError(e) };
      }
    });
  }

  /**
   * Settle a SOL claim: sign + broadcast the durable-nonce System Transfer to
   * `destination`. No destination-tag parameter (Solana has no analogue). This is
   * a two-phase, anti-double-pay flow on the backend (§5.2-5.3 of the design spec):
   * the first call signs+submits (the claim stays until the transfer confirms), a
   * follow-up call confirms it and clears the claim. The UI should call this again
   * (or rely on polling) until the claim disappears from {@link getMyClaims}.
   * Returns the (local) base58 tx signature.
   */
  static async settleSolClaim(
    claimId: SolClaimId | number | bigint,
    destination: string
  ): Promise<SolOpResult<{ signature: string }>> {
    return ApiClient.executeSequentialOperation(async () => {
      try {
        const claimIdBig = solClaimIdToBigInt(claimId);
        const claimIdText = claimIdBig.toString();
        const actor = await this.actor();
        const result = await callWithOisyFalseNegativeGuard(
          () => actor.settle_sol_claim(claimIdBig, destination),
          // Verifier: the first settle phase persists `claim.settlement` BEFORE the
          // submit outcall (design spec §5.2 step 8, mirroring XRP's vault.rs), so
          // if the canister executed at all the claim is now either gone (validated
          // + removed) or carries a settlement. Either way the transfer is on its
          // way, so treat it as landed, meaning an Oisy false-negative isn't reported
          // as a hard failure.
          async () => {
            const claims = await actor.get_my_sol_claims();
            const entry = claims.find(([id]) => id.toString() === claimIdText);
            return !entry || entry[1].settlement.length > 0;
          },
          `settle_sol_claim #${claimIdText}`
        );
        if (isOisyLandedSentinel(result)) {
          return { success: true, oisyResilient: true, data: { signature: '' } };
        }
        if ('Ok' in result) {
          return { success: true, data: { signature: result.Ok } };
        }
        return { success: false, error: ApiClient.formatProtocolError(result.Err) };
      } catch (e) {
        return { success: false, error: e instanceof Error ? e.message : String(e) };
      }
    });
  }

  static async hasOutstandingClaim(claimId: SolClaimId | number | bigint): Promise<boolean> {
    const claimIdText = solClaimIdToBigInt(claimId).toString();
    const actor = await this.actor();
    const claims = await actor.get_my_sol_claims();
    return claims.some(([id]) => id.toString() === claimIdText);
  }

  /** The caller's native-SOL vaults still awaiting their on-chain deposit. */
  static async getMyPendingDeposits(options: SolReadOptions = {}): Promise<SolPendingDepositView[]> {
    if (isOisySignerWallet() && !options.allowSigner) {
      return visiblePendingDeposits(readCachedPendingDeposits());
    }

    try {
      const actor = await this.actor();
      const pending = await actor.get_my_sol_pending_deposits();
      const views = pending.map(([vaultId, dep]) => ({
        vaultId: Number(vaultId),
        custodyAddress: dep.custody_address,
        openedAtMs: Number(dep.opened_at_ns / 1_000_000n),
        rentExemptLamports: dep.rent_exempt_lamports,
      }));
      writeCachedPendingDeposits(views);
      return visiblePendingDeposits(views);
    } catch (e) {
      console.error('getMyPendingDeposits (SOL) failed:', e);
      return isOisySignerWallet() ? visiblePendingDeposits(readCachedPendingDeposits()) : [];
    }
  }

  static getHiddenPendingDeposits(): SolPendingDepositView[] {
    const hidden = readHiddenPendingIds();
    return readCachedPendingDeposits().filter((p) => hidden.has(p.vaultId));
  }

  static hidePendingDeposit(vaultId: number) {
    const hidden = readHiddenPendingIds();
    hidden.add(vaultId);
    writeHiddenPendingIds(hidden);
    emitPendingDepositsChanged();
  }

  static restorePendingDeposit(vaultId: number) {
    const hidden = readHiddenPendingIds();
    hidden.delete(vaultId);
    writeHiddenPendingIds(hidden);
    emitPendingDepositsChanged();
  }

  /** The caller's outstanding native-SOL claims (SOL owed back to them). */
  static async getMyClaims(options: SolReadOptions = {}): Promise<SolClaimView[]> {
    if (isOisySignerWallet() && !options.allowSigner) {
      return [];
    }

    try {
      const actor = await this.actor();
      const claims = await actor.get_my_sol_claims();
      return claims.map(([claimId, c]) => {
        const settlement = c.settlement.length > 0 ? c.settlement[0] : null;
        const custodyNonce = Number(c.custody_nonce);
        return {
          claimId: mapOptionalSolClaimId([claimId]) ?? claimId.toString(),
          lamports: c.lamports,
          sol: lamportsToSol(c.lamports),
          createdAtMs: Number(c.created_at_ns / 1_000_000n),
          custodyNonce,
          vaultId: custodyNonce,
          inFlight: settlement !== null,
          inFlightSignature: settlement ? settlement.signature : null,
          quarantineReason: c.quarantine_reason.length > 0 ? c.quarantine_reason[0] ?? null : null,
        };
      });
    } catch (e) {
      console.error('getMyClaims (SOL) failed:', e);
      return [];
    }
  }
}
