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
import { CONFIG } from '../config';
import { walletStore } from '../stores/wallet';
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
}

export interface XrpPendingDepositView {
  vaultId: number;
  custodyAddress: string;
  openedAtMs: number;
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
          return {
            success: true,
            oisyResilient: true,
            data: { vaultId: Number(vaultId), custodyAddress: dep.custody_address },
          };
        }
        if ('Ok' in result) {
          return {
            success: true,
            data: { vaultId: Number(result.Ok.vault_id), custodyAddress: result.Ok.custody_address },
          };
        }
        return { success: false, error: ApiClient.formatProtocolError(result.Err) };
      } catch (e) {
        return { success: false, error: e instanceof Error ? e.message : String(e) };
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
          return { success: true, oisyResilient: true, data: { creditedDrops: 0n } };
        }
        if ('Ok' in result) {
          return { success: true, data: { creditedDrops: result.Ok } };
        }
        return { success: false, error: ApiClient.formatProtocolError(result.Err) };
      } catch (e) {
        return { success: false, error: e instanceof Error ? e.message : String(e) };
      }
    });
  }

  /**
   * Settle an XRP claim: sign + broadcast the XRPL Payment to `destination`. This is
   * a two-phase, anti-double-pay flow on the backend — the first call signs+submits
   * (the claim stays until the Payment validates), a follow-up call confirms it and
   * clears the claim. The UI should call this again (or rely on polling) until the
   * claim disappears from {@link getMyClaims}. Returns the (local) tx hash.
   */
  static async settleXrpClaim(claimId: number, destination: string): Promise<XrpOpResult<{ txHash: string }>> {
    return ApiClient.executeSequentialOperation(async () => {
      try {
        const actor = await this.actor();
        const result = await callWithOisyFalseNegativeGuard(
          () => actor.settle_xrp_claim(BigInt(claimId), destination),
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
          `settle_xrp_claim #${claimId}`
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
  static async getMyPendingDeposits(): Promise<XrpPendingDepositView[]> {
    try {
      const actor = await this.actor();
      const pending = await actor.get_my_xrp_pending_deposits();
      return pending.map(([vaultId, dep]) => ({
        vaultId: Number(vaultId),
        custodyAddress: dep.custody_address,
        openedAtMs: Number(dep.opened_at_ns / 1_000_000n),
      }));
    } catch (e) {
      console.error('getMyPendingDeposits failed:', e);
      return [];
    }
  }

  /** The caller's outstanding native-XRP claims (XRP owed back to them). */
  static async getMyClaims(): Promise<XrpClaimView[]> {
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
