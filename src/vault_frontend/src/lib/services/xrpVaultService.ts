/**
 * Native-XRP collateral service (P5).
 *
 * Wraps the backend's XRP CDP endpoints behind clean, UI-friendly types. The flow a
 * native-XRP vault goes through:
 *
 *   1. openXrpVault()      -> reserves a vault id + derives a per-vault XRPL custody
 *                             address (threshold Ed25519). No collateral yet.
 *   2. user sends XRP to that custody address from any XRPL wallet (off-chain).
 *   3. confirmXrpDeposit() -> the protocol verifies the on-chain balance and credits
 *                             the vault; from here it is a normal CDP vault (borrow /
 *                             repay / withdraw use the generic vault endpoints).
 *   4. withdraw / close / liquidation produces an XrpClaim (XRP owed back out of the
 *      custody address). settleXrpClaim() signs + broadcasts the XRPL Payment.
 *
 * Mutations go through `ApiClient.executeSequentialOperation` (single in-flight
 * protocol op) and `callWithOisyFalseNegativeGuard` (Oisy signer false-negative
 * resilience), mirroring the ICP vault path in apiClient.ts. Reads use the
 * AUTHENTICATED actor because `get_my_xrp_*` filter by caller.
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

/** 1 XRP = 1,000,000 drops (XRPL native, 6 decimals). */
export const DROPS_PER_XRP = 1_000_000;

/** Convert drops (the on-wire integer unit) to a whole-XRP number for display. */
export function dropsToXrp(drops: bigint | number): number {
  return Number(drops) / DROPS_PER_XRP;
}

// ─── UI-friendly view types ──────────────────────────────────────────────────

export interface XrpVaultOpenView {
  /** Reserved vault id (also the threshold-derivation nonce). */
  vaultId: number;
  /** The per-vault XRPL classic custody address (starts with `r`). */
  custodyAddress: string;
  /** XRPL reserve base fetched by the backend when the custody address was prepared. */
  reserveBaseDrops: bigint;
}

export interface XrpPendingDepositView {
  vaultId: number;
  custodyAddress: string;
  openedAtMs: number;
  reserveBaseDrops: bigint;
}

export interface XrpClaimView {
  claimId: number;
  /** XRP owed to the claimant, in drops. */
  drops: bigint;
  /** XRP owed, as a whole-XRP number (display). */
  xrp: number;
  createdAtMs: number;
  /** True once a settlement Payment has been signed + submitted (awaiting confirm). */
  inFlight: boolean;
  /** The in-flight Payment's local tx hash, if any. */
  inFlightTxHash: string | null;
}

export interface XrpOpResult<T> {
  success: boolean;
  data?: T;
  error?: string;
  /** Set when the Oisy false-negative guard confirmed the op landed despite a signer error. */
  oisyResilient?: boolean;
}

interface CachedXrpPendingDeposit extends XrpPendingDepositView {
  updatedAtMs: number;
}

interface XrpReadOptions {
  /**
   * Oisy calls route through the popup signer. Passive UI refreshes must keep
   * this false; explicit user-triggered refreshes can opt in.
   */
  allowSigner?: boolean;
}

const XRP_PENDING_CACHE_PREFIX = 'rumi_xrp_pending_deposits:';
const XRP_HIDDEN_PENDING_PREFIX = 'rumi_xrp_hidden_pending_deposits:';
export const XRP_PENDING_DEPOSITS_CHANGED = 'rumi:xrp-pending-deposits-changed';

function currentPrincipalText(): string | null {
  return get(walletStore).principal?.toText?.() ?? null;
}

function isOisySignerWallet(): boolean {
  return get(currentWalletType) === WALLET_TYPES.OISY;
}

function pendingCacheKey(owner: string): string {
  return `${XRP_PENDING_CACHE_PREFIX}${owner}`;
}

function hiddenPendingCacheKey(owner: string): string {
  return `${XRP_HIDDEN_PENDING_PREFIX}${owner}`;
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
  if (browser) window.dispatchEvent(new CustomEvent(XRP_PENDING_DEPOSITS_CHANGED));
}

function visiblePendingDeposits(pending: XrpPendingDepositView[], owner = currentPrincipalText()): XrpPendingDepositView[] {
  const hidden = readHiddenPendingIds(owner);
  return pending.filter((p) => !hidden.has(p.vaultId));
}

function readCachedPendingDeposits(owner = currentPrincipalText()): XrpPendingDepositView[] {
  if (!browser || !owner) return [];
  try {
    const raw = localStorage.getItem(pendingCacheKey(owner));
    if (!raw) return [];
    const parsed = JSON.parse(raw) as CachedXrpPendingDeposit[];
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((p) => Number.isFinite(p.vaultId) && typeof p.custodyAddress === 'string')
      .map((p) => ({
        vaultId: p.vaultId,
        custodyAddress: p.custodyAddress,
        openedAtMs: Number.isFinite(p.openedAtMs) ? p.openedAtMs : p.updatedAtMs,
      }));
  } catch {
    return [];
  }
}

function writeCachedPendingDeposits(pending: XrpPendingDepositView[], owner = currentPrincipalText()) {
  if (!browser || !owner) return;
  const deduped = new Map<number, CachedXrpPendingDeposit>();
  for (const p of pending) {
    deduped.set(p.vaultId, {
      ...p,
      updatedAtMs: Date.now(),
    });
  }
  localStorage.setItem(pendingCacheKey(owner), JSON.stringify([...deduped.values()]));
}

function rememberPendingDeposit(pending: XrpPendingDepositView, owner = currentPrincipalText()) {
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

function normalizeXrpError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  const lower = message.toLowerCase();
  if (lower.includes('xrp account_info failed') && lower.includes('network is unreachable')) {
    return 'Could not reach the XRP Ledger to verify this deposit. Your XRP is not lost; wait a minute and try Confirm again.';
  }
  if (lower.includes('xrp account_info failed')) {
    return 'Could not verify the XRP deposit right now. The custody address is still yours; try Confirm again in a minute.';
  }
  if (lower.includes('xrp custody account is unfunded')) {
    return 'No XRP has reached this custody address yet. If you just sent it, wait for the XRPL transaction to settle and try again.';
  }
  return message;
}

// ─── Service ─────────────────────────────────────────────────────────────────

export class XrpVaultService {
  private static async actor(): Promise<_SERVICE> {
    return (await walletStore.getActor(CONFIG.currentCanisterId, rumi_backendIDL)) as _SERVICE;
  }

  /**
   * Open a native-XRP vault: reserves a vault id and returns the XRPL custody
   * address the user must fund. No collateral is credited and no icUSD is minted
   * until {@link confirmXrpDeposit}.
   */
  static async openXrpVault(): Promise<XrpOpResult<XrpVaultOpenView>> {
    return ApiClient.executeSequentialOperation(async () => {
      try {
        const actor = await this.actor();
        const result = await callWithOisyFalseNegativeGuard(
          () => actor.open_xrp_vault(),
          // Verifier: a fresh pending deposit now exists for this caller.
          async () => {
            const pending = await actor.get_my_xrp_pending_deposits();
            return pending.length > 0;
          },
          'open_xrp_vault'
        );
        // Oisy guard confirmed it landed but we don't have the reply: re-read the
        // newest pending deposit so the UI still gets the custody address.
        if (isOisyLandedSentinel(result)) {
          const pending = await actor.get_my_xrp_pending_deposits();
          if (pending.length === 0) {
            return { success: false, error: 'Vault opened but no pending deposit found; refresh and retry.' };
          }
          const [vaultId, dep] = pending[pending.length - 1];
          const view = {
            vaultId: Number(vaultId),
            custodyAddress: dep.custody_address,
            openedAtMs: Number(dep.opened_at_ns / 1_000_000n),
          };
          rememberPendingDeposit(view);
          return {
            success: true,
            oisyResilient: true,
            data: {
              vaultId: Number(vaultId),
              custodyAddress: dep.custody_address,
              reserveBaseDrops: dep.reserve_base_drops,
            },
          };
        }
        if ('Ok' in result) {
          rememberPendingDeposit({
            vaultId: Number(result.Ok.vault_id),
            custodyAddress: result.Ok.custody_address,
            openedAtMs: Date.now(),
          });
          return {
            success: true,
            data: {
              vaultId: Number(result.Ok.vault_id),
              custodyAddress: result.Ok.custody_address,
              reserveBaseDrops: result.Ok.reserve_base_drops,
            },
          };
        }
        return { success: false, error: normalizeXrpError(ApiClient.formatProtocolError(result.Err)) };
      } catch (e) {
        return { success: false, error: normalizeXrpError(e) };
      }
    });
  }

  /**
   * Verify the user's deposit landed on the custody address and credit the vault.
   * Returns the credited collateral in drops. NOT idempotent: a successful confirm
   * removes the pending deposit on the backend, so a repeat call errors with
   * "No pending XRP deposit for this vault". (The Oisy verifier below relies on this:
   * pending-deposit-gone === confirmed.)
   */
  static async confirmXrpDeposit(vaultId: number): Promise<XrpOpResult<{ creditedDrops: bigint }>> {
    return ApiClient.executeSequentialOperation(async () => {
      try {
        const actor = await this.actor();
        const result = await callWithOisyFalseNegativeGuard(
          () => actor.confirm_xrp_deposit(BigInt(vaultId)),
          // Verifier: the pending deposit for this vault is gone (it was confirmed).
          async () => {
            const pending = await actor.get_my_xrp_pending_deposits();
            return !pending.some(([id]) => Number(id) === vaultId);
          },
          `confirm_xrp_deposit #${vaultId}`
        );
        if (isOisyLandedSentinel(result)) {
          forgetPendingDeposit(vaultId);
          return { success: true, oisyResilient: true, data: { creditedDrops: 0n } };
        }
        if ('Ok' in result) {
          forgetPendingDeposit(vaultId);
          return { success: true, data: { creditedDrops: result.Ok } };
        }
        return { success: false, error: normalizeXrpError(ApiClient.formatProtocolError(result.Err)) };
      } catch (e) {
        return { success: false, error: normalizeXrpError(e) };
      }
    });
  }

  /**
   * Settle an XRP claim: sign + broadcast the XRPL Payment to `destination`. This is
   * a two-phase, anti-double-pay flow on the backend — the first call signs+submits
   * (the claim stays until the Payment validates), a follow-up call confirms it and
   * clears the claim. Pass `destinationTag` for exchange-hosted accounts that need
   * one; ordinary self-custody addresses continue through the legacy untagged
   * endpoint. The UI should call this again (or rely on polling) until the claim
   * disappears from {@link getMyClaims}. Returns the (local) tx hash.
   */
  static async settleXrpClaim(
    claimId: number,
    destination: string,
    destinationTag?: number
  ): Promise<XrpOpResult<{ txHash: string }>> {
    return ApiClient.executeSequentialOperation(async () => {
      try {
        if (
          destinationTag !== undefined &&
          (!Number.isInteger(destinationTag) || destinationTag < 0 || destinationTag > 0xffffffff)
        ) {
          return { success: false, error: 'Destination tag must be a whole number from 0 to 4294967295.' };
        }
        const actor = await this.actor();
        const operation =
          destinationTag === undefined
            ? () => actor.settle_xrp_claim(BigInt(claimId), destination)
            : () => actor.settle_xrp_claim_with_tag(BigInt(claimId), destination, destinationTag);
        const result = await callWithOisyFalseNegativeGuard(
          operation,
          // Verifier: the first settle phase records `claim.settlement` BEFORE the
          // submit outcall (backend vault.rs), so if the canister executed at all the
          // claim is now either gone (validated + removed) or carries a settlement.
          // Either way the Payment is on its way — treat it as landed so an Oisy
          // false-negative isn't reported as a hard failure.
          async () => {
            const claims = await actor.get_my_xrp_claims();
            const entry = claims.find(([id]) => Number(id) === claimId);
            return !entry || entry[1].settlement.length > 0;
          },
          destinationTag === undefined ? `settle_xrp_claim #${claimId}` : `settle_xrp_claim_with_tag #${claimId}`
        );
        if (isOisyLandedSentinel(result)) {
          return { success: true, oisyResilient: true, data: { txHash: '' } };
        }
        if ('Ok' in result) {
          return { success: true, data: { txHash: result.Ok } };
        }
        return { success: false, error: ApiClient.formatProtocolError(result.Err) };
      } catch (e) {
        return { success: false, error: e instanceof Error ? e.message : String(e) };
      }
    });
  }

  /** The caller's native-XRP vaults still awaiting their on-chain deposit. */
  static async getMyPendingDeposits(options: XrpReadOptions = {}): Promise<XrpPendingDepositView[]> {
    if (isOisySignerWallet() && !options.allowSigner) {
      return visiblePendingDeposits(readCachedPendingDeposits());
    }

    try {
      const actor = await this.actor();
      const pending = await actor.get_my_xrp_pending_deposits();
      const views = pending.map(([vaultId, dep]) => ({
        vaultId: Number(vaultId),
        custodyAddress: dep.custody_address,
        openedAtMs: Number(dep.opened_at_ns / 1_000_000n),
        reserveBaseDrops: dep.reserve_base_drops,
      }));
      writeCachedPendingDeposits(views);
      return visiblePendingDeposits(views);
    } catch (e) {
      console.error('getMyPendingDeposits failed:', e);
      return isOisySignerWallet() ? visiblePendingDeposits(readCachedPendingDeposits()) : [];
    }
  }

  static getHiddenPendingDeposits(): XrpPendingDepositView[] {
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

  /** The caller's outstanding native-XRP claims (XRP owed back to them). */
  static async getMyClaims(options: XrpReadOptions = {}): Promise<XrpClaimView[]> {
    if (isOisySignerWallet() && !options.allowSigner) {
      return [];
    }

    try {
      const actor = await this.actor();
      const claims = await actor.get_my_xrp_claims();
      return claims.map(([claimId, c]) => {
        const settlement = c.settlement.length > 0 ? c.settlement[0] : null;
        return {
          claimId: Number(claimId),
          drops: c.drops,
          xrp: dropsToXrp(c.drops),
          createdAtMs: Number(c.created_at_ns / 1_000_000n),
          inFlight: settlement !== null,
          inFlightTxHash: settlement ? settlement.tx_hash : null,
        };
      });
    } catch (e) {
      console.error('getMyClaims failed:', e);
      return [];
    }
  }
}
