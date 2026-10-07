import { Actor, HttpAgent } from '@dfinity/agent';
import { Principal } from '@dfinity/principal';
import { BigIntUtils } from '../../utils/bigintUtils';
import { friendlyBorrowCapError } from '../../utils/borrowLimits';
import { idlFactory as rumi_backendIDL } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
import { idlFactory as icp_ledgerIDL } from '$declarations/icp_ledger/icp_ledger.did.js';
import { idlFactory as icusd_ledgerIDL } from '$declarations/icusd_ledger/icusd_ledger.did.js';
import { idlFactory as treasuryIDL } from '$declarations/rumi_treasury/rumi_treasury.did.js';
import { CANISTER_IDS, CONFIG, LOCAL_CANISTER_IDS  } from '../../config';
import { walletStore } from '../../stores/wallet';
import type {
    _SERVICE,
    Vault as CanisterVault,
    ProtocolStatus as CanisterProtocolStatus,
    LiquidityStatus as CanisterLiquidityStatus,
    Fees,
    SuccessWithFee,
    ProtocolError,
    OpenVaultSuccess,
    InboundCollateralRequestState,
    InboundCollateralStatusView,
    RepaymentV2RequestState,
    RepaymentV2StatusView,
    StableRepaymentV2RequestState,
    StableRepaymentV2StatusView,
    LiquidityV2RequestState,
    LiquidityV2StatusView,
  } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
import {
  walletOperations,
  isOisyWallet,
  largeApprovalExpiry,
  assertActionBoundContextCurrent,
  StaleActionSessionError,
  type ActionBoundContext,
} from './walletOperations';
import { pnp } from '../pnp';
import { get } from 'svelte/store';
import { QueryOperations } from './queryOperations';
import { currentWalletType, walletSessionGeneration, WALLET_TYPES } from '../auth';
import { permissionManager } from '../PermissionManager';
import type {
  VaultDTO,
  VaultOperationResult,
  FeesDTO,
  LiquidityStatusDTO,
  CandidVault
} from '../types';
import type {
  RedemptionQueue,
  RedemptionPreview,
  RedemptionQuoteResult,
  RedemptionOfferRefreshResult,
  RedemptionQuotedRequest,
  RedemptionResultVariant,
  AcceptedRedemptionOffer,
} from '$lib/utils/redemptionPreview';
import { acceptedRedemptionOfferTermsAreCurrent } from '$lib/utils/redemptionPreview';
import { RequestDeduplicator } from '../RequestDeduplicator';
import { collateralStore } from '$lib/stores/collateralStore';
import {
  callWithOisyFalseNegativeGuard,
  isOisyArrFalseNegative,
  isOisyLandedSentinel,
} from './oisyResilience';
import {
  warmRawSnapshots,
  warmRawSnapshot,
  warmRawVaultIds,
  getRawSnapshot,
  getRawVaultIds,
  clearRawSnapshots,
} from './rawSnapshotCache';
import { TokenService } from '../tokenService';
import { mapLiquidationSuccessWithFee } from '../xrpPayoutHelpers';
import {
  repaymentV2Disposition,
  repaymentV2StatusMatchesIntent,
  repaymentV2TransportOutcome,
  type RepaymentV2Intent,
} from '$lib/utils/repaymentV2Intent';
import { stableRepaymentV2Outcome, stableRepaymentV2RequiredAllowance, stableRepaymentV2TransportOutcome, unwrapStableRepaymentV2Status } from '$lib/utils/stableRepaymentV2';
import { liquidityV2ClaimIdentityMatches, liquidityV2Disposition, liquidityV2MayAdoptClaimAmount, liquidityV2StatusHasOwner, liquidityV2StatusMatchesIntent, type LiquidityV2Intent, type LiquidityV2Operation } from '$lib/utils/liquidityV2Intent';
import { fetchLedgerFee } from '../ledgerFeeService';

type StableRepaymentV2Token = 'CKUSDT' | 'CKUSDC';
interface StableRepaymentV2Intent {
  version: 2;
  owner: string;
  network: string;
  requestId: string;
  vaultId: string;
  token: StableRepaymentV2Token;
  amountRawE6: string;
  approvalAttempted: boolean;
  backendDispatchAttempted: boolean;
}
export interface BoundLiquidityV2Result {
  kind: BoundActionOutcomeKind;
  status: LiquidityV2StatusView | null;
  errorMessage: string | null;
  approvalMayHaveMutated: boolean;
  amountAdopted?: boolean;
}



// Constants from backend
export const E8S = 100_000_000;
export let MIN_ICUSD_AMOUNT = 10_000_000; // 0.10 icUSD default; updated from protocol status
const REDEMPTION_INGRESS_PAUSED = 'Redemption submissions are paused until transfer recovery is available. No icUSD was approved or submitted.';
function redemptionIngressIsPaused(): boolean {
  return true;
}

/** Update the minimum icUSD amount from protocol status (called after fetching status). */
export function updateMinIcusdAmount(amountE8s: number) {
  if (amountE8s > 0) MIN_ICUSD_AMOUNT = amountE8s;
}
// CRITICAL CHANGE: Set this to false to use real data from the backend
export const USE_MOCK_DATA = false;

// Create anonymous agent using CONFIG values
const anonymousAgent = new HttpAgent({ host: CONFIG.host });
if (CONFIG.isLocal) {
  // Only fetch root key in local development to avoid warnings
  anonymousAgent.fetchRootKey().catch(err => {
    console.error("Failed to fetch root key:", err);
  });
}

// Create anonymous actor for public endpoints that don't require authentication
export const publicActor = Actor.createActor<_SERVICE>(rumi_backendIDL as any, {
  agent: anonymousAgent,
  canisterId: CONFIG.currentCanisterId
});

// Re-exported so callers can import the action-bound primitives from apiClient
// without also reaching into walletOperations directly.
export type { ActionBoundContext };
export { StaleActionSessionError };

/**
 * Typed provenance for a bound (identity- and session-pinned) mutating action.
 *
 *  - predispatch_aborted: the top-level mutating call (open_vault_and_borrow /
 *    borrow_from_vault) was never dispatched — stale session, validation
 *    failure, or an approval failure. This does NOT mean nothing at all was
 *    submitted: see BoundOpenVaultAndBorrowResult.approvalMayHaveMutated.
 *  - dispatched_ok: an actual typed Ok response was received from the backend.
 *  - dispatched_err: an actual typed Err response was received from the
 *    backend — the call reached the canister and was rejected there.
 *  - ambiguous_transport: the call was dispatched but no typed backend result
 *    was ever observed (thrown network/timeout error, lost reply, or the
 *    Oisy `_arr` false-negative pattern). This is NEVER treated as proof of
 *    no mutation — unlike the legacy openVaultAndBorrow/borrowFromVault, the
 *    bound methods do not run an on-chain landed-heuristic to upgrade this to
 *    a success, because a heuristic vault-scan match is not provably
 *    attributable to this specific attempt (see extractPartialZeroDebtVaultId
 *    for the one exception: a source-proven typed Err string).
 */
export type BoundActionOutcomeKind =
  | 'predispatch_aborted'
  | 'dispatched_ok'
  | 'dispatched_err'
  | 'ambiguous_transport';

export interface BoundOpenVaultAndBorrowResult {
  kind: BoundActionOutcomeKind;
  /** Authoritative vault id. ONLY set on dispatched_ok from an actual typed Ok response. */
  vaultId: number | null;
  blockIndex: number | null;
  /**
   * Set only when kind === 'dispatched_err' AND the backend's GenericError text
   * proves the vault.rs partial-failure shape ("Vault created (id=N)..." — vault
   * created, borrow sub-step failed, vault now exists with zero debt). null in
   * every other case, including ambiguous_transport (a thrown error never
   * proves a vault was created).
   */
  partialZeroDebtVaultId: number | null;
  errorMessage: string | null;
  /**
   * True if execution reached (or passed) the ICRC-2 approval dispatch before
   * this result was produced. When true AND kind === 'predispatch_aborted',
   * the allowance may already have been mutated on-chain even though the
   * top-level open_vault_and_borrow call was never dispatched — callers must
   * not tell the user "nothing was submitted" in that case.
   */
  approvalMayHaveMutated: boolean;
  /** Exact raw wire amounts actually submitted, regardless of outcome. */
  submittedCollateralRaw: bigint;
  submittedIcusdRaw: bigint;
}

export interface BoundBorrowFromVaultResult {
  kind: BoundActionOutcomeKind;
  vaultId: number;
  blockIndex: number | null;
  feePaidRaw: bigint | null;
  errorMessage: string | null;
  submittedIcusdRaw: bigint;
}

/** Result of opening collateral through the request-ID-backed ingress journal. */
export interface BoundOpenVaultV2Result {
  kind: BoundActionOutcomeKind;
  status: InboundCollateralStatusView | null;
  errorMessage: string | null;
  approvalMayHaveMutated: boolean;
}

/** Result of adding collateral through the durable inbound collateral journal. */
export interface BoundAddMarginV2Result {
  kind: BoundActionOutcomeKind;
  status: InboundCollateralStatusView | null;
  errorMessage: string | null;
  approvalMayHaveMutated: boolean;
}

export interface BoundRepaymentV2Result {
  kind: BoundActionOutcomeKind;
  status: RepaymentV2StatusView | null;
  errorMessage: string | null;
  approvalMayHaveMutated: boolean;
}

export type RedeemQuotedResult = VaultOperationResult & {
  /** Which wallet mutation had an uncertain transport result. */
  ambiguityStage?: 'approval' | 'submission';
  /** Typed backend response belongs to the initiating session, which is no longer active. */
  sessionChangedAfterSubmission?: boolean;
};

/** Fresh icUSD ledger reads prepared before the click-handler signer window. */
export interface RedemptionPreflight {
  principalText: string;
  walletType: string | null;
  sessionGeneration: number;
  ledgerId: string;
  observedAtMs: number;
  allowanceRaw: bigint;
  balanceRaw: bigint;
  feeRaw: bigint;
}

/**
 * Core API client for interacting with the protocol backend
 */
export class ApiClient {

  private static readonly STALE_OPERATION_TIMEOUT = 60 * 1000; // 60 seconds - reduced from 5 minutes
  private static operationTimestamps: Map<number, number> = new Map();
  private static cleanupInterval: NodeJS.Timeout | null = null;
  static OPERATION_TIMEOUT = 60 * 1000; // 60 seconds to match STALE_OPERATION_TIMEOUT


  // Add this property to track if an operation is in progress
private static operationInProgress = false;

  // Raw-vault-snapshot cache lives in ./rawSnapshotCache, keyed by wallet
  // principal (FE-002) so a verifier never reads a snapshot captured under a
  // different wallet. Warmed wherever get_vaults is fetched so the Oisy `_arr`
  // verifiers can read pre-op ("before") state synchronously instead of
  // awaiting get_vaults inside the click gesture window.

  /** Current wallet principal as text, or null when disconnected. Synchronous. */
  private static currentPrincipalText(): string | null {
    const p = get(walletStore).principal;
    return p ? p.toString() : null;
  }

/**
 * Helper for sequential operations with integrated operation tracking
 * @param operation The operation function to execute
 * @param operationId Optional ID for tracking (vaultId for vault operations)
 * @param refreshOptions Options for data refreshing
 */
static async executeSequentialOperation<T>(
  operation: () => Promise<T>, 
  operationId?: number,
  refreshOptions = { refreshBefore: true, refreshAfter: true }
): Promise<T> {
  // Wait for any previous operation to complete WITH TIMEOUT
  let waitCount = 0;
  const maxWaitAttempts = 20; // 10 seconds max wait (20 * 500ms)
  
  while (ApiClient.operationInProgress) {
    waitCount++;
    if (waitCount >= maxWaitAttempts) {
      console.warn('Operation wait timeout exceeded, proceeding with new operation');
      // Don't force reset - just continue with new operation
      break;
    }
    await new Promise(resolve => setTimeout(resolve, 500));
  }
  
  // Track operation start if we have an ID
  if (operationId !== undefined) {
    ApiClient.operationTimestamps.set(operationId, Date.now());
  }
  
  try {
    // Mark that we're starting an operation
    ApiClient.operationInProgress = true;
    
    // Refresh vaults at the beginning if enabled
    // Skip for Oisy: any network await between the click and the first signer
    // call burns the browser's transient-activation window and trips
    // "Signer window should not be opened outside of click handler".
    if (refreshOptions.refreshBefore && !isOisyWallet()) {
      console.log('Refreshing vault data before operation');
      try {
        await ApiClient.refreshVaultData();
      } catch (refreshErr) {
        console.warn('Error refreshing vault data before operation:', refreshErr);
        // Continue with operation despite refresh error
      }
    }
    
    // Execute the operation
    const result = await operation();
    
    return result;
  } finally {
    // Always mark operation as complete, even if it fails
    ApiClient.operationInProgress = false;
    
    // Clear operation tracking if we were tracking
    if (operationId !== undefined) {
      ApiClient.operationTimestamps.delete(operationId);
    }
    
    // Refresh vaults at the end if enabled
    if (refreshOptions.refreshAfter) {
      console.log('Refreshing vault data after operation');
      try {
        await ApiClient.refreshVaultData();
      } catch (err) {
        console.warn('Error refreshing vault data after operation:', err);
        // Non-critical error, don't re-throw
      }
    }
  }
}
/**
 * Helper to ensure vault data is refreshed consistently
 */
private static async refreshVaultData(): Promise<void> {
  try {
    // Clear the vault cache to force fresh data
    ApiClient.clearVaultCache();
    // Reload the vaults from the backend
    await ApiClient.getUserVaults(true);
    const { vaultStore } = await import('$lib/stores/vaultStore');
    await vaultStore.loadVaults(true)
    console.log('Vault data refresh complete');
  } catch (err) {
    console.warn('Error refreshing vault data:', err);
    // Continue - this is a non-critical operation
  }
}

  /**
   * Start the stale operation cleanup interval
   */
  static startCleanupInterval(): void {
    if (this.cleanupInterval) return;
    
    this.cleanupInterval = setInterval(() => {
      this.clearAllStaleOperations();
    }, 120000); // Run every 10 seconds (more frequent checks)
    
    console.log('Started stale operation cleanup interval');
  }

    /**
   * Get an actor with the current user's identity
   */
    private static async getAuthenticatedActor(): Promise<_SERVICE> {
      if (USE_MOCK_DATA) {
        return publicActor; // Use anonymous actor for mock data
      }

      try {
        return await walletStore.getActor(CONFIG.currentCanisterId, rumi_backendIDL) as _SERVICE;
      } catch (err) {
        console.error('Failed to get authenticated actor:', err);
        throw new Error('Failed to initialize protocol actor');
      }
    }

    /**
     * Bound sibling of getAuthenticatedActor: re-asserts ctx immediately
     * before the actor is constructed, so an actor built after an internal
     * await can never end up bound to a different session's identity than
     * the one the caller pinned at the start of the action.
     */
    private static async getBoundAuthenticatedActor(ctx: ActionBoundContext): Promise<_SERVICE> {
      assertActionBoundContextCurrent(ctx);
      try {
        return await walletStore.getActor(CONFIG.currentCanisterId, rumi_backendIDL) as _SERVICE;
      } catch (err) {
        console.error('Failed to get authenticated (bound) actor:', err);
        throw new Error('Failed to initialize protocol actor');
      }
    }

    /**
     * Extracts the vault id from a typed ProtocolError's GenericError text
     * when it proves the vault.rs open_vault_and_borrow partial-failure shape
     * ("Vault created (id=N)..."). This inspects the exact source-proven typed
     * error variant returned by the backend, NOT a lowercased/normalized
     * substring classifier — it must only ever be called on a real Err
     * received from a typed backend response, never on a message string
     * synthesized from a thrown/ambiguous transport error.
     */
    private static extractPartialZeroDebtVaultId(error: unknown): number | null {
      if (error && typeof error === 'object' && 'GenericError' in (error as any) && typeof (error as any).GenericError === 'string') {
        const m = /Vault created \(id=(\d+)\)/.exec((error as any).GenericError);
        if (m) return Number(m[1]);
      }
      return null;
    }

  /**
   * Stop the cleanup interval
   */
  static stopCleanupInterval(): void {
    if (this.cleanupInterval) {
      clearInterval(this.cleanupInterval);
      this.cleanupInterval = null;
      console.log('Stopped stale operation cleanup interval');
    }
  }

  /**
   * Verify vault ownership and existence
   */
  static async verifyVaultAccess(vaultId: number): Promise<{
    vault: any;
    actor: any;
    error?: string;
  }> {
    try {
      // Check if vault exists (uses anonymous actor for reads)
      const vault = await this.getVaultById(vaultId);
      if (!vault) {
        return { vault: null, actor: null, error: 'Vault not found' };
      }

      // Verify ownership
      const walletState = get(walletStore);
      if (vault.owner !== walletState.principal?.toString()) {
        return { vault, actor: null, error: 'You do not own this vault' };
      }

      // Only get authenticated actor after ownership verified (for update calls)
      const actor = await this.getAuthenticatedActor();
      return { vault, actor };
    } catch (error) {
      console.error('Error verifying vault access:', error);
      return {
        vault: null,
        actor: null,
        error: error instanceof Error ? error.message : 'Failed to verify vault access'
      };
    }
  }

  /**
   * Make a call to a public endpoint that doesn't require authentication
   */
  static getPublicData(method: 'get_redemption_queue'): ReturnType<_SERVICE['get_redemption_queue']>;
  static getPublicData(method: 'get_redemption_quote', amountE8s: bigint): ReturnType<_SERVICE['get_redemption_quote']>;
  static getPublicData(method: 'get_redemption_preview', amountE8s: bigint): ReturnType<_SERVICE['get_redemption_preview']>;
  static async getPublicData<T>(method: keyof typeof publicActor, ...args: any[]): Promise<T>;
  static async getPublicData<T>(
    method: keyof typeof publicActor,
    ...args: any[]
  ): Promise<T> {
    if (USE_MOCK_DATA) {
      return this.getMockData<T>(method as string, ...args);
    }

    try {
      console.log(`Calling ${String(method)} with args:`, args);
      return (await (publicActor[method] as any)(...args)) as T;
    } catch (err) {
      console.error(`Failed to fetch ${String(method)}:`, err);
      throw new Error(`Could not fetch ${String(method)}`);
    }
  }

  /**
   * Get mock data for development
   */
  static getMockData<T>(method: string, ...args: any[]): T {
    console.log(`[MOCK] Getting mock data for ${method} with args:`, args);
    
    // Protocol status mock data
    if (method === 'get_protocol_status') {
      return {
        mode: 'GeneralAvailability',
        total_icp_margin: 1000000000n,
        total_icusd_borrowed: 500000000n,
        last_icp_rate: 6.41,
        last_icp_timestamp: BigInt(Date.now()),
        total_collateral_ratio: 2.0
      } as unknown as T;
    }
    
    // Default empty response
    return {} as T;
  }

    /**
   * Manually trigger the backend to process pending transfers
   */
    static async triggerPendingTransfers(): Promise<boolean> {
        try {
          // Use anonymous actor — get_protocol_status is a public query
          await publicActor.get_protocol_status();

          // Make a second call after a short delay to help trigger the timer
          setTimeout(async () => {
            try {
              await publicActor.get_protocol_status();
            } catch (e) {
              console.error('Error in follow-up call:', e);
            }
          }, 2000);
          
          return true;
        } catch (err) {
          console.error('Error triggering pending transfers:', err);
          return false;
        }
      }
  
  /**
   * Format error messages from the protocol backend
   */
  static formatProtocolError(error: any): string {
    // Handle BigInt serialization issues
    if (error instanceof Error && error.message.includes('Do not know how to serialize a BigInt')) {
      return 'Error processing large numbers. Please try again.';
    }

    if (typeof error === 'string') {
      return error;
    }
    
    // Handle specific error types
    if ('AnonymousCallerNotAllowed' in error) {
      return 'You must connect your wallet to perform this action';
    } 
    else if ('CallerNotOwner' in error) {
      return 'You do not have permission to modify this vault';
    } 
    else if ('TemporarilyUnavailable' in error && typeof error.TemporarilyUnavailable === 'string') {
      return `Service temporarily unavailable: ${error.TemporarilyUnavailable}`;
    } 
    else if ('GenericError' in error && typeof error.GenericError === 'string') {
      // Turn the raw BorrowReservationGuard ceiling / mint-cap rejections
      // (guard.rs) into friendly copy; fall through to the raw string otherwise.
      return friendlyBorrowCapError(error.GenericError) ?? error.GenericError;
    }
    else if ('TransferError' in error) {
      const te = error.TransferError;
      if ('InsufficientFunds' in te) {
        return `Insufficient funds. Balance: ${Number(te.InsufficientFunds.balance) / E8S}`;
      }
      if ('BadFee' in te) {
        return `Unexpected fee. Expected: ${Number(te.BadFee.expected_fee)}`;
      }
      return `Transfer error: ${BigIntUtils.stringify(te)}`;
    }
    else if ('TransferFromError' in error) {
      const tfe = error.TransferFromError[0];
      if ('InsufficientAllowance' in tfe) {
        return `Insufficient allowance (have: ${Number(tfe.InsufficientAllowance.allowance) / E8S}). Please approve the tokens first.`;
      }
      if ('InsufficientFunds' in tfe) {
        return `Insufficient ${error.TransferFromError[1] ? 'token' : ''} funds. Your balance is too low for this amount.`;
      }
      if ('BadFee' in tfe) {
        return `Unexpected fee. Expected: ${Number(tfe.BadFee.expected_fee)}`;
      }
      return `Transfer error: ${BigIntUtils.stringify(tfe)}`;
    } 
    else if ('AmountTooLow' in error) {
      return `Amount too low. Minimum amount: ${Number(error.AmountTooLow.minimum_amount) / E8S} icUSD`;
    }
    else if ('AlreadyProcessing' in error) {
      return 'This operation is already in progress. Please wait.';
    }
    
    return 'An error occurred with the operation';
  }

  /**
   * Check if an error is an AlreadyProcessing error
   */
  static isAlreadyProcessingError(error: any): boolean {
    return error && (
      ('AlreadyProcessing' in error) || 
      (error instanceof Error && error.message.toLowerCase().includes('already has an ongoing operation')) ||
      (error instanceof Error && error.message.toLowerCase().includes('operation in progress'))
    );
  }

  /**
   * Check if an error is for a stale processing state
   */
  static isStaleProcessingState(error: any, timeThresholdSeconds: number = 90): boolean {
    if (error && typeof error === 'object' && 'timestamp' in error) {
      const errorTime = Number(error.timestamp);
      return Date.now() - errorTime > timeThresholdSeconds * 1000;
    }
    
    if (error instanceof Error && error.message.toLowerCase().includes('stale')) {
      return true;
    }
    
    return false;
  }

    /**
     * Open a new vault with collateral.
     * @param collateralAmount Amount of collateral in human-readable units (e.g., 2.5 ICP)
     * @param collateralTypePrincipal Optional principal text of the collateral token's ledger.
     *        If omitted, defaults to ICP.
     */
    static async openVault(collateralAmount: number, collateralTypePrincipal?: string): Promise<VaultOperationResult> {
        // Keep track of ongoing request
        let abortController: AbortController | null = null;

        // Resolve collateral info
        const ctPrincipal = collateralTypePrincipal || CANISTER_IDS.ICP_LEDGER;
        const collateralInfo = collateralStore.getCollateralInfo(ctPrincipal);
        const decimals = collateralInfo?.decimals ?? 8;
        const decimalsFactor = Math.pow(10, decimals);
        const ledgerCanisterId = collateralInfo?.ledgerCanisterId ?? CONFIG.currentIcpLedgerId;
        const symbol = collateralInfo?.symbol ?? 'ICP';

        try {
          console.log(`Creating vault with ${collateralAmount} ${symbol}`);

          // Minimum amount check (in raw units) — per-collateral from on-chain config
          const amountRaw = BigInt(Math.floor(collateralAmount * decimalsFactor));
          const minDeposit = collateralInfo?.minCollateralDeposit ?? 0;
          if (minDeposit > 0 && amountRaw < BigInt(minDeposit)) {
            return {
              success: false,
              error: `Amount too low. Minimum required: ${minDeposit / decimalsFactor} ${symbol}`
            };
          }

          // Check wallet connection status before proceeding
          const walletState = get(walletStore);
          if (!walletState.isConnected || !walletState.principal) {
            return {
              success: false,
              error: "Wallet not connected. Please connect your wallet and try again."
            };
          }

          // Create a new abort controller for this request
          abortController = new AbortController();
          const signal = abortController.signal;

          // Enhanced error handling for wallet signer issues
          try {
            const actor = await ApiClient.getAuthenticatedActor();

            // Build the optional collateral_type argument
            const collateralTypeOpt: [] | [Principal] = ctPrincipal === CANISTER_IDS.ICP_LEDGER
              ? []  // ICP is the default, no need to pass
              : [Principal.fromText(ctPrincipal)];

            // Pre-open vault-id snapshot for the Oisy false-negative
            // verifier: if open_vault lands but Oisy mangles the
            // response, we still find the new vault by diffing IDs.
            // Oisy reads the warm sync cache (no network await inside the
            // click gesture window); non-Oisy awaits a fresh snapshot.
            const beforeIds = isOisyWallet() ? ApiClient.getCachedUserVaultIds() : await ApiClient.snapshotUserVaultIds();
            const verifyOpenLanded = async () => {
              if (!beforeIds) return false;
              const newVault = await ApiClient.findNewlyOpenedVault(beforeIds);
              if (!newVault) return false;
              // Sanity check: collateral on the new vault is at least
              // 95% of what we requested. Below that and something
              // really odd happened.
              return newVault.collateralAmount >= (amountRaw * 95n) / 100n;
            };

            // ─── Oisy ICRC-112 batched path ───
            // Batches approve + open_vault into a single signer popup via ICRC-112.
            // The SignerAgent natively handles icrc2_approve consent (Tier 1) so
            // the ICP ledger's lack of ICRC-21 is not an issue.
            const signerAgent = isOisyWallet() ? await pnp.getSignerAgent() : null;

            if (signerAgent) {
              console.log(`[Oisy] Sequential approve + open_vault via @icp-sdk/signer v5`);
              const requestedAllowance = amountRaw * 105n / 100n;

              // Get the ledger actor (routes through Oisy signer)
              const ledgerActor = await walletStore.getActor(ledgerCanisterId, CONFIG.icp_ledgerIDL) as any;

              // 1) Approve (first Oisy consent screen). icrc2_approve is handled
              //    natively by Oisy at Tier 1 — no ICRC-21 needed on the ledger.
              const approveResult = await ledgerActor.icrc2_approve({
                amount: requestedAllowance,
                spender: {
                  owner: Principal.fromText(CONFIG.currentCanisterId),
                  subaccount: []
                },
                expires_at: [],
                expected_allowance: [],
                memo: [],
                fee: [],
                from_subaccount: [],
                created_at_time: []
              });
              if (approveResult && 'Err' in approveResult) {
                return {
                  success: false,
                  error: `${symbol} approval failed: ${JSON.stringify(approveResult.Err)}`
                };
              }

              // 2) Open vault (second Oisy consent screen).
              //    Wrapped with _arr false-negative guard since the backend method
              //    is custom and prone to Oisy's Principal serialization bug.
              const result = await callWithOisyFalseNegativeGuard(
                () => actor.open_vault(amountRaw, collateralTypeOpt),
                verifyOpenLanded,
                `Oisy open_vault ${collateralAmount} ${symbol}`
              );

              if (isOisyLandedSentinel(result)) {
                const found = beforeIds ? await ApiClient.findNewlyOpenedVault(beforeIds) : null;
                return {
                  success: true,
                  vaultId: found?.vaultId,
                  blockIndex: undefined,
                  oisyResilient: true,
                };
              }

              if ('Ok' in result) {
                return {
                  success: true,
                  vaultId: Number(result.Ok.vault_id),
                  blockIndex: Number(result.Ok.block_index)
                };
              } else {
                return {
                  success: false,
                  error: ApiClient.formatProtocolError(result.Err)
                };
              }
            }

            // ─── Standard ICRC-2 path (Plug, II, etc.) ───
            // Sequential approve then open_vault (two separate popups)
            const spenderCanisterId = CONFIG.currentCanisterId;

            // First check current allowance (using generic collateral method)
            const currentAllowance = await walletOperations.checkCollateralAllowance(spenderCanisterId, ledgerCanisterId);
            console.log(`Current ${symbol} allowance for protocol canister: ${Number(currentAllowance) / decimalsFactor}`);

            // If allowance is insufficient, request approval
            if (currentAllowance < amountRaw) {
              console.log(`Requesting approval for ${collateralAmount} ${symbol}`);

              // Use a higher allowance (5% more than needed) to avoid small rounding issues
              const requestedAllowance = amountRaw * 105n / 100n;

              const approvalResult = await walletOperations.approveCollateralTransfer(
                requestedAllowance, spenderCanisterId, ledgerCanisterId
              );

              if (!approvalResult.success) {
                return {
                  success: false,
                  error: approvalResult.error || `Failed to approve ${symbol} transfer`
                };
              }

              console.log(`Successfully set ${symbol} allowance to ${Number(requestedAllowance) / decimalsFactor}`);
            }

            // Add a timeout to catch hanging signatures
            const timeoutPromise = new Promise<never>((_, reject) => {
              setTimeout(() => reject(new Error("Wallet signature request timed out")), 60000);
            });

            // Race between the actual operation and the timeout
            const result = await callWithOisyFalseNegativeGuard(
              () => Promise.race([
                actor.open_vault(amountRaw, collateralTypeOpt),
                timeoutPromise
              ]),
              verifyOpenLanded,
              `open_vault ${collateralAmount} ${symbol}`
            );

            if (isOisyLandedSentinel(result)) {
              const found = beforeIds ? await ApiClient.findNewlyOpenedVault(beforeIds) : null;
              return {
                success: true,
                vaultId: found?.vaultId,
                blockIndex: undefined,
                oisyResilient: true,
              };
            }

            if ('Ok' in result) {
              return {
                success: true,
                vaultId: Number(result.Ok.vault_id),
                blockIndex: Number(result.Ok.block_index)
              };
            } else {
              return {
                success: false,
                error: ApiClient.formatProtocolError(result.Err)
              };
            }
          } catch (signerErr) {
            console.error('Signer error:', signerErr);
            
            // Explicitly abort any pending requests
            if (abortController && !signal.aborted) {
              abortController.abort();
              console.log('Aborted previous signature request after error');
            }
            
            // Handle insufficient allowance errors
            if (signerErr instanceof Error) {
              const errMsg = signerErr.message.toLowerCase();
              
              if (errMsg.includes('insufficientallowance') || 
                  errMsg.includes('insufficient allowance')) {
                return {
                  success: false,
                  error: "Insufficient ICP allowance. Please try again to approve the required amount."
                };
              }
              
              if (errMsg.includes('invalid response from signer') || 
                  errMsg.includes('failed to sign') ||
                  errMsg.includes('rejected') ||
                  errMsg.includes('user declined')) {
                
                // Clear any pending wallet states
                await walletOperations.resetWalletSignerState();
                
                // Attempt to refresh the wallet connection
                try {
                  await walletStore.refreshWallet();
                  return {
                    success: false,
                    error: "Wallet signature failed. Please try again after refreshing the page."
                  };
                } catch (refreshErr) {
                  return {
                    success: false,
                    error: "Wallet signature error. Please disconnect and reconnect your wallet."
                  };
                }
              }
            }
            
            throw signerErr; // Re-throw if it's not a specific signer error we can handle
          }
        } catch (err) {
          console.error('Error opening vault:', err);
          return {
            success: false,
            error: err instanceof Error ? err.message : 'Unknown error opening vault'
          };
        } finally {
          // Make sure to clean up the abort controller
          if (abortController && !abortController.signal.aborted) {
            abortController.abort();
          }
        }
      }


/**
 * Compound: open vault + borrow icUSD in a single canister call.
 *
 * For Oisy / ICRC-112 wallets this batches approve + open_vault_and_borrow
 * into **one** signer popup (instead of approve → open_vault → borrow which
 * requires three popups and fails because the browser blocks async popups).
 *
 * For non-Oisy wallets this still reduces round-trips (approve → one backend call
 * instead of approve → open_vault → borrow).
 */
static async openVaultAndBorrow(
  collateralAmount: number,
  icusdAmount: number,
  collateralTypePrincipal?: string
): Promise<VaultOperationResult> {
  return ApiClient.executeSequentialOperation(async () => {
    const ctPrincipal = collateralTypePrincipal || CANISTER_IDS.ICP_LEDGER;
    const collateralInfo = collateralStore.getCollateralInfo(ctPrincipal);
    const decimals = collateralInfo?.decimals ?? 8;
    const decimalsFactor = Math.pow(10, decimals);
    const ledgerCanisterId = collateralInfo?.ledgerCanisterId ?? CONFIG.currentIcpLedgerId;
    const symbol = collateralInfo?.symbol ?? 'ICP';

    try {
      console.log(`Creating vault with ${collateralAmount} ${symbol} and borrowing ${icusdAmount} icUSD`);

      const amountRaw = BigInt(Math.floor(collateralAmount * decimalsFactor));
      const borrowRaw = BigInt(Math.floor(icusdAmount * E8S));

      const minDeposit = collateralInfo?.minCollateralDeposit ?? 0;
      if (minDeposit > 0 && amountRaw < BigInt(minDeposit)) {
        return { success: false, error: `Amount too low. Minimum required: ${minDeposit / decimalsFactor} ${symbol}` };
      }

      const walletState = get(walletStore);
      if (!walletState.isConnected || !walletState.principal) {
        return { success: false, error: "Wallet not connected. Please connect your wallet and try again." };
      }

      try {
        const actor = await ApiClient.getAuthenticatedActor();

        const collateralTypeOpt: [] | [Principal] = ctPrincipal === CANISTER_IDS.ICP_LEDGER
          ? []
          : [Principal.fromText(ctPrincipal)];

        // Pre-open-and-borrow snapshot for the Oisy false-negative
        // verifier. open_vault_and_borrow creates a new vault AND
        // borrows in one canister call; we treat "new vault appeared
        // with the right collateral" as proof the call landed.
        // Oisy reads the warm sync cache (no network await inside the
        // click gesture window); non-Oisy awaits a fresh snapshot.
        const beforeIds = isOisyWallet() ? ApiClient.getCachedUserVaultIds() : await ApiClient.snapshotUserVaultIds();
        const verifyOpenAndBorrowLanded = async () => {
          if (!beforeIds) return false;
          const newVault = await ApiClient.findNewlyOpenedVault(beforeIds);
          if (!newVault) return false;
          return newVault.collateralAmount >= (amountRaw * 95n) / 100n;
        };

        // ─── Oisy ICRC-112 batched path ───
        const signerAgent = isOisyWallet() ? await pnp.getSignerAgent() : null;

        if (signerAgent) {
          console.log(`[Oisy] Sequential approve + open_vault_and_borrow via @icp-sdk/signer v5`);
          const oisyLedgerFee = BigInt(collateralInfo?.ledgerFee ?? 10_000);
          const requestedAllowance = amountRaw + oisyLedgerFee * 2n;

          const ledgerActor = await walletStore.getActor(ledgerCanisterId, CONFIG.icp_ledgerIDL) as any;

          // 1) Approve (first Oisy consent screen, native Tier 1 handling).
          const approveResult = await ledgerActor.icrc2_approve({
            amount: requestedAllowance,
            spender: {
              owner: Principal.fromText(CONFIG.currentCanisterId),
              subaccount: []
            },
            expires_at: [],
            expected_allowance: [],
            memo: [],
            fee: [],
            from_subaccount: [],
            created_at_time: []
          });
          if (approveResult && 'Err' in approveResult) {
            return { success: false, error: `${symbol} approval failed: ${JSON.stringify(approveResult.Err)}` };
          }

          // 2) open_vault_and_borrow (second consent screen), guarded against _arr.
          const result = await callWithOisyFalseNegativeGuard(
            () => actor.open_vault_and_borrow(amountRaw, borrowRaw, collateralTypeOpt),
            verifyOpenAndBorrowLanded,
            `Oisy open_vault_and_borrow ${collateralAmount} ${symbol}`
          );

          if (isOisyLandedSentinel(result)) {
            const found = beforeIds ? await ApiClient.findNewlyOpenedVault(beforeIds) : null;
            return {
              success: true,
              vaultId: found?.vaultId,
              blockIndex: undefined,
              oisyResilient: true,
            };
          }

          if ('Ok' in result) {
            return {
              success: true,
              vaultId: Number(result.Ok.vault_id),
              blockIndex: Number(result.Ok.block_index)
            };
          } else {
            return { success: false, error: ApiClient.formatProtocolError(result.Err) };
          }
        }

        // ─── Standard ICRC-2 path (Plug, II, etc.) ───
        const spenderCanisterId = CONFIG.currentCanisterId;

        const currentAllowance = await walletOperations.checkCollateralAllowance(spenderCanisterId, ledgerCanisterId);
        const ledgerFee = BigInt(collateralInfo?.ledgerFee ?? 10_000);
        const requiredAllowance = amountRaw + ledgerFee;
        if (currentAllowance < requiredAllowance) {
          const requestedAllowance = amountRaw + ledgerFee * 2n;
          const approvalResult = await walletOperations.approveCollateralTransfer(
            requestedAllowance, spenderCanisterId, ledgerCanisterId
          );
          if (!approvalResult.success) {
            return { success: false, error: approvalResult.error || `Failed to approve ${symbol} transfer` };
          }
        }

        const timeoutPromise = new Promise<never>((_, reject) => {
          setTimeout(() => reject(new Error("Wallet signature request timed out")), 60000);
        });

        const result = await callWithOisyFalseNegativeGuard(
          () => Promise.race([
            actor.open_vault_and_borrow(amountRaw, borrowRaw, collateralTypeOpt),
            timeoutPromise
          ]),
          verifyOpenAndBorrowLanded,
          `open_vault_and_borrow ${collateralAmount} ${symbol}`
        );

        if (isOisyLandedSentinel(result)) {
          const found = beforeIds ? await ApiClient.findNewlyOpenedVault(beforeIds) : null;
          return {
            success: true,
            vaultId: found?.vaultId,
            blockIndex: undefined,
            oisyResilient: true,
          };
        }

        if ('Ok' in result) {
          return {
            success: true,
            vaultId: Number(result.Ok.vault_id),
            blockIndex: Number(result.Ok.block_index)
          };
        } else {
          return { success: false, error: ApiClient.formatProtocolError(result.Err) };
        }
      } catch (signerErr) {
        console.error('Error in openVaultAndBorrow:', signerErr);

        if (signerErr instanceof Error) {
          const errMsg = signerErr.message.toLowerCase();
          if (errMsg.includes('insufficientallowance') || errMsg.includes('insufficient allowance')) {
            return { success: false, error: "Insufficient ICP allowance. Please try again to approve the required amount." };
          }
          if (errMsg.includes('invalid response from signer') || errMsg.includes('failed to sign') ||
              errMsg.includes('rejected') || errMsg.includes('user declined')) {
            await walletOperations.resetWalletSignerState();
            try {
              await walletStore.refreshWallet();
              return { success: false, error: "Wallet signature failed. Please try again after refreshing the page." };
            } catch {
              return { success: false, error: "Wallet signature error. Please disconnect and reconnect your wallet." };
            }
          }
        }
        throw signerErr;
      }
    } catch (err) {
      console.error('Error in openVaultAndBorrow:', err);
      return { success: false, error: err instanceof Error ? err.message : 'Unknown error opening vault' };
    }
  });
}

/**
 * Borrow icUSD from an existing vault
 */
static async borrowFromVault(vaultId: number, icusdAmount: number): Promise<VaultOperationResult> {
  return ApiClient.executeSequentialOperation(async () => {
    try {
      console.log(`Borrowing ${icusdAmount} icUSD from vault #${vaultId}`);
      
      // Validate input is finite before any calculations
      if (!isFinite(icusdAmount) || icusdAmount <= 0) {
        return {
          success: false,
          error: `Invalid borrowing amount: ${icusdAmount}. Amount must be a finite positive number.`
        };
      }
      
      if (icusdAmount * E8S < MIN_ICUSD_AMOUNT) { // Updated minimum validation
        return {
          success: false,
          error: `Amount too low. Minimum borrowing amount: ${MIN_ICUSD_AMOUNT / E8S} icUSD`
        };
      }
      
      // Simulate processing delay.
      // Skip for Oisy: this artificial await burns the click gesture window
      // before the first signer call and trips the popup error.
      if (!isOisyWallet()) {
        await new Promise(resolve => setTimeout(resolve, 1200));
      }

      const actor = await ApiClient.getAuthenticatedActor();
      const vaultArg = {
        vault_id: BigInt(vaultId),
        amount: BigInt(Math.floor(icusdAmount * E8S))
      };

      // Snapshot pre-borrow state for the Oisy false-negative verifier.
      // We compare RAW e8s (not the rounded human float on VaultDTO) so a
      // 0.1 icUSD borrow can't be hidden by display rounding.
      // Oisy reads the warm sync cache (no network await inside the click
      // gesture window); non-Oisy awaits a fresh snapshot.
      const before = isOisyWallet() ? ApiClient.getCachedRawBorrowedE8s(vaultId) : await ApiClient.getRawBorrowedE8s(vaultId);
      const expectedDeltaE8s = vaultArg.amount;

      const result = await callWithOisyFalseNegativeGuard(
        () => actor.borrow_from_vault(vaultArg),
        async () => {
          if (before === null) return false;
          // Allow up to 1 minute for canister state to be observable
          // post-call (interest accrual + cert propagation). 95% of the
          // borrow amount lower bound tolerates the rounding from
          // BigInt(Math.floor(amount * E8S)) on tiny amounts.
          const after = await ApiClient.getRawBorrowedE8s(vaultId);
          if (after === null) return false;
          const delta = after - before;
          return delta >= (expectedDeltaE8s * 95n) / 100n;
        },
        `borrow ${icusdAmount} icUSD from vault #${vaultId}`
      );

      if (isOisyLandedSentinel(result)) {
        return {
          success: true,
          vaultId,
          blockIndex: undefined,
          feePaid: undefined,
          oisyResilient: true,
        };
      }

      if ('Ok' in result) {
        return {
          success: true,
          vaultId,
          blockIndex: Number(result.Ok.block_index),
          feePaid: Number(result.Ok.fee_amount_paid) / E8S
        };
      } else {
        return {
          success: false,
          error: ApiClient.formatProtocolError(result.Err)
        };
      }
    } catch (err) {
      console.error('Error borrowing from vault:', err);
      return {
        success: false,
        error: err instanceof Error ? err.message : 'Unknown error borrowing from vault'
      };
    }
    // REMOVED: Don't manually track operations here - executeSequentialOperation does this
  }, vaultId); // Pass vaultId here to let executeSequentialOperation track it
}

/**
 * Bound sibling of openVaultAndBorrow (see ActionBoundContext / BoundOpenVaultAndBorrowResult
 * doc comments for the full contract). Takes RAW bigint wire amounts — no Number/float
 * round-trip, no +0.5 bias — and re-verifies ctx immediately before every actor acquisition
 * and mutating call, and immediately after every internal await, so an account switch (or a
 * same-account disconnect/reconnect, i.e. a new session with the same principal text) mid-flow
 * aborts before any further signer call rather than silently mixing identities across sub-steps.
 *
 * Deliberately does NOT run the legacy Oisy `_arr` false-negative on-chain landed-heuristic:
 * a heuristic vault-scan match is not provably attributable to this specific attempt, so any
 * thrown error after the mutating call is dispatched (including that pattern) is surfaced as
 * 'ambiguous_transport', never upgraded to a success. Does NOT go through
 * executeSequentialOperation — no global mutex, no before/after vault-cache refresh; the caller
 * owns reconciliation.
 */
static async getCollateralIngressStateBound(
  ctx: ActionBoundContext,
  ledgerCanisterId: string
): Promise<InboundCollateralRequestState> {
  assertActionBoundContextCurrent(ctx);
  const actor = await ApiClient.getBoundAuthenticatedActor(ctx);
  assertActionBoundContextCurrent(ctx);
  const result = await actor.get_my_collateral_ingress_state(Principal.fromText(ledgerCanisterId));
  assertActionBoundContextCurrent(ctx);
  if ('Err' in result) throw new Error(ApiClient.formatProtocolError(result.Err));
  return result.Ok;
}

static async getCollateralIngressBound(
  ctx: ActionBoundContext,
  ledgerCanisterId: string,
  requestId: bigint
): Promise<InboundCollateralStatusView | null> {
  assertActionBoundContextCurrent(ctx);
  const actor = await ApiClient.getBoundAuthenticatedActor(ctx);
  assertActionBoundContextCurrent(ctx);
  const result = await actor.get_my_collateral_ingress(Principal.fromText(ledgerCanisterId), requestId);
  assertActionBoundContextCurrent(ctx);
  return result[0] ?? null;
}

/**
 * Opens a vault through the durable inbound collateral request journal. The caller must persist
 * and reuse requestId (decimal Nat) across reloads; this method intentionally never allocates or
 * rotates an ID. A thrown/lost reply stays ambiguous unless the owner-scoped journal query
 * returns the exact request row/result.
 */
static async openVaultV2Bound(
  ctx: ActionBoundContext,
  requestId: bigint,
  collateralAmountRaw: bigint,
  collateralTypePrincipal?: string
): Promise<BoundOpenVaultV2Result> {
  const ctPrincipal = collateralTypePrincipal || CANISTER_IDS.ICP_LEDGER;
  const collateralInfo = collateralStore.getCollateralInfo(ctPrincipal);
  const ledgerCanisterId = collateralInfo?.ledgerCanisterId ?? CONFIG.currentIcpLedgerId;
  const symbol = collateralInfo?.symbol ?? 'ICP';
  let approvalDispatched = false;
  let existingPendingRequest = false;
  const result = (kind: BoundActionOutcomeKind, status: InboundCollateralStatusView | null, errorMessage: string | null) => ({
    kind, status, errorMessage, approvalMayHaveMutated: approvalDispatched,
  });

  if (requestId <= 0n || collateralAmountRaw <= 0n) {
    return result('predispatch_aborted', null, 'Invalid collateral request ID or amount.');
  }
  let collateralTypeOpt: [] | [Principal];
  try {
    assertActionBoundContextCurrent(ctx);
    collateralTypeOpt = ctPrincipal === CANISTER_IDS.ICP_LEDGER ? [] : [Principal.fromText(ctPrincipal)];
    const ingressState = await ApiClient.getCollateralIngressStateBound(ctx, ledgerCanisterId);
    assertActionBoundContextCurrent(ctx);
    const active = ingressState.active_request[0] ?? null;
    const latest = ingressState.latest_result[0] ?? null;
    const exactStatus = [active, latest].find((view) => view?.request_id === requestId) ?? null;
    if (exactStatus) {
      const operationMatches = 'Open' in exactStatus.operation &&
        exactStatus.operation.Open.collateral_type.toText() === ctPrincipal;
      if (exactStatus.owner.toText() !== ctx.expectedPrincipalText || exactStatus.ledger.toText() !== ledgerCanisterId ||
          exactStatus.amount_raw !== collateralAmountRaw || !operationMatches) {
        return result('predispatch_aborted', exactStatus, 'This request ID is already bound to different collateral arguments. No approval or new open was submitted.');
      }
      if ('Complete' in exactStatus.phase || 'Rejected' in exactStatus.phase) {
        return 'Complete' in exactStatus.phase
          ? result('dispatched_ok', exactStatus, null)
          : result('predispatch_aborted', exactStatus, exactStatus.result[0] && 'Rejected' in exactStatus.result[0]
              ? exactStatus.result[0].Rejected.message : 'The backend recorded this request as rejected.');
      }
      existingPendingRequest = true;
    }
    if (active && active.request_id !== requestId) {
      return result('predispatch_aborted', active, 'Another collateral request is active for this ledger. No approval or open was submitted.');
    }
    if (!exactStatus && requestId !== ingressState.next_request_id) {
      return result('predispatch_aborted', null, 'The request ID is old, missing, or not the next owner-scoped ID. Reconcile the journal before opening.');
    }
    const signerAgent = isOisyWallet() ? await pnp.getSignerAgent() : null;
    assertActionBoundContextCurrent(ctx);
    const requiredAllowance = collateralAmountRaw + BigInt(collateralInfo?.ledgerFee ?? 10_000);
    const currentAllowance = existingPendingRequest
      ? requiredAllowance
      : await walletOperations.checkCollateralAllowanceBound(ctx, CONFIG.currentCanisterId, ledgerCanisterId);
    assertActionBoundContextCurrent(ctx);
    const needsApproval = !existingPendingRequest && currentAllowance < requiredAllowance;

    if (signerAgent) {
      if (needsApproval) {
        assertActionBoundContextCurrent(ctx);
        const ledgerActor = await walletStore.getActor(ledgerCanisterId, CONFIG.icp_ledgerIDL) as any;
        assertActionBoundContextCurrent(ctx);
        approvalDispatched = true;
        const approveResult = await ledgerActor.icrc2_approve({
          amount: collateralAmountRaw + BigInt(collateralInfo?.ledgerFee ?? 10_000) * 2n,
          spender: { owner: Principal.fromText(CONFIG.currentCanisterId), subaccount: [] },
          expires_at: largeApprovalExpiry(), expected_allowance: [], memo: [], fee: [], from_subaccount: [], created_at_time: [],
        });
        assertActionBoundContextCurrent(ctx);
        if (approveResult && 'Err' in approveResult) {
          return result('predispatch_aborted', exactStatus, `${symbol} approval failed: ${JSON.stringify(approveResult.Err)}`);
        }
      }
    } else {
      if (needsApproval) {
        const spender = CONFIG.currentCanisterId;
        approvalDispatched = true;
        const approval = await walletOperations.approveCollateralTransferBound(
          ctx, collateralAmountRaw + BigInt(collateralInfo?.ledgerFee ?? 10_000) * 2n, spender, ledgerCanisterId
        );
        assertActionBoundContextCurrent(ctx);
        if (!approval.success) return result('predispatch_aborted', null, approval.error || `Failed to approve ${symbol} transfer`);
      }
    }
  } catch (error) {
    return result('predispatch_aborted', null, error instanceof Error ? error.message : 'Unable to prepare collateral approval.');
  }

  let actor: _SERVICE;
  try {
    assertActionBoundContextCurrent(ctx);
    actor = await ApiClient.getBoundAuthenticatedActor(ctx);
    assertActionBoundContextCurrent(ctx);
  } catch (error) {
    return result('predispatch_aborted', null, error instanceof Error ? error.message : 'Session changed before open dispatch.');
  }

  let response;
  try {
    assertActionBoundContextCurrent(ctx);
    response = await actor.open_vault_v2(requestId, collateralAmountRaw, collateralTypeOpt);
    assertActionBoundContextCurrent(ctx);
  } catch (error) {
    let status: InboundCollateralStatusView | null = null;
    try { status = await ApiClient.getCollateralIngressBound(ctx, ledgerCanisterId, requestId); } catch { /* remains ambiguous */ }
    return result('ambiguous_transport', status, error instanceof Error ? error.message : 'Open reply was lost or unresolved.');
  }

  if ('Ok' in response) {
    const returnedStatus = response.Ok;
    const exactReply = returnedStatus.owner.toText() === ctx.expectedPrincipalText &&
      returnedStatus.ledger.toText() === ledgerCanisterId && returnedStatus.request_id === requestId &&
      returnedStatus.amount_raw === collateralAmountRaw && 'Open' in returnedStatus.operation &&
      returnedStatus.operation.Open.collateral_type.toText() === ctPrincipal;
    if (exactReply) return result('dispatched_ok', returnedStatus, null);
    let exactJournalStatus: InboundCollateralStatusView | null = null;
    try { exactJournalStatus = await ApiClient.getCollateralIngressBound(ctx, ledgerCanisterId, requestId); } catch { /* remains ambiguous */ }
    return result('ambiguous_transport', exactJournalStatus, 'The backend returned a collateral row that does not match this owner, request ID, ledger, and amount. Reconcile the exact request before continuing.');
  }
  let status: InboundCollateralStatusView | null = null;
  try { status = await ApiClient.getCollateralIngressBound(ctx, ledgerCanisterId, requestId); } catch { /* keep typed error and journal unresolved */ }
  return result('dispatched_err', status, ApiClient.formatProtocolError(response.Err));
}

/**
 * Adds collateral using the owner-scoped durable request journal. The caller
 * supplies and persists the request ID; this method never allocates or rotates
 * one. Existing Pending/Held rows are replayed exactly without another
 * approval. `beforeApprovalDispatch` must persist the caller's
 * approval-may-have-mutated marker synchronously before an approval call.
 */
static async addMarginV2Bound(
  ctx: ActionBoundContext,
  requestId: bigint,
  vaultId: number,
  collateralAmountRaw: bigint,
  collateralTypePrincipal: string,
  approvalWasPreviouslyDispatched = false,
  beforeApprovalDispatch?: () => void,
  confirmAmbiguousApprovalRetry?: () => boolean,
): Promise<BoundAddMarginV2Result> {
  const collateralInfo = collateralStore.getCollateralInfo(collateralTypePrincipal);
  const ledgerCanisterId = collateralInfo?.ledgerCanisterId ?? collateralTypePrincipal;
  const symbol = collateralInfo?.symbol ?? 'collateral';
  let approvalDispatched = approvalWasPreviouslyDispatched;
  const result = (kind: BoundActionOutcomeKind, status: InboundCollateralStatusView | null, errorMessage: string | null) => ({
    kind, status, errorMessage, approvalMayHaveMutated: approvalDispatched,
  });
  const matches = (status: InboundCollateralStatusView | null): status is InboundCollateralStatusView => !!status &&
    status.owner.toText() === ctx.expectedPrincipalText &&
    status.ledger.toText() === ledgerCanisterId &&
    status.request_id === requestId &&
    status.amount_raw === collateralAmountRaw &&
    'AddMargin' in status.operation &&
    status.operation.AddMargin.vault_id === BigInt(vaultId);
  const complete = (status: InboundCollateralStatusView | null) => matches(status) &&
    'Complete' in status.phase && status.result[0] !== undefined && 'AddMargin' in status.result[0];
  const rejectedMessage = (status: InboundCollateralStatusView | null) =>
    matches(status) && 'Rejected' in status.phase && status.result[0] && 'Rejected' in status.result[0]
      ? status.result[0].Rejected.message : 'The backend recorded this collateral request as rejected.';

  if (requestId <= 0n || !Number.isSafeInteger(vaultId) || vaultId <= 0 || collateralAmountRaw <= 0n) {
    return result('predispatch_aborted', null, 'Invalid collateral request ID, vault, or amount.');
  }
  const minimumDeposit = collateralInfo?.minCollateralDeposit ?? 0;
  if (minimumDeposit > 0 && collateralAmountRaw < BigInt(minimumDeposit)) {
    return result('predispatch_aborted', null, `Amount too low. Minimum required: ${minimumDeposit} raw units.`);
  }

  let exactStatus: InboundCollateralStatusView | null = null;
  let existingPendingRequest = false;
  try {
    assertActionBoundContextCurrent(ctx);
    const state = await ApiClient.getCollateralIngressStateBound(ctx, ledgerCanisterId);
    assertActionBoundContextCurrent(ctx);
    const active = state.active_request[0] ?? null;
    const latest = state.latest_result[0] ?? null;
    exactStatus = [active, latest].find((view) => view?.request_id === requestId) ?? null;
    if (exactStatus && !matches(exactStatus)) {
      return result('predispatch_aborted', exactStatus, 'This request ID is bound to different collateral arguments. No approval or top-up was submitted.');
    }
    if (complete(exactStatus)) return result('dispatched_ok', exactStatus, null);
    if (exactStatus && 'Rejected' in exactStatus.phase) {
      return result('predispatch_aborted', exactStatus, rejectedMessage(exactStatus));
    }
    existingPendingRequest = exactStatus !== null;
    if (active && active.request_id !== requestId) {
      return result('predispatch_aborted', active, 'Another collateral request is active for this ledger. No approval or top-up was submitted.');
    }
    if (!exactStatus && requestId !== state.next_request_id) {
      return result('predispatch_aborted', null, 'The request ID is old, missing, or not the next owner-scoped ID. Reconcile the journal before adding collateral.');
    }

    if (!existingPendingRequest) {
      const requiredAllowance = collateralAmountRaw + BigInt(collateralInfo?.ledgerFee ?? 10_000);
      const currentAllowance = await walletOperations.checkCollateralAllowanceBound(ctx, CONFIG.currentCanisterId, ledgerCanisterId);
      assertActionBoundContextCurrent(ctx);
      if (currentAllowance < requiredAllowance) {
        let needsApproval = true;
        if (approvalWasPreviouslyDispatched) {
          if (confirmAmbiguousApprovalRetry?.() !== true) {
            return result('predispatch_aborted', null, `A prior approval may have landed, but current ${symbol} allowance is still below the required amount. Confirm an approval retry to continue; no collateral top-up was submitted.`);
          }
          assertActionBoundContextCurrent(ctx);

          // The confirmation may take time. Re-read the durable journal and live allowance
          // immediately before a second approval so an exact pending row is replayed without
          // another approval, and a newly sufficient allowance is reused as-is.
          const retryState = await ApiClient.getCollateralIngressStateBound(ctx, ledgerCanisterId);
          assertActionBoundContextCurrent(ctx);
          const retryActive = retryState.active_request[0] ?? null;
          const retryLatest = retryState.latest_result[0] ?? null;
          const retryStatus = [retryActive, retryLatest].find((view) => view?.request_id === requestId) ?? null;
          if (retryStatus && !matches(retryStatus)) {
            return result('predispatch_aborted', retryStatus, 'The request ID is bound to different collateral arguments. No second approval or top-up was submitted.');
          }
          if (complete(retryStatus)) return result('dispatched_ok', retryStatus, null);
          if (retryStatus && 'Rejected' in retryStatus.phase) {
            return result('predispatch_aborted', retryStatus, rejectedMessage(retryStatus));
          }
          if (retryActive && retryActive.request_id !== requestId) {
            return result('predispatch_aborted', retryActive, 'Another collateral request became active. No second approval or top-up was submitted.');
          }
          if (!retryStatus && requestId !== retryState.next_request_id) {
            return result('predispatch_aborted', null, 'The request ID is no longer the next owner-scoped ID. Reconcile the journal before retrying approval.');
          }
          if (retryStatus) {
            // A matching Pending/Held row owns the exact pull tuple. Retry it without a new
            // approval; the backend journal, not allowance or balance deltas, decides its state.
            existingPendingRequest = true;
            needsApproval = false;
          } else {
            const retryAllowance = await walletOperations.checkCollateralAllowanceBound(ctx, CONFIG.currentCanisterId, ledgerCanisterId);
            assertActionBoundContextCurrent(ctx);
            if (retryAllowance < requiredAllowance) {
              // Fall through to exactly one user-confirmed approval. Its amount includes the
              // approval fee and the subsequent transfer fee.
            } else {
              // Another approval may have completed while the confirmation was open.
              needsApproval = false;
            }
          }
        }
        if (needsApproval) {
          if (isOisyWallet()) {
            assertActionBoundContextCurrent(ctx);
            const signerAgent = await pnp.getSignerAgent();
            assertActionBoundContextCurrent(ctx);
            if (!signerAgent) return result('predispatch_aborted', null, 'Oisy signer is unavailable before approval.');
            const ledgerActor = await walletStore.getActor(ledgerCanisterId, CONFIG.icp_ledgerIDL) as any;
            assertActionBoundContextCurrent(ctx);
            beforeApprovalDispatch?.();
            approvalDispatched = true;
            const approveResult = await ledgerActor.icrc2_approve({
              amount: collateralAmountRaw + BigInt(collateralInfo?.ledgerFee ?? 10_000) * 2n,
              spender: { owner: Principal.fromText(CONFIG.currentCanisterId), subaccount: [] },
              expires_at: largeApprovalExpiry(), expected_allowance: [], memo: [], fee: [], from_subaccount: [], created_at_time: [],
            });
            assertActionBoundContextCurrent(ctx);
            if (approveResult && 'Err' in approveResult) {
              return result('predispatch_aborted', null, `${symbol} approval failed: ${JSON.stringify(approveResult.Err)}. Its final allowance effect is not assumed; reconcile before retrying.`);
            }
          } else {
            beforeApprovalDispatch?.();
            approvalDispatched = true;
            const approval = await walletOperations.approveCollateralTransferBound(
              ctx,
              collateralAmountRaw + BigInt(collateralInfo?.ledgerFee ?? 10_000) * 2n,
              CONFIG.currentCanisterId,
              ledgerCanisterId,
            );
            assertActionBoundContextCurrent(ctx);
            if (!approval.success) return result('predispatch_aborted', null, `${approval.error || `Failed to approve ${symbol} transfer`}. Its final allowance effect is not assumed; reconcile before retrying.`);
          }
          const allowanceAfterApproval = await walletOperations.checkCollateralAllowanceBound(ctx, CONFIG.currentCanisterId, ledgerCanisterId);
          assertActionBoundContextCurrent(ctx);
          if (allowanceAfterApproval < requiredAllowance) {
            return result('predispatch_aborted', null, `Approval was submitted, but allowance is still below the required ${symbol} amount. No top-up was submitted.`);
          }
        }
      }
    }
  } catch (error) {
    return result('predispatch_aborted', exactStatus, error instanceof Error ? error.message : 'Unable to preflight collateral request.');
  }

  let actor: _SERVICE;
  try {
    assertActionBoundContextCurrent(ctx);
    actor = await ApiClient.getBoundAuthenticatedActor(ctx);
    assertActionBoundContextCurrent(ctx);
  } catch (error) {
    return result('predispatch_aborted', exactStatus, error instanceof Error ? error.message : 'Session changed before top-up dispatch.');
  }

  let response;
  try {
    assertActionBoundContextCurrent(ctx);
    response = await actor.add_margin_v2(requestId, { vault_id: BigInt(vaultId), amount: collateralAmountRaw });
    assertActionBoundContextCurrent(ctx);
  } catch (error) {
    let status: InboundCollateralStatusView | null = null;
    try { status = await ApiClient.getCollateralIngressBound(ctx, ledgerCanisterId, requestId); } catch { /* remains unresolved */ }
    return result(complete(status) ? 'dispatched_ok' : 'ambiguous_transport', status,
      complete(status) ? null : error instanceof Error ? error.message : 'Top-up reply was lost or unresolved.');
  }

  if ('Ok' in response) {
    const returnedStatus = response.Ok;
    if (matches(returnedStatus)) return result('dispatched_ok', returnedStatus, null);
    let status: InboundCollateralStatusView | null = null;
    try { status = await ApiClient.getCollateralIngressBound(ctx, ledgerCanisterId, requestId); } catch { /* remains unresolved */ }
    return result(complete(status) ? 'dispatched_ok' : 'ambiguous_transport', status,
      complete(status) ? null : 'The backend returned a collateral row that does not match this owner, request, ledger, vault, and amount. Reconcile the exact request.');
  }
  let status: InboundCollateralStatusView | null = null;
  try { status = await ApiClient.getCollateralIngressBound(ctx, ledgerCanisterId, requestId); } catch { /* keep journal unresolved */ }
  if (complete(status)) return result('dispatched_ok', status, null);
  return result('dispatched_err', status, matches(status) && 'Rejected' in status.phase
    ? rejectedMessage(status) : ApiClient.formatProtocolError(response.Err));
}

static async openVaultAndBorrowBound(
  ctx: ActionBoundContext,
  collateralAmountRaw: bigint,
  icusdAmountRaw: bigint,
  collateralTypePrincipal?: string
): Promise<BoundOpenVaultAndBorrowResult> {
  const ctPrincipal = collateralTypePrincipal || CANISTER_IDS.ICP_LEDGER;
  const collateralInfo = collateralStore.getCollateralInfo(ctPrincipal);
  const ledgerCanisterId = collateralInfo?.ledgerCanisterId ?? CONFIG.currentIcpLedgerId;
  const symbol = collateralInfo?.symbol ?? 'ICP';

  const submitted = { submittedCollateralRaw: collateralAmountRaw, submittedIcusdRaw: icusdAmountRaw };
  const abort = (errorMessage: string, approvalMayHaveMutated: boolean): BoundOpenVaultAndBorrowResult => ({
    kind: 'predispatch_aborted',
    vaultId: null,
    blockIndex: null,
    partialZeroDebtVaultId: null,
    errorMessage,
    approvalMayHaveMutated,
    ...submitted,
  });

  if (collateralAmountRaw <= 0n) {
    return abort('Invalid collateral amount.', false);
  }
  if (icusdAmountRaw <= 0n) {
    return abort('Invalid borrowing amount.', false);
  }
  const minDeposit = collateralInfo?.minCollateralDeposit ?? 0;
  if (minDeposit > 0 && collateralAmountRaw < BigInt(minDeposit)) {
    return abort(`Amount too low. Minimum required: ${minDeposit} raw units`, false);
  }

  let approvalDispatched = false;
  let actor!: _SERVICE;
  let collateralTypeOpt!: [] | [Principal];

  try {
    assertActionBoundContextCurrent(ctx);

    collateralTypeOpt = ctPrincipal === CANISTER_IDS.ICP_LEDGER ? [] : [Principal.fromText(ctPrincipal)];

    // ─── Oisy ICRC-112 batched path ───
    const signerAgent = isOisyWallet() ? await pnp.getSignerAgent() : null;
    assertActionBoundContextCurrent(ctx);

    if (signerAgent) {
      const oisyLedgerFee = BigInt(collateralInfo?.ledgerFee ?? 10_000);
      const requestedAllowance = collateralAmountRaw + oisyLedgerFee * 2n;

      const ledgerActor = await walletStore.getActor(ledgerCanisterId, CONFIG.icp_ledgerIDL) as any;
      assertActionBoundContextCurrent(ctx);

      approvalDispatched = true;
      const approveResult = await ledgerActor.icrc2_approve({
        amount: requestedAllowance,
        spender: { owner: Principal.fromText(CONFIG.currentCanisterId), subaccount: [] },
        expires_at: [],
        expected_allowance: [],
        memo: [],
        fee: [],
        from_subaccount: [],
        created_at_time: []
      });
      assertActionBoundContextCurrent(ctx);

      if (approveResult && 'Err' in approveResult) {
        return abort(`${symbol} approval failed: ${JSON.stringify(approveResult.Err)}`, true);
      }

      actor = await ApiClient.getBoundAuthenticatedActor(ctx);
      assertActionBoundContextCurrent(ctx);
    } else {
      // ─── Standard ICRC-2 path (Plug, II, etc.) ───
      const spenderCanisterId = CONFIG.currentCanisterId;
      const currentAllowance = await walletOperations.checkCollateralAllowanceBound(ctx, spenderCanisterId, ledgerCanisterId);
      assertActionBoundContextCurrent(ctx);

      const ledgerFee = BigInt(collateralInfo?.ledgerFee ?? 10_000);
      const requiredAllowance = collateralAmountRaw + ledgerFee;
      if (currentAllowance < requiredAllowance) {
        const requestedAllowance = collateralAmountRaw + ledgerFee * 2n;
        approvalDispatched = true;
        const approvalResult = await walletOperations.approveCollateralTransferBound(
          ctx, requestedAllowance, spenderCanisterId, ledgerCanisterId
        );
        assertActionBoundContextCurrent(ctx);
        if (!approvalResult.success) {
          return abort(approvalResult.error || `Failed to approve ${symbol} transfer`, true);
        }
      }

      actor = await ApiClient.getBoundAuthenticatedActor(ctx);
      assertActionBoundContextCurrent(ctx);
    }
  } catch (err) {
    if (err instanceof StaleActionSessionError) {
      return abort(err.message, approvalDispatched);
    }
    return abort(err instanceof Error ? err.message : 'Unknown error before dispatch', approvalDispatched);
  }

  // Final checkpoint immediately before the actual mutating call.
  try {
    assertActionBoundContextCurrent(ctx);
  } catch (err) {
    return abort(err instanceof Error ? err.message : 'Session changed before dispatch.', approvalDispatched);
  }

  const timeoutPromise = new Promise<never>((_, reject) => {
    setTimeout(() => reject(new Error('Wallet signature request timed out')), 60000);
  });

  let result: any;
  try {
    result = await Promise.race([
      actor.open_vault_and_borrow(collateralAmountRaw, icusdAmountRaw, collateralTypeOpt),
      timeoutPromise,
    ]);
  } catch (dispatchErr) {
    // Dispatched but no typed backend result observed — never proof of no mutation.
    return {
      kind: 'ambiguous_transport',
      vaultId: null,
      blockIndex: null,
      partialZeroDebtVaultId: null,
      errorMessage: dispatchErr instanceof Error ? dispatchErr.message : 'Network error after dispatch.',
      approvalMayHaveMutated: approvalDispatched,
      ...submitted,
    };
  }

  if ('Ok' in result) {
    return {
      kind: 'dispatched_ok',
      vaultId: Number(result.Ok.vault_id),
      blockIndex: Number(result.Ok.block_index),
      partialZeroDebtVaultId: null,
      errorMessage: null,
      approvalMayHaveMutated: approvalDispatched,
      ...submitted,
    };
  }

  return {
    kind: 'dispatched_err',
    vaultId: null,
    blockIndex: null,
    partialZeroDebtVaultId: ApiClient.extractPartialZeroDebtVaultId(result.Err),
    errorMessage: ApiClient.formatProtocolError(result.Err),
    approvalMayHaveMutated: approvalDispatched,
    ...submitted,
  };
}

/**
 * Bound sibling of borrowFromVault — the "finish borrow" leg for an already-open, zero-debt
 * vault (the partial_zero_debt recovery path). Same raw-amount and ctx-recheck discipline as
 * openVaultAndBorrowBound; no separate approval sub-step exists here, so a
 * predispatch_aborted result always means nothing at all was submitted.
 */
static async borrowFromVaultBound(
  ctx: ActionBoundContext,
  vaultId: number,
  icusdAmountRaw: bigint
): Promise<BoundBorrowFromVaultResult> {
  const abort = (errorMessage: string): BoundBorrowFromVaultResult => ({
    kind: 'predispatch_aborted',
    vaultId,
    blockIndex: null,
    feePaidRaw: null,
    errorMessage,
    submittedIcusdRaw: icusdAmountRaw,
  });

  if (icusdAmountRaw <= 0n) {
    return abort(`Invalid borrowing amount: ${icusdAmountRaw.toString()}. Amount must be a positive integer.`);
  }
  if (icusdAmountRaw < BigInt(MIN_ICUSD_AMOUNT)) {
    return abort(`Amount too low. Minimum borrowing amount: ${MIN_ICUSD_AMOUNT / E8S} icUSD`);
  }

  let actor: _SERVICE;
  try {
    assertActionBoundContextCurrent(ctx);
    actor = await ApiClient.getBoundAuthenticatedActor(ctx);
    assertActionBoundContextCurrent(ctx);
  } catch (err) {
    return abort(err instanceof Error ? err.message : 'Unknown error before dispatch');
  }

  const vaultArg = { vault_id: BigInt(vaultId), amount: icusdAmountRaw };

  let result: any;
  try {
    result = await actor.borrow_from_vault(vaultArg);
  } catch (dispatchErr) {
    return {
      kind: 'ambiguous_transport',
      vaultId,
      blockIndex: null,
      feePaidRaw: null,
      errorMessage: dispatchErr instanceof Error ? dispatchErr.message : 'Network error after dispatch.',
      submittedIcusdRaw: icusdAmountRaw,
    };
  }

  if ('Ok' in result) {
    return {
      kind: 'dispatched_ok',
      vaultId,
      blockIndex: Number(result.Ok.block_index),
      feePaidRaw: BigInt(result.Ok.fee_amount_paid),
      errorMessage: null,
      submittedIcusdRaw: icusdAmountRaw,
    };
  }

  return {
    kind: 'dispatched_err',
    vaultId,
    blockIndex: null,
    feePaidRaw: null,
    errorMessage: ApiClient.formatProtocolError(result.Err),
    submittedIcusdRaw: icusdAmountRaw,
  };
}


/**
 * Add Margin (collateral) to a vault.
 * @param vaultId The vault to add collateral to
 * @param collateralAmount Amount in human-readable units
 * @param collateralTypePrincipal Optional: the collateral type principal. If omitted, looks up from vault data or defaults to ICP.
 */
static async addMarginToVault(vaultId: number, collateralAmount: number, collateralTypePrincipal?: string, actionContext?: ActionBoundContext): Promise<VaultOperationResult> {
  return ApiClient.executeSequentialOperation(async () => {
    try {
      const assertCurrent = () => {
        if (actionContext) assertActionBoundContextCurrent(actionContext);
      };
      assertCurrent();
      // Resolve collateral info — try the provided principal, or look up from vault, or default to ICP
      let ctPrincipal = collateralTypePrincipal;
      if (!ctPrincipal) {
        // Try to get collateral type from the user's vault data
        const vault = await ApiClient.getVaultById(vaultId);
        assertCurrent();
        ctPrincipal = vault?.collateralType || CANISTER_IDS.ICP_LEDGER;
      }
      const ctInfo = collateralStore.getCollateralInfo(ctPrincipal);
      const ctDecimals = ctInfo?.decimals ?? 8;
      const ctDecimalsFactor = Math.pow(10, ctDecimals);
      const ledgerCanisterId = ctInfo?.ledgerCanisterId ?? CONFIG.currentIcpLedgerId;
      const symbol = ctInfo?.symbol ?? 'ICP';

      console.log(`Adding ${collateralAmount} ${symbol} to vault #${vaultId}`);

      const minDeposit = ctInfo?.minCollateralDeposit ?? 0;
      if (minDeposit > 0 && collateralAmount * ctDecimalsFactor < minDeposit) {
        return {
          success: false,
          error: `Amount too low. Minimum required: ${minDeposit / ctDecimalsFactor} ${symbol}`
        };
      }
      const amountRaw = BigInt(Math.floor(collateralAmount * ctDecimalsFactor));
      const bufferAmount = amountRaw * BigInt(120) / BigInt(100); // 20% buffer

      // Check if user has sufficient balance (only works for ICP via wallet store)
      // Skip for Oisy — async calls burn user gesture context needed for signer popup.
      // The canister validates balance anyway.
      if (ctPrincipal === CANISTER_IDS.ICP_LEDGER && !isOisyWallet()) {
        const hasSufficientBalance = await walletOperations.checkSufficientBalance(Number(bufferAmount) / ctDecimalsFactor);
        assertCurrent();
        if (!hasSufficientBalance) {
          return {
            success: false,
            error: `Insufficient ${symbol} balance. Please ensure you have at least ${collateralAmount} ${symbol} available.`
          };
        }
      }

      const actor = actionContext
        ? await ApiClient.getBoundAuthenticatedActor(actionContext)
        : await ApiClient.getAuthenticatedActor();
      assertCurrent();

      // Snapshot pre-add collateral for the Oisy false-negative verifier.
      // We read RAW token units so we can compare with BigInt arithmetic.
      // Oisy reads the warm sync cache (no network await inside the click
      // gesture window); non-Oisy awaits a fresh snapshot.
      const beforeCollateral = isOisyWallet() ? ApiClient.getCachedRawCollateralAmount(vaultId) : await ApiClient.getRawCollateralAmount(vaultId);
      assertCurrent();

      // ─── Oisy ICRC-112 batched path ───
      // Batches approve + add_margin into a single signer popup via ICRC-112.
      const marginSignerAgent = isOisyWallet() ? await pnp.getSignerAgent() : null;
      assertCurrent();

      if (marginSignerAgent) {
        console.log(`[Oisy] Sequential approve + add_margin for vault #${vaultId}`);

        const ledgerActor = await walletStore.getActor(ledgerCanisterId, CONFIG.icp_ledgerIDL) as any;
        assertCurrent();

        // 1) Approve (first Oisy consent screen).
        const approveResult = await ledgerActor.icrc2_approve({
          amount: bufferAmount,
          spender: {
            owner: Principal.fromText(CONFIG.currentCanisterId),
            subaccount: []
          },
          expires_at: [],
          expected_allowance: [],
          memo: [],
          fee: [],
          from_subaccount: [],
          created_at_time: []
        });
        assertCurrent();
        if (approveResult && 'Err' in approveResult) {
          return {
            success: false,
            error: `${symbol} approval failed: ${JSON.stringify(approveResult.Err)}`
          };
        }

        // 2) add_margin_to_vault (second consent screen), guarded against _arr.
        assertCurrent();
        const marginResult = await callWithOisyFalseNegativeGuard(
          () => actor.add_margin_to_vault({
            vault_id: BigInt(vaultId),
            amount: amountRaw
          }),
          async () => {
            if (beforeCollateral === null) return false;
            const after = await ApiClient.getRawCollateralAmount(vaultId);
            if (after === null) return false;
            return after - beforeCollateral >= (amountRaw * 95n) / 100n;
          },
          `Oisy add_margin ${collateralAmount} ${symbol} to vault #${vaultId}`
        );
        try { assertCurrent(); }
        catch { throw new Error('The add-margin call may have completed under the previous wallet. Check that wallet’s vault before retrying.'); }

        if (isOisyLandedSentinel(marginResult)) {
          return {
            success: true,
            vaultId,
            blockIndex: undefined,
            oisyResilient: true,
          };
        }

        if ('Ok' in marginResult) {
          return {
            success: true,
            vaultId,
            blockIndex: Number(marginResult.Ok)
          };
        } else {
          return {
            success: false,
            error: ApiClient.formatProtocolError(marginResult.Err)
          };
        }
      }

      // ─── Standard ICRC-2 path (Plug, II, etc.) ───
      // First check current allowance
      const spenderCanisterId = CONFIG.currentCanisterId;
      let currentAllowance;

      try {
        currentAllowance = actionContext
          ? await walletOperations.checkCollateralAllowanceBound(actionContext, spenderCanisterId, ledgerCanisterId)
          : await walletOperations.checkCollateralAllowance(spenderCanisterId, ledgerCanisterId);
        assertCurrent();
        console.log(`Current ${symbol} allowance:`, currentAllowance.toString());
      } catch (err) {
        console.error('Error checking allowance:', err);
        return {
          success: false,
          error: 'Failed to check token allowance. Please ensure your wallet is connected and try again.'
        };
      }

      if (currentAllowance < amountRaw) {
        console.log('Insufficient allowance, requesting approval...');
        console.log(`Requesting ${bufferAmount} raw (original: ${amountRaw} raw)`);

        try {
          assertCurrent();
          const approvalResult = actionContext
            ? await walletOperations.approveCollateralTransferBound(actionContext, bufferAmount, spenderCanisterId, ledgerCanisterId)
            : await walletOperations.approveCollateralTransfer(bufferAmount, spenderCanisterId, ledgerCanisterId);
          assertCurrent();

          if (!approvalResult.success) {
            return {
              success: false,
              error: approvalResult.error || `Failed to approve ${symbol} transfer`
            };
          }

          // Short delay to allow approval to be processed
          await new Promise(resolve => setTimeout(resolve, 2000));
          assertCurrent();

          // Verify approval worked
          const newAllowance = actionContext
            ? await walletOperations.checkCollateralAllowanceBound(actionContext, spenderCanisterId, ledgerCanisterId)
            : await walletOperations.checkCollateralAllowance(spenderCanisterId, ledgerCanisterId);
          assertCurrent();
          console.log('New allowance after approval:', newAllowance.toString());

          if (newAllowance < amountRaw) {
            return {
              success: false,
              error: `Approval did not complete successfully. Required: ${amountRaw}, Got: ${newAllowance}`
            };
          }
        } catch (approvalErr) {
          console.error('Approval error:', approvalErr);
          return {
            success: false,
            error: approvalErr instanceof Error ?
              approvalErr.message : 'Unknown error during approval'
          };
        }
      } else {
        console.log(`Current allowance ${currentAllowance} is sufficient for amount ${amountRaw}`);

        // If allowance is just barely enough, still request a higher allowance
        if (currentAllowance < bufferAmount) {
          console.log('Existing allowance is close to required amount, increasing for safety');
          try {
            assertCurrent();
            const approvalResult = actionContext
              ? await walletOperations.approveCollateralTransferBound(actionContext, bufferAmount, spenderCanisterId, ledgerCanisterId)
              : await walletOperations.approveCollateralTransfer(bufferAmount, spenderCanisterId, ledgerCanisterId);
            assertCurrent();

            if (approvalResult.success) {
              console.log('Successfully increased allowance for future operations');
              await new Promise(resolve => setTimeout(resolve, 2000));
              assertCurrent();
            } else {
              console.warn('Failed to increase allowance, but continuing with existing allowance');
            }
          } catch (err) {
            console.warn('Error increasing allowance, but continuing with existing allowance:', err);
          }
        }
      }

      // Now proceed with adding margin
      const vaultArg = {
        vault_id: BigInt(vaultId),
        amount: amountRaw // Use the original amount for the actual operation
      };

      console.log('Calling add_margin_to_vault with args:', {
        vault_id: vaultArg.vault_id.toString(),
        amount: vaultArg.amount.toString()
      });

      assertCurrent();
      const result = await callWithOisyFalseNegativeGuard(
        () => actor.add_margin_to_vault(vaultArg),
        async () => {
          if (beforeCollateral === null) return false;
          const after = await ApiClient.getRawCollateralAmount(vaultId);
          if (after === null) return false;
          return after - beforeCollateral >= (amountRaw * 95n) / 100n;
        },
        `add_margin ${collateralAmount} ${symbol} to vault #${vaultId}`
      );
      try { assertCurrent(); }
      catch { throw new Error('The add-margin call may have completed under the previous wallet. Check that wallet’s vault before retrying.'); }

      if (isOisyLandedSentinel(result)) {
        return {
          success: true,
          vaultId,
          blockIndex: undefined,
          oisyResilient: true,
        };
      }

      if ('Ok' in result) {
        return {
          success: true,
          vaultId,
          blockIndex: Number(result.Ok)
        };
      } else {
        return {
          success: false,
          error: ApiClient.formatProtocolError(result.Err)
        };
      }
    } catch (err) {
      console.error('Error adding margin to vault:', err);
      return {
        success: false,
        error: err instanceof Error ? err.message : 'Unknown error adding margin'
      };
    }
    // REMOVED: Don't use finally block with manual timestamp deletion
  }, vaultId); // Pass vaultId here to let executeSequentialOperation handle tracking
}
  
/** Compatibility entry point for the disabled no-ID legacy repayment route. */
static async getRepaymentV2RequestStateBound(ctx: ActionBoundContext): Promise<RepaymentV2RequestState> {
  assertActionBoundContextCurrent(ctx);
  const actor = await ApiClient.getBoundAuthenticatedActor(ctx);
  assertActionBoundContextCurrent(ctx);
  const response = await actor.get_my_repayment_v2_request_state();
  assertActionBoundContextCurrent(ctx);
  if ('Err' in response) throw new Error(ApiClient.formatProtocolError(response.Err));
  return response.Ok;
}

static async getRepaymentV2StatusBound(ctx: ActionBoundContext, requestId: bigint): Promise<RepaymentV2StatusView | null> {
  assertActionBoundContextCurrent(ctx);
  const actor = await ApiClient.getBoundAuthenticatedActor(ctx);
  assertActionBoundContextCurrent(ctx);
  const response = await actor.get_my_repayment_v2_status(requestId);
  assertActionBoundContextCurrent(ctx);
  if ('Err' in response) throw new Error(ApiClient.formatProtocolError(response.Err));
  return response.Ok[0] ?? null;
}

/**
 * Reconcile and submit one exact owner-global repayment request. Existing rows
 * are replayed without another approval; a missing compacted/old ID is never
 * treated as permission to allocate or dispatch a new payment.
 */
static async repayV2Bound(
  ctx: ActionBoundContext,
  intent: RepaymentV2Intent,
  beforeApprovalDispatch: () => void,
  confirmAmbiguousApprovalRetry: () => boolean,
  beforeBackendDispatch: () => void,
): Promise<BoundRepaymentV2Result> {
  let approvalMayHaveMutated = false;
  const result = (kind: BoundActionOutcomeKind, status: RepaymentV2StatusView | null, errorMessage: string | null) => ({
    kind, status, errorMessage, approvalMayHaveMutated,
  });
  const requestId = BigInt(intent.requestId);
  const amountRaw = BigInt(intent.requestedAmountRaw);
  const ledgerText = CONFIG.currentIcusdLedgerId;
  if (ctx.expectedPrincipalText !== intent.owner || requestId <= 0n || amountRaw <= 0n || !/^\d+$/.test(intent.vaultId)) {
    return result('predispatch_aborted', null, 'The saved repayment intent does not match this wallet or has invalid raw arguments.');
  }

  let state: RepaymentV2RequestState;
  let status: RepaymentV2StatusView | null;
  try {
    status = await ApiClient.getRepaymentV2StatusBound(ctx, requestId);
    state = await ApiClient.getRepaymentV2RequestStateBound(ctx);
  } catch (error) {
    return result('predispatch_aborted', null, `Could not preflight the owner repayment journal; no approval or repayment was submitted. ${error instanceof Error ? error.message : ''}`);
  }
  const active = state.active_request[0] ?? null;
  const latest = state.latest_result[0] ?? null;
  status = status ?? [active, latest].find((row) => row?.request_id === requestId) ?? null;
  const matches = (row: RepaymentV2StatusView | null): row is RepaymentV2StatusView => !!row && repaymentV2StatusMatchesIntent(row, intent, ledgerText);
  if (status && !matches(status)) {
    return result('predispatch_aborted', status, 'This request ID is bound to different owner, ledger, vault, amount, or close arguments. No approval or payment was submitted.');
  }
  if (active && active.request_id !== requestId) {
    return result('predispatch_aborted', active, 'Another owner-global repayment is active. Reconcile that exact request before starting another repayment.');
  }
  if (!status && requestId !== state.next_request_id) {
    return result('predispatch_aborted', null, 'This repayment ID is no longer available in the retained journal. A missing old status is unknown, not proof of no payment; do not submit a replacement.');
  }

  if (status) {
    if ('Complete' in status.phase) return status.result[0]
      ? result('dispatched_ok', status, null)
      : result('ambiguous_transport', status, 'The journal marked repayment complete without a result receipt. Reconcile before proceeding.');
    if ('Rejected' in status.phase) return result('dispatched_err', status, status.last_error[0] ?? 'The exact repayment request was rejected before completion.');
    if ('CloseNeedsAdditionalRepayment' in status.phase) {
      return result('dispatched_ok', status, 'The repayment receipt is recorded, but accrued interest prevented closing. The vault remains open; any further repayment must use a new request ID.');
    }
  }

  const existingJournal = status !== null;
  try {
    if (!existingJournal) {
      const signerAgent = isOisyWallet() ? await pnp.getSignerAgent() : null;
      assertActionBoundContextCurrent(ctx);
      const ledgerFee = 100_000n;
      const requiredAllowance = amountRaw + ledgerFee * 2n;
      const currentAllowance = await walletOperations.checkIcusdAllowanceBound(ctx, CONFIG.currentCanisterId);
      const needsApproval = currentAllowance < requiredAllowance;
      if (needsApproval) {
        if (intent.approvalAttempted && !confirmAmbiguousApprovalRetry()) {
          return result('predispatch_aborted', null, 'The prior approval outcome is uncertain and the live allowance is still insufficient. No repayment was submitted.');
        }
        beforeApprovalDispatch();
        assertActionBoundContextCurrent(ctx);
        approvalMayHaveMutated = true;
        let approval: { success: boolean; error?: string };
        if (signerAgent) {
          const ledgerActor = await walletStore.getActor(CONFIG.currentIcusdLedgerId, CONFIG.icusd_ledgerIDL) as any;
          assertActionBoundContextCurrent(ctx);
          const response = await ledgerActor.icrc2_approve({
            amount: requiredAllowance,
            spender: { owner: Principal.fromText(CONFIG.currentCanisterId), subaccount: [] },
            expires_at: largeApprovalExpiry(), expected_allowance: [], memo: [], fee: [],
            from_subaccount: [], created_at_time: [],
          });
          assertActionBoundContextCurrent(ctx);
          approval = 'Err' in response
            ? { success: false, error: `icUSD approval failed: ${JSON.stringify(response.Err)}` }
            : { success: true };
        } else {
          approval = await walletOperations.approveIcusdTransferBound(
            ctx, requiredAllowance, CONFIG.currentCanisterId,
          );
        }
        if (!approval.success) return result('predispatch_aborted', null, approval.error || 'icUSD approval failed. Its outcome is retained for reconciliation.');
      }
    }
  } catch (error) {
    return result('predispatch_aborted', status, error instanceof Error ? error.message : 'Approval or journal preflight failed.');
  }

  let actor: _SERVICE;
  try {
    assertActionBoundContextCurrent(ctx);
    actor = await ApiClient.getBoundAuthenticatedActor(ctx);
    assertActionBoundContextCurrent(ctx);
  } catch (error) {
    return result('predispatch_aborted', status, error instanceof Error ? error.message : 'Session changed before repayment dispatch.');
  }
  const arg = { vault_id: BigInt(intent.vaultId), amount: amountRaw };
  try {
    beforeBackendDispatch();
    assertActionBoundContextCurrent(ctx);
    const response = intent.closeAfterRepay
      ? await actor.repay_and_close_vault_v2(requestId, arg)
      : await actor.repay_to_vault_v2(requestId, arg);
    assertActionBoundContextCurrent(ctx);
    if ('Ok' in response) {
      if (matches(response.Ok)) {
        const outcome = repaymentV2TransportOutcome(response.Ok);
        const errorMessage = outcome === 'dispatched_err'
          ? response.Ok.last_error[0] ?? 'The exact repayment request was rejected before completion.'
          : outcome === 'ambiguous_transport'
            ? 'The exact repayment request is recorded but remains pending or held.'
            : null;
        return result(outcome, response.Ok, errorMessage);
      }
      const exact = await ApiClient.getRepaymentV2StatusBound(ctx, requestId).catch(() => null);
      return result('ambiguous_transport', matches(exact) ? exact : null, 'The backend returned a status that does not match this exact repayment request. Reconcile before proceeding.');
    }
    const exact = await ApiClient.getRepaymentV2StatusBound(ctx, requestId).catch(() => null);
    if (matches(exact)) {
      const outcome = repaymentV2TransportOutcome(exact);
      if (outcome === 'dispatched_ok') return result(outcome, exact, null);
      if (outcome === 'ambiguous_transport') {
        return result(outcome, exact, 'The repayment call returned an error while the exact journal row remains pending or held. Reconcile before retrying.');
      }
    }
    return result('dispatched_err', matches(exact) ? exact : null, ApiClient.formatProtocolError(response.Err));
  } catch (error) {
    const exact = await ApiClient.getRepaymentV2StatusBound(ctx, requestId).catch(() => null);
    if (matches(exact)) {
      const outcome = repaymentV2TransportOutcome(exact);
      if (outcome === 'dispatched_ok') return result(outcome, exact, null);
      if (outcome === 'dispatched_err') {
        return result(outcome, exact, exact.last_error[0] ?? 'The exact repayment request was rejected before completion.');
      }
      return result(outcome, exact, error instanceof Error ? error.message : 'Repayment reply was lost; the exact request remains pending or held.');
    }
    return result('ambiguous_transport', null,
      error instanceof Error ? error.message : 'Repayment reply was lost; exact request remains unresolved.');
  }
}

static async repayToVault(vaultId: number, icusdAmount: number): Promise<VaultOperationResult> {
  // Legacy no-ID endpoint is fail-closed on the backend. Stop here so direct
  // callers cannot pay an approval fee before reaching that backend gate.
  void vaultId;
  void icusdAmount;
  return ApiClient.legacyIcusdRepaymentDisabled();
}

/** Compatibility entry point for the disabled no-ID legacy repay-and-close route. */
static async repayAndCloseVault(vaultId: number, icusdAmount: number): Promise<VaultOperationResult> {
  // This legacy no-ID path is gated before debit. Do not approve first; callers
  // must use the persisted, journal-bound V2 flow with a stable request ID.
  void vaultId;
  void icusdAmount;
  return ApiClient.legacyIcusdRepaymentDisabled();
}

static legacyIcusdRepaymentDisabled(): VaultOperationResult {
  return {
    success: false,
    error: 'Legacy icUSD repayment is disabled. Refresh the app and use the journaled V2 repayment flow; no approval or repayment was submitted.',
  };
}

static legacyStableRepaymentDisabled(): VaultOperationResult {
  return { success: false, error: 'Legacy stable repayment is disabled. Refresh the app and use the journaled V2 repayment flow; no approval or repayment was submitted.' };
}

static async getStableRepaymentV2RequestStateBound(ctx: ActionBoundContext): Promise<StableRepaymentV2RequestState> {
  assertActionBoundContextCurrent(ctx);
  const actor = await ApiClient.getBoundAuthenticatedActor(ctx);
  assertActionBoundContextCurrent(ctx);
  const response = await actor.get_my_stable_repayment_v2_request_state();
  assertActionBoundContextCurrent(ctx);
  if ('Err' in response) throw new Error(ApiClient.formatProtocolError(response.Err));
  return response.Ok;
}

static async getStableRepaymentV2StatusBound(ctx: ActionBoundContext, requestId: bigint): Promise<StableRepaymentV2StatusView | null> {
  assertActionBoundContextCurrent(ctx);
  const actor = await ApiClient.getBoundAuthenticatedActor(ctx);
  assertActionBoundContextCurrent(ctx);
  const response = await actor.get_my_stable_repayment_v2_status(requestId);
  assertActionBoundContextCurrent(ctx);
  return unwrapStableRepaymentV2Status(response);
}

/** Submit/replay one exact stable repayment journal row. This route never calls the legacy method. */
static async repayStableV2Bound(
  ctx: ActionBoundContext,
  intent: StableRepaymentV2Intent,
  beforeApprovalDispatch: () => void,
  confirmAmbiguousApprovalRetry: () => boolean,
  beforeBackendDispatch: () => void,
): Promise<{ kind: BoundActionOutcomeKind; status: StableRepaymentV2StatusView | null; errorMessage: string | null }> {
  const requestId = BigInt(intent.requestId);
  const amountRawE6 = BigInt(intent.amountRawE6);
  const ledgerText = CONFIG.getStableLedgerId(intent.token);
  const matches = (row: StableRepaymentV2StatusView | null): row is StableRepaymentV2StatusView => !!row &&
    row.owner.toText() === intent.owner && row.ledger.toText() === ledgerText && row.request_id === requestId &&
    row.vault_id === BigInt(intent.vaultId) && row.requested_amount_e8 === amountRawE6 * 100n &&
    row.principal_pull_e6 > 0n && (intent.token in row.token_type);
  const outcome = (kind: BoundActionOutcomeKind, status: StableRepaymentV2StatusView | null, errorMessage: string | null) => ({ kind, status, errorMessage });
  if (ctx.expectedPrincipalText !== intent.owner || requestId <= 0n || amountRawE6 <= 0n || !/^\d+$/.test(intent.vaultId))
    return outcome('predispatch_aborted', null, 'Saved stable repayment does not match this wallet or has invalid arguments.');

  let state: StableRepaymentV2RequestState;
  let status: StableRepaymentV2StatusView | null;
  try {
    status = await ApiClient.getStableRepaymentV2StatusBound(ctx, requestId);
    state = await ApiClient.getStableRepaymentV2RequestStateBound(ctx);
  } catch (error) {
    return outcome('predispatch_aborted', null, `Could not preflight the stable repayment journal; no approval or repayment was submitted. ${error instanceof Error ? error.message : ''}`);
  }
  const active = state.active_request[0] ?? null;
  for (const row of [active, state.latest_result[0] ?? null]) {
    if (!row) continue;
    const rowToken: StableRepaymentV2Token = 'CKUSDT' in row.token_type ? 'CKUSDT' : 'CKUSDC';
    if (row.owner.toText() !== intent.owner || row.ledger.toText() !== CONFIG.getStableLedgerId(rowToken)) {
      return outcome('predispatch_aborted', row, 'The stable repayment journal returned an unexpected owner or token ledger. No approval or repayment was submitted.');
    }
  }
  status = status ?? [active, state.latest_result[0] ?? null].find((row) => row?.request_id === requestId) ?? null;
  if (status && !matches(status)) return outcome('predispatch_aborted', status, 'This request ID is bound to a different owner, ledger, vault, amount, or token. No approval or repayment was submitted.');
  if (active && active.request_id !== requestId) return outcome('predispatch_aborted', active, 'Another owner-global stable repayment is active. Reconcile it before starting another repayment.');
  if (!status && requestId !== state.next_request_id) return outcome('predispatch_aborted', null, 'This old stable repayment ID is absent from the retained journal. Its outcome is unknown; do not replace it.');
  if (status) {
    const exactOutcome = stableRepaymentV2Outcome(status);
    if (exactOutcome === 'complete') return outcome('dispatched_ok', status, null);
    if (exactOutcome === 'rejected') return outcome('dispatched_err', status, status.last_error[0] ?? 'The stable repayment was rejected with no effect.');
  }

  // A journal row owns its precise pull tuple; replay it without another approval.
  if (!status) {
    try {
      const signer = isOisyWallet() ? await pnp.getSignerAgent() : null;
      assertActionBoundContextCurrent(ctx);
      const ledgerId = CONFIG.getStableLedgerId(intent.token);
      const wallet = get(walletStore).principal;
      if (!wallet || wallet.toText() !== intent.owner) return outcome('predispatch_aborted', null, 'Wallet changed before stable allowance preflight.');
      const allowance = await walletOperations.checkStableAllowance(CONFIG.currentCanisterId, intent.token);
      assertActionBoundContextCurrent(ctx);
      const protocolStatus = await QueryOperations.getProtocolStatus();
      assertActionBoundContextCurrent(ctx);
      const requiredAllowance = stableRepaymentV2RequiredAllowance(amountRawE6, protocolStatus.ckstableRepayFee || 0);
      if (allowance < requiredAllowance) {
        if (intent.approvalAttempted && !confirmAmbiguousApprovalRetry()) return outcome('predispatch_aborted', null, 'The previous approval may have landed, but allowance is still insufficient. No repayment was submitted.');
        beforeApprovalDispatch();
        assertActionBoundContextCurrent(ctx);
        let approval: { success: boolean; error?: string };
        if (signer) {
          const response = await (await walletStore.getActor(ledgerId, icusd_ledgerIDL) as any).icrc2_approve({ amount: requiredAllowance, spender: { owner: Principal.fromText(CONFIG.currentCanisterId), subaccount: [] }, expires_at: largeApprovalExpiry(), expected_allowance: [], memo: [], fee: [], from_subaccount: [], created_at_time: [] });
          approval = 'Err' in response ? { success: false, error: `${intent.token} approval failed: ${JSON.stringify(response.Err)}` } : { success: true };
        } else approval = await walletOperations.approveStableTransferBound(ctx, requiredAllowance, CONFIG.currentCanisterId, intent.token);
        assertActionBoundContextCurrent(ctx);
        if (!approval.success) return outcome('predispatch_aborted', null, approval.error ?? `${intent.token} approval failed.`);
      }
    } catch (error) {
      return outcome('predispatch_aborted', null, error instanceof Error ? error.message : 'Stable approval preflight failed.');
    }
  }

  try {
    assertActionBoundContextCurrent(ctx);
    const actor = await ApiClient.getBoundAuthenticatedActor(ctx);
    assertActionBoundContextCurrent(ctx);
    const token_type = intent.token === 'CKUSDT' ? { CKUSDT: null } as const : { CKUSDC: null } as const;
    beforeBackendDispatch();
    assertActionBoundContextCurrent(ctx);
    const response = await actor.repay_to_vault_with_stable_v2(requestId, { vault_id: BigInt(intent.vaultId), amount: amountRawE6 * 100n, token_type });
    assertActionBoundContextCurrent(ctx);
    if ('Ok' in response) {
      const returned = response.Ok;
      if (!matches(returned)) return outcome('ambiguous_transport', await ApiClient.getStableRepaymentV2StatusBound(ctx, requestId).catch(() => null), 'Backend returned a status for different repayment arguments.');
      const exactOutcome = stableRepaymentV2TransportOutcome(returned);
      if (exactOutcome === 'dispatched_ok') return outcome(exactOutcome, returned, null);
      if (exactOutcome === 'dispatched_err') return outcome(exactOutcome, returned, returned.last_error[0] ?? 'The stable repayment was rejected with no effect.');
      return outcome(exactOutcome, returned, 'The exact stable repayment request is pending or held. Replay only this request after checking its journal.');
    }
    const exact = await ApiClient.getStableRepaymentV2StatusBound(ctx, requestId).catch(() => null);
    if (matches(exact) && 'Rejected' in exact.phase) return outcome('dispatched_err', exact, exact.last_error[0] ?? ApiClient.formatProtocolError(response.Err));
    return outcome('ambiguous_transport', matches(exact) ? exact : null, ApiClient.formatProtocolError(response.Err));
  } catch (error) {
    const exact = await ApiClient.getStableRepaymentV2StatusBound(ctx, requestId).catch(() => null);
    if (matches(exact) && stableRepaymentV2Outcome(exact) === 'complete') return outcome('dispatched_ok', exact, null);
    if (matches(exact) && stableRepaymentV2Outcome(exact) === 'rejected') return outcome('dispatched_err', exact, exact.last_error[0] ?? 'The repayment was rejected.');
    return outcome('ambiguous_transport', matches(exact) ? exact : null, error instanceof Error ? error.message : 'Repayment reply was lost; the exact request remains unresolved.');
  }
}

/**
 * Compatibility wrapper for the disabled no-ID stable repayment route.
 * Stable repayments must use the owner-journaled V2 request flow.
 */
static async repayToVaultWithStable(
  vaultId: number,
  amount: number,
  tokenType: 'CKUSDT' | 'CKUSDC',
  onStage?: (stage: 'approval_attempted' | 'backend_dispatch_attempted') => void,
  actionContext?: ActionBoundContext,
  exactAmountRawE6?: bigint
): Promise<VaultOperationResult> {
  void vaultId; void amount; void tokenType; void onStage; void actionContext; void exactAmountRawE6;
  return ApiClient.legacyStableRepaymentDisabled();
}

  /**
   * Close a vault - with enhanced error handling for auto-removed vaults
   */
  static async closeVault(vaultId: number): Promise<VaultOperationResult> {
    return ApiClient.executeSequentialOperation(async () => {
      // REMOVE: this.operationTimestamps.set(vaultId, Date.now());

      try {
        console.log(`Closing vault #${vaultId}`);
        
        // Verify vault access first. Oisy reads the synchronous snapshot cache
        // and builds the actor directly — verifyVaultAccess does a getVaultById
        // query that would burn the gesture window and block the signer popup.
        let actor: any;
        let isEmpty: boolean;
        if (isOisyWallet()) {
          const snap = ApiClient.getCachedRawSnapshot(vaultId);
          if (!snap) {
            return { success: false, error: `Vault #${vaultId} status is not loaded. Refresh your vaults and try again.` };
          }
          actor = await ApiClient.getAuthenticatedActor();
          isEmpty = snap.icpMargin === 0n && snap.borrowedIcusd === 0n;
        } else {
          const access = await ApiClient.verifyVaultAccess(vaultId);
          if (access.error) {
            // Special case for already closed vaults
            if (access.error === 'Vault not found') {
              return {
                success: true,
                message: `Vault #${vaultId} is already closed.`,
                vaultId
              };
            }
            return { success: false, error: access.error };
          }
          actor = access.actor;
          isEmpty = access.vault.icpMargin === 0 && access.vault.borrowedIcusd === 0;
        }

        // If vault exists but has no collateral and no debt, treat as already closed
        if (isEmpty) {
          return {
            success: true,
            message: `Vault #${vaultId} is empty and will be automatically closed.`,
            vaultId
          };
        }
  
        // Execute close operation
        // close_vault deletes the vault from canister state when it has
        // no collateral and no debt. Verifier: vault no longer present.
        type CloseVaultResult = { Ok: [] | [bigint] } | { Err: ProtocolError };
        const result = await callWithOisyFalseNegativeGuard<CloseVaultResult>(
          () => actor.close_vault(BigInt(vaultId)) as Promise<CloseVaultResult>,
          async () => {
            const snap = await ApiClient.fetchVaultRawSnapshot(vaultId);
            // Either snapshot says vault is gone, or both collateral
            // and borrowed are now zero (canister may keep a husk).
            if (!snap) return false;
            if (!snap.exists) return true;
            return snap.collateralAmount === 0n && snap.borrowedIcusd === 0n;
          },
          `close_vault #${vaultId}`
        );

        if (isOisyLandedSentinel(result)) {
          return {
            success: true,
            vaultId,
            message: `Successfully closed vault #${vaultId}`,
            oisyResilient: true,
          };
        }

        if ('Ok' in result) {
          return {
            success: true,
            vaultId,
            message: `Successfully closed vault #${vaultId}`
          };
        }

        const errorMsg = ApiClient.formatProtocolError(result.Err);
        return {
          success: false,
          error: errorMsg
        };
        
      } catch (error) {
        // Handle specific error cases
        const errorMsg = error?.toString() || '';
        if (ApiClient.isVaultNotFoundError(errorMsg)) {
          return {
            success: true,
            message: `Vault #${vaultId} has already been closed.`,
            vaultId
          };
        }
        
        return {
          success: false,
          error: error instanceof Error ? error.message : 'Unknown error closing vault'
        };
      }
      // REMOVE: The finally block with timestamp deletion
    }, vaultId); // ADD: vaultId parameter here
  }

  /**
   * Clear the vault cache to force refresh on next request
   */
  static clearVaultCache(): void {
    ApiClient.vaultCache = {
      vaults: [],
      timestamp: 0,
      loading: false
    };
    // FE-002: drop the raw snapshots backing the Oisy `_arr` verifiers too,
    // so wallet connect/disconnect (which both call this) can't leave a
    // previous wallet's pre-op state behind.
    clearRawSnapshots();
  }

  // Cache to store user vaults data with timestamp
  private static vaultCache: {
    vaults: VaultDTO[],
    timestamp: number,
    loading: boolean  // Add loading flag to prevent duplicate requests
  } = {
    vaults: [],
    timestamp: 0,
    loading: false
  };

  /**
   * Get the current user's vaults with caching and request deduplication
   */
  static async getUserVaults(forceRefresh = false): Promise<VaultDTO[]> {
    const walletState = get(walletStore);
    const userPrincipal = walletState.principal;
    
    if (!userPrincipal) {
      return [];
    }

    const principalStr = userPrincipal.toString();
    const cacheKey = `get_vaults_${principalStr}`;
    
    // Use request deduplication to prevent multiple simultaneous calls
    return RequestDeduplicator.deduplicate(cacheKey, async () => {
      try {
        const now = Date.now();
        
        // Use cache if it's less than 5 seconds old and not forcing refresh
        if (!forceRefresh && 
            !ApiClient.vaultCache.loading &&
            now - ApiClient.vaultCache.timestamp < 5000 && 
            ApiClient.vaultCache.vaults.length > 0) {
          console.log('Using cached vaults:', ApiClient.vaultCache.vaults);
          return ApiClient.vaultCache.vaults;
        }
        
        // Set loading flag to prevent duplicate requests
        ApiClient.vaultCache.loading = true;
        
        // Use anonymous actor for read-only queries — avoids Oisy signer popups
        const principalForQuery = userPrincipal || Principal.fromText(principalStr);

        console.log(`Fetching vaults for principal: ${principalStr}`);
        const canisterVaults = await publicActor.get_vaults([principalForQuery]);
        console.log('Raw canister vaults data:', canisterVaults);

        // Keep the sync raw-snapshot caches warm so the Oisy click-path
        // verifiers can read pre-op state without a network await.
        warmRawSnapshots(principalStr, canisterVaults.map((cv: any) => [Number(cv.vault_id), { collateralAmount: BigInt(cv.collateral_amount), borrowedIcusd: BigInt(cv.borrowed_icusd_amount), icpMargin: BigInt(cv.icp_margin_amount) }]));
        
        // Get protocol status for ICP price calculation
        const status = await QueryOperations.getProtocolStatus();
        const icpPrice = status.lastIcpRate;
        
        // Transform canister vaults to our frontend model
        const vaults: VaultDTO[] = [];
        
        // Sort vaults by ID to ensure consistent ordering
        const sortedVaults = [...canisterVaults].sort((a, b) => 
          Number(a.vault_id) - Number(b.vault_id)
        );
        
        for (const v of sortedVaults) {
          // Create a display-friendly ID
          const vaultId = Number(v.vault_id);

          // Resolve collateral type
          const ctRaw = v.collateral_type.toText();
          const ctPrincipal = ctRaw === '2vxsx-fae' ? CANISTER_IDS.ICP_LEDGER : ctRaw;
          const ctInfo = collateralStore.getCollateralInfo(ctPrincipal);
          const ctDecimals = ctInfo?.decimals ?? 8;
          const ctDecimalsFactor = Math.pow(10, ctDecimals);
          const ctSymbol = ctInfo?.symbol ?? collateralStore.getCollateralSymbol(ctPrincipal);

          // Convert raw amounts to human-readable
          const collateralAmount = Number(v.collateral_amount) / ctDecimalsFactor;
          const borrowedIcusd = Number(v.borrowed_icusd_amount) / E8S;

          // icpMargin kept for backward compat (same value as collateralAmount for ICP vaults)
          const icpMargin = Number(v.icp_margin_amount) / E8S;

          console.log(`Processing vault #${vaultId}: ${ctSymbol}=${collateralAmount}, icUSD=${borrowedIcusd}`);

          vaults.push({
            vaultId,
            owner: v.owner.toString(),
            icpMargin,
            borrowedIcusd,
            timestamp: now,
            collateralType: ctPrincipal,
            collateralAmount,
            collateralSymbol: ctSymbol,
            collateralDecimals: ctDecimals,
            accruedInterest: Number((v as any).accrued_interest ?? 0),
          });
        }
        
        // Update cache
        ApiClient.vaultCache = {
          vaults,
          timestamp: now,
          loading: false
        };
        
        console.log('Processed vault DTOs:', vaults);
        return vaults;
      } catch (err) {
        console.error('Error getting user vaults:', err);
        // Clear loading flag on error
        ApiClient.vaultCache.loading = false;
        
        // Return cached data if available, even if it's stale
        if (ApiClient.vaultCache.vaults.length > 0) {
          console.warn('Returning stale cached vaults due to error');
          return ApiClient.vaultCache.vaults;
        }
        throw err;
      }
    });
  }

  /**
   * Withdraw collateral (ICP) from a vault
   */
  static async withdrawCollateral(vaultId: number): Promise<VaultOperationResult> {
    return ApiClient.executeSequentialOperation(async () => {
      // REMOVE: this.operationTimestamps.set(vaultId, Date.now());
      
      try {
        console.log(`Withdrawing collateral from vault #${vaultId}`);

        // Get the authenticated actor
        const actor = await ApiClient.getAuthenticatedActor();

        // Snapshot for the Oisy false-negative verifier. Full withdraw
        // takes collateral to zero (and may auto-close the vault).
        // Oisy reads the warm sync cache (no network await inside the click
        // gesture window); non-Oisy awaits a fresh snapshot.
        const beforeCollateral = isOisyWallet() ? ApiClient.getCachedRawCollateralAmount(vaultId) : await ApiClient.getRawCollateralAmount(vaultId);

        const result = await callWithOisyFalseNegativeGuard(
          () => actor.withdraw_collateral(BigInt(vaultId)),
          async () => {
            // Either vault is gone (auto-closed) or collateral dropped
            // by at least 95% of the pre-call value.
            const snap = await ApiClient.fetchVaultRawSnapshot(vaultId);
            if (!snap) return false;
            if (!snap.exists) return true;
            if (beforeCollateral === null) return false;
            const drop = beforeCollateral - snap.collateralAmount;
            return drop >= (beforeCollateral * 95n) / 100n;
          },
          `withdraw_collateral from vault #${vaultId}`
        );

        if (isOisyLandedSentinel(result)) {
          return {
            success: true,
            blockIndex: undefined,
            vaultId,
            message: `Successfully withdrew collateral from vault #${vaultId}. If the vault had no debt, it may have been automatically closed.`,
            oisyResilient: true,
          };
        }

        if ('Ok' in result) {
          const blockIndex = Number(result.Ok);

          // IMPORTANT: Add a note about the vault possibly being auto-closed
          return {
            success: true,
            blockIndex,
            vaultId,
            message: `Successfully withdrew collateral from vault #${vaultId}. If the vault had no debt, it may have been automatically closed.`
          };
        } else {
          return {
            success: false,
            error: ApiClient.formatProtocolError(result.Err)
          };
        }
      } catch (err) {
        console.error('Error withdrawing collateral:', err);
        return {
          success: false,
          error: err instanceof Error ? err.message : 'Unknown error withdrawing collateral'
        };
      }
      // REMOVE: finally block with timestamp deletion
    }, vaultId); // ADD: vaultId parameter here
  }

  /**
   * Withdraw partial collateral from a vault (keeps CR above minimum)
   * @param decimals Token decimal precision (default 8 for ICP)
   */
  static async withdrawPartialCollateral(vaultId: number, icpAmount: number, decimals: number = 8): Promise<VaultOperationResult> {
    return ApiClient.executeSequentialOperation(async () => {
      try {
        const factor = Math.pow(10, decimals);
        console.log(`Withdrawing ${icpAmount} collateral (decimals=${decimals}) from vault #${vaultId}`);

        // Min amount validated by backend (per-collateral min_collateral_deposit)

        const actor = await ApiClient.getAuthenticatedActor();
        const expectedDelta = BigInt(Math.floor(icpAmount * factor));
        const vaultArg = {
          vault_id: BigInt(vaultId),
          amount: expectedDelta
        };

        // Snapshot for the Oisy false-negative verifier.
        // Oisy reads the warm sync cache (no network await inside the click
        // gesture window); non-Oisy awaits a fresh snapshot.
        const beforeCollateral = isOisyWallet() ? ApiClient.getCachedRawCollateralAmount(vaultId) : await ApiClient.getRawCollateralAmount(vaultId);

        const result = await callWithOisyFalseNegativeGuard(
          () => actor.withdraw_partial_collateral(vaultArg),
          async () => {
            if (beforeCollateral === null) return false;
            const after = await ApiClient.getRawCollateralAmount(vaultId);
            if (after === null) return false;
            const drop = beforeCollateral - after;
            return drop >= (expectedDelta * 95n) / 100n;
          },
          `withdraw_partial_collateral ${icpAmount} from vault #${vaultId}`
        );

        if (isOisyLandedSentinel(result)) {
          return {
            success: true,
            vaultId,
            blockIndex: undefined,
            oisyResilient: true,
          };
        }

        if ('Ok' in result) {
          return {
            success: true,
            vaultId,
            blockIndex: Number(result.Ok)
          };
        } else {
          return {
            success: false,
            error: ApiClient.formatProtocolError(result.Err)
          };
        }
      } catch (err) {
        console.error('Error withdrawing partial collateral:', err);
        return {
          success: false,
          error: err instanceof Error ? err.message : 'Unknown error withdrawing collateral'
        };
      }
    }, vaultId);
  }

    /**
     * Get vault history
     */
    static async getVaultHistory(vaultId: number): Promise<any[]> {
      try {
        // Use anonymous actor for read-only queries — avoids Oisy signer popups.
        // Backend returns `(global_index, event)` tuples; legacy callers want a
        // flat event list, so drop the index here.
        const history = await publicActor.get_vault_history(BigInt(vaultId));
        return (history as any[]).map((pair: any) => pair[1]);
      } catch (err) {
        console.error('Error getting vault history:', err);
        return [];
      }
    }

    /**
     * Get the dynamic interest rate for a specific vault (considers CR-based multiplier + recovery mode).
     * Returns the APR as a decimal (e.g. 0.05 = 5%).
     */
    static async getVaultInterestRate(vaultId: number): Promise<number | null> {
      try {
        const result = await publicActor.get_vault_interest_rate(BigInt(vaultId));
        if ('Ok' in result) return result.Ok;
        console.warn('get_vault_interest_rate error:', result.Err);
        return null;
      } catch (err) {
        console.error('Error getting vault interest rate:', err);
        return null;
      }
    }


    /**
     * Redeem icUSD for ckStable reserves (ckUSDT/ckUSDC).
     * Falls back to vault redemption if reserves are insufficient.
     * @param icusdAmount Amount of icUSD to redeem (human-readable)
     * @param preferredToken Optional principal of preferred ckStable ledger
     */
    static async redeemReserves(
      icusdAmount: number,
      preferredToken?: string
    ): Promise<VaultOperationResult & { stableAmountSent?: number; vaultSpillover?: number }> {
      if (redemptionIngressIsPaused()) return { success: false, error: REDEMPTION_INGRESS_PAUSED };
      try {
        console.log(`Redeeming ${icusdAmount} icUSD for reserves`);

        if (icusdAmount * E8S < MIN_ICUSD_AMOUNT) {
          return {
            success: false,
            error: `Amount too low, minimum is ${MIN_ICUSD_AMOUNT / E8S} icUSD`
          };
        }

        const icusdE8s = BigInt(Math.floor(icusdAmount * E8S));
        const spenderCanisterId = CONFIG.currentCanisterId;
        const bufferedAmount = icusdE8s * 105n / 100n;

        const preferredOpt: [] | [Principal] = preferredToken
          ? [Principal.fromText(preferredToken)]
          : [];

        // Pre-redeem icUSD balance for the Oisy false-negative
        // verifier: redeem burns icUSD from the caller.
        const walletState = get(walletStore);
        // Oisy: read the pre-redeem icUSD balance SYNCHRONOUSLY from the
        // already-loaded wallet store (no awaited query in the gesture window,
        // which would block the signer popup). Non-Oisy uses a fresh skipCache
        // read for max precision.
        const beforeIcusd = isOisyWallet()
          ? (walletState.tokenBalances?.ICUSD?.raw ?? null)
          : (walletState.principal
              ? await TokenService.getTokenBalance(CONFIG.currentIcusdLedgerId, walletState.principal, { skipCache: true }).catch(() => null)
              : null);
        const verifyRedeemLanded = async () => {
          if (beforeIcusd === null || !walletState.principal) return false;
          const after = await TokenService.getTokenBalance(
            CONFIG.currentIcusdLedgerId, walletState.principal, { skipCache: true }
          ).catch(() => null);
          if (after === null) return false;
          return beforeIcusd - after >= (icusdE8s * 95n) / 100n;
        };

        // ─── Oisy ICRC-112 batched path ───
        // Always batch approve+redeem — skipping the allowance check eliminates an
        // async canister query that burns the browser user gesture context.
        const signerAgent = isOisyWallet() ? await pnp.getSignerAgent() : null;
        if (signerAgent) {
          console.log(`[Oisy] Sequential icUSD approve + redeem_reserves`);
          const LARGE_APPROVAL = BigInt(100_000_000_000_000_000); // 1B icUSD in e8s
          const icusdLedgerActor = await walletStore.getActor(
            CONFIG.currentIcusdLedgerId, CONFIG.icusd_ledgerIDL
          ) as any;
          const actor = await ApiClient.getAuthenticatedActor();

          // 1) Approve icUSD (first consent screen, Tier 1 native).
          const approveResult = await icusdLedgerActor.icrc2_approve({
            amount: LARGE_APPROVAL,
            spender: { owner: Principal.fromText(spenderCanisterId), subaccount: [] },
            expires_at: largeApprovalExpiry(), expected_allowance: [], memo: [], fee: [],
            from_subaccount: [], created_at_time: []
          });
          if (approveResult && 'Err' in approveResult) {
            return { success: false, error: `icUSD approval failed: ${JSON.stringify(approveResult.Err)}` };
          }

          // 2) redeem_reserves (second consent screen), guarded against _arr.
          const result = await callWithOisyFalseNegativeGuard(
            () => actor.redeem_reserves(icusdE8s, preferredOpt),
            verifyRedeemLanded,
            `Oisy redeem_reserves ${icusdAmount} icUSD`
          );

          if (isOisyLandedSentinel(result)) {
            return {
              success: true,
              blockIndex: undefined,
              feePaid: undefined,
              oisyResilient: true,
            };
          }

          if ('Ok' in result) {
            const r = result.Ok;
            return {
              success: true,
              blockIndex: Number(r.icusd_block_index),
              feePaid: Number(r.fee_amount) / E8S,
              stableAmountSent: Number(r.stable_amount_sent),
              vaultSpillover: Number(r.vault_spillover_amount),
            };
          } else {
            return { success: false, error: ApiClient.formatProtocolError(result.Err) };
          }
        }

        // ─── Standard path (non-Oisy, or sufficient allowance) ───
        const currentAllowance = await walletOperations.checkIcusdAllowance(spenderCanisterId);
        if (currentAllowance < bufferedAmount) {
          const LARGE_APPROVAL = BigInt(100_000_000_000_000_000);
          const approvalResult = await walletOperations.approveIcusdTransfer(
            LARGE_APPROVAL, spenderCanisterId
          );
          if (!approvalResult.success) {
            return {
              success: false,
              error: approvalResult.error || 'Failed to approve icUSD transfer'
            };
          }
          await new Promise(resolve => setTimeout(resolve, 1000));
        }

        const actor = await ApiClient.getAuthenticatedActor();
        const result = await callWithOisyFalseNegativeGuard(
          () => actor.redeem_reserves(icusdE8s, preferredOpt),
          verifyRedeemLanded,
          `redeem_reserves ${icusdAmount} icUSD`
        );

        if (isOisyLandedSentinel(result)) {
          return {
            success: true,
            blockIndex: undefined,
            feePaid: undefined,
            oisyResilient: true,
          };
        }

        if ('Ok' in result) {
          const r = result.Ok;
          return {
            success: true,
            blockIndex: Number(r.icusd_block_index),
            feePaid: Number(r.fee_amount) / E8S,
            stableAmountSent: Number(r.stable_amount_sent),
            vaultSpillover: Number(r.vault_spillover_amount),
          };
        } else {
          return {
            success: false,
            error: ApiClient.formatProtocolError(result.Err)
          };
        }
      } catch (err) {
        console.error('Error redeeming reserves:', err);
        return {
          success: false,
          error: err instanceof Error ? err.message : 'Unknown error redeeming reserves'
        };
      }
    }

    /**
     * Get reserve balances (ckStable tokens held by protocol).
     * Returns array of {ledger, balance, symbol}.
     */
    static async getReserveBalances(): Promise<Array<{ ledger: string; balance: number; symbol: string }>> {
      try {
        const result = await ApiClient.getPublicData<any[]>('get_reserve_balances');
        return result.map((rb: any) => ({
          ledger: rb.ledger.toText ? rb.ledger.toText() : String(rb.ledger),
          balance: Number(rb.balance),
          symbol: rb.symbol,
        }));
      } catch (err) {
        console.error('Error getting reserve balances:', err);
        return [];
      }
    }

    /**
     * Redeem ICP by providing icUSD
     * @param icusdAmount Amount of icUSD to redeem
     */
      static async redeemIcp(icusdAmount: number): Promise<VaultOperationResult> {
      if (redemptionIngressIsPaused()) return { success: false, error: REDEMPTION_INGRESS_PAUSED };
      try {
        console.log(`Redeeming ${icusdAmount} icUSD for ICP`);

        if (icusdAmount * E8S < MIN_ICUSD_AMOUNT) {
          return {
            success: false,
            error: `Amount too low, minimum is ${MIN_ICUSD_AMOUNT / E8S} icUSD`
          };
        }

        const actor = await ApiClient.getAuthenticatedActor();
        const amountE8s = BigInt(Math.floor(icusdAmount * E8S));

        // Pre-redeem icUSD balance for the Oisy false-negative verifier.
        // Oisy reads it SYNCHRONOUSLY from the wallet store (no gesture-burning
        // query); non-Oisy uses a fresh skipCache read.
        const walletState = get(walletStore);
        const beforeIcusd = isOisyWallet()
          ? (walletState.tokenBalances?.ICUSD?.raw ?? null)
          : (walletState.principal
              ? await TokenService.getTokenBalance(CONFIG.currentIcusdLedgerId, walletState.principal, { skipCache: true }).catch(() => null)
              : null);

        const result = await callWithOisyFalseNegativeGuard(
          () => actor.redeem_icp(amountE8s),
          async () => {
            if (beforeIcusd === null || !walletState.principal) return false;
            const after = await TokenService.getTokenBalance(
              CONFIG.currentIcusdLedgerId, walletState.principal, { skipCache: true }
            ).catch(() => null);
            if (after === null) return false;
            return beforeIcusd - after >= (amountE8s * 95n) / 100n;
          },
          `redeem_icp ${icusdAmount} icUSD`
        );

        if (isOisyLandedSentinel(result)) {
          return {
            success: true,
            blockIndex: undefined,
            feePaid: undefined,
            oisyResilient: true,
          };
        }

        if ('Ok' in result) {
          return {
            success: true,
            blockIndex: Number(result.Ok.block_index),
            feePaid: Number(result.Ok.fee_amount_paid) / E8S
          };
        } else {
          return {
            success: false,
            error: ApiClient.formatProtocolError(result.Err)
          };
        }
      } catch (err) {
        console.error('Error redeeming ICP:', err);
        return {
          success: false,
          error: err instanceof Error ? err.message : 'Unknown error redeeming ICP'
        };
      }
    }
  
    /**
     * Redeem collateral by providing icUSD — generic version that works for any collateral type
     * @param collateralTypePrincipal Ledger canister principal text of the collateral to redeem
     * @param icusdAmount Amount of icUSD to redeem
     */
    static async redeemCollateral(collateralTypePrincipal: string, icusdAmount: number): Promise<VaultOperationResult> {
      if (redemptionIngressIsPaused()) return { success: false, error: REDEMPTION_INGRESS_PAUSED };
      try {
        console.log(`Redeeming ${icusdAmount} icUSD for collateral ${collateralTypePrincipal}`);

        if (icusdAmount * E8S < MIN_ICUSD_AMOUNT) {
          return {
            success: false,
            error: `Amount too low, minimum is ${MIN_ICUSD_AMOUNT / E8S} icUSD`
          };
        }

        const actor = await ApiClient.getAuthenticatedActor();
        const collateralPrincipal = Principal.fromText(collateralTypePrincipal);
        const amountE8s = BigInt(Math.floor(icusdAmount * E8S));

        // Pre-redeem icUSD balance for the Oisy false-negative
        // verifier: redeem burns icUSD from the caller.
        const walletState = get(walletStore);
        // Oisy: read the pre-redeem icUSD balance synchronously from the wallet
        // store (no gesture-burning query); non-Oisy uses a fresh skipCache read.
        const beforeIcusd = isOisyWallet()
          ? (walletState.tokenBalances?.ICUSD?.raw ?? null)
          : (walletState.principal
              ? await TokenService.getTokenBalance(CONFIG.currentIcusdLedgerId, walletState.principal, { skipCache: true }).catch(() => null)
              : null);

        const result = await callWithOisyFalseNegativeGuard(
          () => actor.redeem_collateral(collateralPrincipal, amountE8s),
          async () => {
            if (beforeIcusd === null || !walletState.principal) return false;
            const after = await TokenService.getTokenBalance(
              CONFIG.currentIcusdLedgerId, walletState.principal, { skipCache: true }
            ).catch(() => null);
            if (after === null) return false;
            return beforeIcusd - after >= (amountE8s * 95n) / 100n;
          },
          `redeem_collateral ${icusdAmount} icUSD -> ${collateralTypePrincipal}`
        );

        if (isOisyLandedSentinel(result)) {
          return {
            success: true,
            blockIndex: undefined,
            feePaid: undefined,
            oisyResilient: true,
          };
        }

        if ('Ok' in result) {
          return {
            success: true,
            blockIndex: Number(result.Ok.block_index),
            feePaid: Number(result.Ok.fee_amount_paid) / E8S
          };
        } else {
          return {
            success: false,
            error: ApiClient.formatProtocolError(result.Err)
          };
        }
      } catch (err) {
        console.error('Error redeeming collateral:', err);
        return {
          success: false,
          error: err instanceof Error ? err.message : 'Unknown error redeeming collateral'
        };
      }
    }

    /**
     * Get a specific vault by ID
     * This is a helper method that searches through all user vaults
     */
    static async getVaultById(vaultId: number): Promise<VaultDTO | null> {
      try {
        const vaults = await ApiClient.getUserVaults();
        return vaults.find(v => v.vaultId === vaultId) || null;
      } catch (err) {
        console.error('Error getting vault by ID:', err);
        return null;
      }
    }

    /**
     * Cache-bypassing snapshot of a vault's RAW (e8s / token-decimals) state.
     * Used by the Oisy false-negative verifier — needs precise BigInts, not
     * the float-rounded values on VaultDTO.
     *
     * Returns null if the wallet isn't connected or the canister query fails.
     * Returns `{ exists: false }` if the user has vaults but vaultId isn't
     * among them (e.g. closed vault, or open_vault that never landed).
     */
    static async fetchVaultRawSnapshot(vaultId: number): Promise<
      | { exists: true; collateralAmount: bigint; borrowedIcusd: bigint; icpMargin: bigint }
      | { exists: false }
      | null
    > {
      try {
        const walletState = get(walletStore);
        if (!walletState.principal) return null;

        const canisterVaults = await publicActor.get_vaults([walletState.principal]);
        const v = canisterVaults.find(cv => Number(cv.vault_id) === vaultId);
        if (!v) return { exists: false };
        // Keep the sync cache warm for the Oisy click-path verifiers.
        warmRawSnapshot(walletState.principal.toString(), Number(v.vault_id), {
          collateralAmount: BigInt(v.collateral_amount),
          borrowedIcusd: BigInt(v.borrowed_icusd_amount),
          icpMargin: BigInt(v.icp_margin_amount),
        });
        return {
          exists: true,
          collateralAmount: BigInt(v.collateral_amount),
          borrowedIcusd: BigInt(v.borrowed_icusd_amount),
          icpMargin: BigInt(v.icp_margin_amount),
        };
      } catch (err) {
        console.warn(`fetchVaultRawSnapshot(${vaultId}) failed:`, err);
        return null;
      }
    }

    /**
     * Synchronous reads of the raw-vault-snapshot cache, used by the Oisy
     * click-path verifiers to capture pre-op ("before") state WITHOUT a
     * network await (which would burn the transient-activation window).
     * The non-Oisy path keeps awaiting the fresh queries above.
     */
    static getCachedRawBorrowedE8s(vaultId: number): bigint | null { const s = getRawSnapshot(ApiClient.currentPrincipalText(), vaultId); return s ? s.borrowedIcusd : null; }
    static getCachedRawCollateralAmount(vaultId: number): bigint | null { const s = getRawSnapshot(ApiClient.currentPrincipalText(), vaultId); return s ? s.collateralAmount : null; }
    static getCachedRawSnapshot(vaultId: number): { exists: true; collateralAmount: bigint; borrowedIcusd: bigint; icpMargin: bigint } | null { const s = getRawSnapshot(ApiClient.currentPrincipalText(), vaultId); return s ? { exists: true, collateralAmount: s.collateralAmount, borrowedIcusd: s.borrowedIcusd, icpMargin: s.icpMargin } : null; }
    static getCachedUserVaultIds(): Set<number> | null { return getRawVaultIds(ApiClient.currentPrincipalText()); }

    /**
     * Convenience wrapper around fetchVaultRawSnapshot returning just
     * borrowed_icusd_amount in raw e8s. Returns null on any failure or
     * when the vault doesn't exist.
     */
    static async getRawBorrowedE8s(vaultId: number): Promise<bigint | null> {
      const snap = await ApiClient.fetchVaultRawSnapshot(vaultId);
      if (!snap || !snap.exists) return null;
      return snap.borrowedIcusd;
    }

    /**
     * Convenience wrapper around fetchVaultRawSnapshot returning just
     * collateral_amount in raw token units (per collateral decimals).
     * Returns null on any failure or when the vault doesn't exist.
     */
    static async getRawCollateralAmount(vaultId: number): Promise<bigint | null> {
      const snap = await ApiClient.fetchVaultRawSnapshot(vaultId);
      if (!snap || !snap.exists) return null;
      return snap.collateralAmount;
    }

    /**
     * Snapshot the set of vault IDs the connected user owns right now.
     * Used by the open_vault Oisy false-negative verifier (which then
     * looks for a NEW id post-call). Returns null on any failure.
     */
    static async snapshotUserVaultIds(): Promise<Set<number> | null> {
      try {
        const walletState = get(walletStore);
        if (!walletState.principal) return null;
        const canisterVaults = await publicActor.get_vaults([walletState.principal]);
        const ids = new Set(canisterVaults.map(v => Number(v.vault_id)));
        // Keep the sync vault-id cache warm for the Oisy click-path verifiers.
        warmRawVaultIds(walletState.principal.toString(), ids);
        return ids;
      } catch (err) {
        console.warn('snapshotUserVaultIds failed:', err);
        return null;
      }
    }

    /**
     * Cache-bypassing snapshot of the connected user's protocol-side
     * liquidity-pool position (the on-canister stability pool view).
     * Returns null on any failure.
     */
    static async fetchLiquidityStatusSnapshot(): Promise<
      | {
          liquidityProvided: bigint;
          availableReward: bigint;
        }
      | null
    > {
      try {
        const walletState = get(walletStore);
        if (!walletState.principal) return null;
        const status = await publicActor.get_liquidity_status(walletState.principal);
        return {
          liquidityProvided: BigInt(status.liquidity_provided),
          availableReward: BigInt(status.available_liquidity_reward),
        };
      } catch (err) {
        console.warn('fetchLiquidityStatusSnapshot failed:', err);
        return null;
      }
    }

    /**
     * Find a vault that is in the user's current set but was NOT in
     * `beforeIds`. Used by the open_vault verifier to confirm a new
     * vault landed. Returns null if no new vault is visible yet (or
     * on any query failure).
     */
    static async findNewlyOpenedVault(
      beforeIds: Set<number>
    ): Promise<{ vaultId: number; collateralAmount: bigint } | null> {
      try {
        const walletState = get(walletStore);
        if (!walletState.principal) return null;
        const canisterVaults = await publicActor.get_vaults([walletState.principal]);
        for (const v of canisterVaults) {
          const id = Number(v.vault_id);
          if (!beforeIds.has(id)) {
            return { vaultId: id, collateralAmount: BigInt(v.collateral_amount) };
          }
        }
        return null;
      } catch (err) {
        console.warn('findNewlyOpenedVault failed:', err);
        return null;
      }
    }


    static async getLiquidityStatus(principal: Principal): Promise<CanisterLiquidityStatus> {
        try {
          if (USE_MOCK_DATA) {
            return {
              liquidity_provided: 1000000000n, // 10 ICP
              total_liquidity_provided: 5000000000n, // 50 ICP
              liquidity_pool_share: 0.2, // 20%
              available_liquidity_reward: 500000000n, // 5 icUSD
              total_available_returns: 2500000000n // 25 icUSD
            };
          }
          
          // Use anonymous actor for read-only queries — avoids Oisy signer popups
          const principalForQuery = Principal.fromText(principal.toString());
          return publicActor.get_liquidity_status(principalForQuery);
        } catch (err) {
          console.error('Error getting liquidity status:', err);
          throw new Error('Failed to get liquidity status');
        }
      }
    
      static async getLiquidityV2RequestStateBound(ctx: ActionBoundContext): Promise<LiquidityV2RequestState> {
        assertActionBoundContextCurrent(ctx);
        const actor = await ApiClient.getBoundAuthenticatedActor(ctx);
        assertActionBoundContextCurrent(ctx);
        const state = await actor.get_my_liquidity_v2_request_state();
        assertActionBoundContextCurrent(ctx);
        return state;
      }

      static async attachLiquidityV2CandidateBound(
        ctx: ActionBoundContext,
        intent: LiquidityV2Intent,
        blockIndex: bigint,
      ): Promise<BoundLiquidityV2Result> {
        const ledger = intent.ledgerPrincipal;
        const result = (kind: BoundActionOutcomeKind, status: LiquidityV2StatusView | null, errorMessage: string | null): BoundLiquidityV2Result => ({
          kind, status, errorMessage, approvalMayHaveMutated: intent.approvalAttempted,
        });
        if (ctx.expectedPrincipalText !== intent.owner || !ledger || blockIndex < 0n || blockIndex > 18_446_744_073_709_551_615n) {
          return result('predispatch_aborted', null, 'Candidate block or owner does not match the saved liquidity request.');
        }
        const state = await ApiClient.getLiquidityV2RequestStateBound(ctx);
        const active = state.active_request[0] ?? null;
        if (!active || active.request_id !== BigInt(intent.requestId) ||
            !liquidityV2StatusMatchesIntent(active, intent) || !active.had_ambiguous_attempt ||
            !('Held' in active.phase)) {
          return result('predispatch_aborted', active, 'Only the exact owner-bound Held request with an ambiguous attempt can accept a candidate block.');
        }
        assertActionBoundContextCurrent(ctx);
        const actor = await ApiClient.getBoundAuthenticatedActor(ctx);
        assertActionBoundContextCurrent(ctx);
        const response = await actor.attach_my_liquidity_v2_candidate(BigInt(intent.requestId), blockIndex);
        assertActionBoundContextCurrent(ctx);
        const after = await ApiClient.getLiquidityV2RequestStateBound(ctx);
        const exact = [after.active_request[0] ?? null, after.latest_result[0] ?? null]
          .find((row) => row?.request_id === BigInt(intent.requestId)) ?? null;
        if (exact && liquidityV2StatusMatchesIntent(exact, intent) &&
            'Complete' in exact.phase && exact.result_block_index[0] !== undefined) {
          return result('dispatched_ok', exact, null);
        }
        if (exact && !('Held' in exact.phase) && 'Rejected' in exact.phase)
          return result('dispatched_err', exact, exact.last_error[0] ?? 'The candidate did not prove the exact liquidity transfer.');
        return result('ambiguous_transport', exact, response && 'Err' in response
          ? ApiClient.formatProtocolError(response.Err)
          : 'The candidate remains held and did not prove the exact transfer. Keep the request locked and investigate ledger history.');
      }

      /** Reconcile or replay one exact owner-global liquidity request. */
      static async liquidityV2Bound(
        ctx: ActionBoundContext,
        intent: LiquidityV2Intent,
        beforeApprovalDispatch: () => void,
        confirmAmbiguousApprovalRetry: () => boolean,
        beforeBackendDispatch: () => void,
      ): Promise<BoundLiquidityV2Result> {
        let approvalMayHaveMutated = intent.approvalAttempted;
        const requestId = BigInt(intent.requestId);
        const amountRaw = BigInt(intent.amountRaw);
        const expectedLedger = intent.ledgerPrincipal;
        const result = (kind: BoundActionOutcomeKind, status: LiquidityV2StatusView | null, errorMessage: string | null, amountAdopted = false): BoundLiquidityV2Result => ({
          kind, status, errorMessage, approvalMayHaveMutated, amountAdopted,
        });
        const matches = (status: LiquidityV2StatusView | null): status is LiquidityV2StatusView => !!status &&
          liquidityV2StatusMatchesIntent(status, intent);
        if (ctx.expectedPrincipalText !== intent.owner || !expectedLedger || requestId <= 0n || amountRaw <= 0n ||
            amountRaw > 18_446_744_073_709_551_615n) {
          return result('predispatch_aborted', null, 'Saved liquidity request does not match this wallet or has invalid wire arguments.');
        }

        let state: LiquidityV2RequestState;
        try { state = await ApiClient.getLiquidityV2RequestStateBound(ctx); }
        catch (error) { return result('predispatch_aborted', null, `Could not preflight the liquidity request journal; no approval or action was submitted. ${error instanceof Error ? error.message : ''}`); }
        const active = state.active_request[0] ?? null;
        const latest = state.latest_result[0] ?? null;
        for (const row of [active, latest]) {
          if (!row) continue;
          if (!liquidityV2StatusHasOwner(row, intent.owner)) {
            return result('predispatch_aborted', row, 'The liquidity journal returned an unexpected owner. No approval or action was submitted.');
          }
        }
        let status = [active, latest].find((row) => row?.request_id === requestId) ?? null;
        if (status && !matches(status)) return result('predispatch_aborted', status, 'This request ID is bound to different owner, ledger, operation, or amount arguments. No approval or action was submitted.');
        if (active && active.request_id !== requestId) return result('predispatch_aborted', active, 'A different owner-global liquidity request is unresolved. Resume that exact request first.');
        if (!status && requestId !== state.next_request_id) return result('predispatch_aborted', null, 'This liquidity request ID is absent from the retained journal and is no longer next. Its outcome is unknown; do not replace it.');
        const canAdoptClaimAmount = liquidityV2MayAdoptClaimAmount(intent, !!status, !!active, state.next_request_id);
        if (status) {
          const disposition = liquidityV2Disposition(status);
          if (disposition === 'complete') return result('dispatched_ok', status, null);
          if (disposition === 'rejected') return result('dispatched_err', status, status.last_error[0] ?? 'The exact liquidity request was rejected with no effect.');
        }

        if (!status && expectedLedger !== (intent.operation === 'ClaimReturns' ? CONFIG.currentIcpLedgerId : CONFIG.currentIcusdLedgerId)) {
          return result('predispatch_aborted', null, 'The saved request is pinned to a ledger that is no longer current, and no exact journal row exists. Reconcile the saved approval/request before proceeding.');
        }

        if (!status && intent.operation === 'Provide') {
          try {
            const fee = await fetchLedgerFee({ ledgerId: expectedLedger, decimals: 8, symbol: 'icUSD' });
            assertActionBoundContextCurrent(ctx);
            const allowance = await walletOperations.checkIcusdAllowanceBound(ctx, CONFIG.currentCanisterId);
            assertActionBoundContextCurrent(ctx);
            const requiredAllowance = amountRaw + fee;
            if (allowance < requiredAllowance) {
              if (intent.approvalAttempted && !confirmAmbiguousApprovalRetry())
                return result('predispatch_aborted', null, 'A prior icUSD approval may have succeeded, but allowance is still insufficient. No liquidity request was dispatched.');
              beforeApprovalDispatch();
              assertActionBoundContextCurrent(ctx);
              approvalMayHaveMutated = true;
              const approval = await walletOperations.approveIcusdTransferBound(ctx, amountRaw + fee * 2n, CONFIG.currentCanisterId);
              assertActionBoundContextCurrent(ctx);
              if (!approval.success) return result('predispatch_aborted', null, approval.error ?? 'icUSD approval failed.');
            }
          } catch (error) {
            return result('predispatch_aborted', null, error instanceof Error ? error.message : 'Liquidity approval preflight failed.');
          }
        }

        let response: Awaited<ReturnType<_SERVICE['provide_liquidity_v2']>> | null = null;
        try {
          assertActionBoundContextCurrent(ctx);
          const actor = await ApiClient.getBoundAuthenticatedActor(ctx);
          assertActionBoundContextCurrent(ctx);
          beforeBackendDispatch();
          assertActionBoundContextCurrent(ctx);
          if (intent.operation === 'Provide') response = await actor.provide_liquidity_v2(requestId, amountRaw);
          else if (intent.operation === 'Withdraw') response = await actor.withdraw_liquidity_v2(requestId, amountRaw);
          else response = await actor.claim_liquidity_returns_v2(requestId);
          assertActionBoundContextCurrent(ctx);
        } catch (error) {
          let recovered: LiquidityV2StatusView | null = null;
          let recoveredState: LiquidityV2RequestState | null = null;
          try {
            recoveredState = await ApiClient.getLiquidityV2RequestStateBound(ctx);
            recovered = [recoveredState.active_request[0] ?? null, recoveredState.latest_result[0] ?? null].find((row) => row?.request_id === requestId) ?? null;
          } catch { /* exact status remains unavailable */ }
          const noConflictingActive = !!recoveredState && (!recoveredState.active_request[0] || recoveredState.active_request[0]?.request_id === requestId);
          const recoveredMatches = matches(recovered) || (canAdoptClaimAmount && noConflictingActive && !!recovered && liquidityV2ClaimIdentityMatches(recovered, intent));
          if (recoveredMatches && recovered) {
            const disposition = liquidityV2Disposition(recovered);
            const adopted = !matches(recovered) && canAdoptClaimAmount;
            if (disposition === 'complete') return result('dispatched_ok', recovered, null, adopted);
            if (disposition === 'rejected') return result('dispatched_err', recovered, recovered.last_error[0] ?? 'The exact liquidity request was rejected.', adopted);
            if (adopted) return result('ambiguous_transport', recovered, recovered.last_error[0] ?? 'The exact claim request is pending or held.', true);
          }
          return result('ambiguous_transport', matches(recovered) ? recovered : null, error instanceof Error ? error.message : 'Liquidity reply was lost; reconcile this exact request.');
        }

        let after: LiquidityV2RequestState;
        try { after = await ApiClient.getLiquidityV2RequestStateBound(ctx); }
        catch (error) { return result('ambiguous_transport', null, `Liquidity call returned but its exact journal status is unavailable. ${error instanceof Error ? error.message : ''}`); }
        const exact = [after.active_request[0] ?? null, after.latest_result[0] ?? null].find((row) => row?.request_id === requestId) ?? null;
        const exactCanAdoptClaim = canAdoptClaimAmount && !!exact && liquidityV2ClaimIdentityMatches(exact, intent) &&
          ((after.active_request[0]?.request_id === requestId) || !after.active_request[0]);
        const exactMatches = matches(exact) || exactCanAdoptClaim;
        if (exactMatches && exact) {
          const disposition = liquidityV2Disposition(exact);
          const adopted = exactCanAdoptClaim && !matches(exact);
          if (disposition === 'complete') return result('dispatched_ok', exact, null, adopted);
          if (disposition === 'rejected') return result('dispatched_err', exact, exact.last_error[0] ?? (response && 'Err' in response ? ApiClient.formatProtocolError(response.Err) : 'The exact liquidity request was rejected.'), adopted);
          return result('ambiguous_transport', exact, exact.last_error[0] ?? 'The exact liquidity request remains pending or held. Replay only this request after checking its journal.', adopted);
        }
        if (response && 'Err' in response && !active && after.next_request_id === requestId) {
          return result('dispatched_err', null, ApiClient.formatProtocolError(response.Err));
        }
        return result('ambiguous_transport', null, response && 'Err' in response
          ? ApiClient.formatProtocolError(response.Err)
          : 'Backend response has no exact matching receipt. Reconcile before taking another action.');
      }

      /** Legacy no-ID liquidity routes are disabled before approval or dispatch. */
      static async provideLiquidity(amount: number): Promise<VaultOperationResult> {
        void amount;
        return { success: false, error: 'Legacy liquidity requests are disabled. Refresh the app and use the journaled V2 liquidity flow.' };
      }

      static async withdrawLiquidity(amount: number): Promise<VaultOperationResult> {
        void amount;
        return { success: false, error: 'Legacy liquidity requests are disabled. Refresh the app and use the journaled V2 liquidity flow.' };
      }

      static async claimLiquidityReturns(): Promise<VaultOperationResult> {
        return { success: false, error: 'Legacy liquidity requests are disabled. Refresh the app and use the journaled V2 liquidity flow.' };
      }

      /**
       * Claim a pending transfer that has been stuck
       */
      static async claimPendingTransfer(vaultId: number): Promise<VaultOperationResult> {
        try {
          console.log(`Attempting to claim pending transfer for vault #${vaultId}`);
          
          // First attempt to trigger pending transfer processing
          await ApiClient.triggerPendingTransfers();
          
          // Give some time for the backend to process
          await new Promise(resolve => setTimeout(resolve, 3000));
          
          // Now try a direct approach by querying the backend for this vault's status
          const actor = await ApiClient.getAuthenticatedActor();
          
          // For now, the backend doesn't have a specific endpoint for this,
          // so we'll simulate success if we can trigger pending transfers
          const simulatedResult = {
            success: true,
            vaultId,
            blockIndex: Date.now() // Using timestamp as a fake block index
          };
          
          return simulatedResult;
        } catch (err) {
          console.error('Error claiming pending transfer:', err);
          return {
            success: false,
            error: err instanceof Error ? err.message : 'Unknown error claiming transfer'
          };
        }
      }

      static logApiResponse(response: any): void {
        console.log('API response data:', BigIntUtils.stringify(response));
      }
    
      // When storing in localStorage:
      static saveToLocalStorage(key: string, data: any): void {
        try {
          localStorage.setItem(key, BigIntUtils.stringify(data));
        } catch (err) {
          console.error('Error saving to localStorage:', err);
        }
      }
    
      static getFromLocalStorage<T>(key: string): T | null {
        try {
          const data = localStorage.getItem(key);
          return data ? BigIntUtils.parse(data) : null;
        } catch (err) {
          console.error('Error reading from localStorage:', err);
          return null;
        }
      }
    
      // When formatting for display
      static formatAmount(amount: bigint, decimals: number = 8): string {
        return BigIntUtils.formatE8s(amount);
      }

  /**
   * Get pending transfers
   */
  static async getPendingTransfers(): Promise<any[]> {
    // For now, just return the transfers from the vault store
    const { vaultStore } = await import('$lib/stores/vaultStore');
    return get(vaultStore).pendingTransfers.map(transfer => ({
      id: `RUMI-${transfer.vaultId}-${Date.now()}`,
      amount: transfer.amount,
      timestamp: transfer.timestamp,
      completed: false
    }));
  }

  /** Read the backend's globally health-ordered, consecutive same-collateral redemption runs. */
  static async getRedemptionQueue(): Promise<RedemptionQueue> {
    return ApiClient.getPublicData('get_redemption_queue');
  }

  /** Read a cached redemption snapshot for an indicative estimate only. */
  static async getRedemptionPreview(amountE8s: bigint): Promise<RedemptionPreview> {
    return ApiClient.getPublicData('get_redemption_preview', amountE8s);
  }

  /** Refresh stale candidate prices and prepare a no-funds live offer. */
  static async prepareRedemptionOffer(amountE8s: bigint): Promise<RedemptionOfferRefreshResult> {
    return publicActor.prepare_redemption_offer(amountE8s);
  }

  /** Read the exact quote for the first currently eligible redemption run. */
  static async getRedemptionQuote(amountE8s: bigint): Promise<RedemptionQuoteResult> {
    return ApiClient.getPublicData('get_redemption_quote', amountE8s);
  }

  /**
   * Prepare fresh ICRC-2 allowance, icUSD balance, and ledger fee reads before
   * a signer gesture. Oisy callers pass this snapshot into redeemQuoted so no
   * asynchronous ledger query is needed between the click and approval.
   */
  static async getRedemptionPreflight(): Promise<RedemptionPreflight> {
    const walletState = get(walletStore);
    const principal = walletState.principal;
    const principalText = principal?.toText() ?? '';
    const expectedWalletType = get(currentWalletType);
    if (!walletState.isConnected || !principal || !principalText || !expectedWalletType) {
      throw new Error('Connect a wallet before checking redemption allowance and balance.');
    }
    const expectedOisy = expectedWalletType === WALLET_TYPES.OISY;
    const expectedSessionGeneration = get(walletSessionGeneration);
    const ctx: ActionBoundContext = {
      expectedPrincipalText: principalText,
      assertCurrent: () => get(walletStore).isConnected
        && get(currentWalletType) === expectedWalletType
        && get(walletSessionGeneration) === expectedSessionGeneration,
    };
    assertActionBoundContextCurrent(ctx);
    const ledgerId = CONFIG.currentIcusdLedgerId;
    const ledgerActor = Actor.createActor(icusd_ledgerIDL as any, {
      agent: anonymousAgent,
      canisterId: ledgerId,
    }) as any;
    const account = { owner: Principal.fromText(principalText), subaccount: [] };
    const spender = { owner: Principal.fromText(CONFIG.currentCanisterId), subaccount: [] };
    const [allowance, balance, fee] = await Promise.all([
      ledgerActor.icrc2_allowance({ account, spender }),
      ledgerActor.icrc1_balance_of(account),
      ledgerActor.icrc1_fee(),
    ]);
    assertActionBoundContextCurrent(ctx);
    return {
      principalText,
      walletType: expectedWalletType,
      sessionGeneration: expectedSessionGeneration,
      ledgerId,
      observedAtMs: Date.now(),
      allowanceRaw: BigInt(allowance.allowance),
      balanceRaw: BigInt(balance),
      feeRaw: BigInt(fee),
    };
  }

  /** Submit a quote-bound direct-vault redemption. A successful result means queued, not delivered. */
  static async redeemQuoted(
    request: RedemptionQuotedRequest,
    preparedPreflight: RedemptionPreflight | undefined,
    acceptedOffer: AcceptedRedemptionOffer,
  ): Promise<RedeemQuotedResult> {
    if (redemptionIngressIsPaused()) return { success: false, error: REDEMPTION_INGRESS_PAUSED };
    let submissionDispatched = false;
    let submissionReplyObserved = false;
    try {
      const walletState = get(walletStore);
      const expectedPrincipalText = walletState.principal?.toText() ?? '';
      const expectedWalletType = get(currentWalletType);
      const expectedSessionGeneration = get(walletSessionGeneration);
      const expectedOisy = expectedWalletType === WALLET_TYPES.OISY;
      if (!walletState.isConnected || !expectedPrincipalText || !expectedWalletType) {
        return { success: false, error: 'Connect a wallet before redeeming.' };
      }
      const acceptedOfferIsCurrent = () => {
        const currentWallet = get(walletStore);
        const currentWalletTypeValue = get(currentWalletType);
        const currentPrincipalText = currentWallet.principal?.toText() ?? '';
        return currentWallet.isConnected && acceptedRedemptionOfferTermsAreCurrent(
          acceptedOffer,
          request.amount_e8s,
          request.expected_collateral_type.toText(),
          request.min_net_collateral_raw,
          {
            principalText: currentPrincipalText,
            ledgerId: CONFIG.currentIcusdLedgerId,
            walletType: currentWalletTypeValue,
            sessionGeneration: get(walletSessionGeneration),
            networkKey: CONFIG.host,
          },
        );
      };
      if (!acceptedOfferIsCurrent()) {
        return { success: false, error: 'Accept a fresh live offer for this amount, asset, wallet, and network before redeeming.' };
      }
      const actionContext: ActionBoundContext = {
        expectedPrincipalText,
        assertCurrent: () => get(walletStore).isConnected
          && get(currentWalletType) === expectedWalletType
          && get(walletSessionGeneration) === expectedSessionGeneration,
      };
      assertActionBoundContextCurrent(actionContext);

      // Oisy's signer gesture cannot safely span an asynchronous allowance
      // query. Match the other Oisy flows: request an explicit, amount-bounded
      // approval, then submit the action as the next signer operation.
      const spender = Principal.fromText(CONFIG.currentCanisterId);
      const preflight = expectedOisy
        ? preparedPreflight
        : await ApiClient.getRedemptionPreflight();
      assertActionBoundContextCurrent(actionContext);
      if (!acceptedOfferIsCurrent()) {
        return { success: false, error: 'The accepted live offer expired or changed before approval. Check and accept a new offer.' };
      }
      if (!preflight
        || preflight.principalText !== expectedPrincipalText
        || preflight.walletType !== expectedWalletType
        || preflight.sessionGeneration !== expectedSessionGeneration
        || preflight.ledgerId !== CONFIG.currentIcusdLedgerId
        || !Number.isSafeInteger(preflight.observedAtMs)
        || Date.now() - preflight.observedAtMs > 30_000
        || preflight.observedAtMs > Date.now() + 1_000
        || typeof preflight.allowanceRaw !== 'bigint'
        || preflight.allowanceRaw < 0n
        || typeof preflight.balanceRaw !== 'bigint'
        || preflight.balanceRaw < 0n
        || typeof preflight.feeRaw !== 'bigint'
        || preflight.feeRaw < 0n
        || typeof request.amount_e8s !== 'bigint'
        || request.amount_e8s <= 0n) {
        return {
          success: false,
          error: expectedOisy
            ? 'The icUSD allowance, balance, and ledger fee check is missing or stale. Refresh the redemption preflight before signing.'
            : 'The icUSD allowance, balance, or ledger fee check is stale. Refresh before redeeming.',
        };
      }

      const requiredAllowance = request.amount_e8s + preflight.feeRaw;
      const needsApproval = preflight.allowanceRaw < requiredAllowance;
      const requiredBalance = request.amount_e8s + preflight.feeRaw * (needsApproval ? 2n : 1n);
      if (preflight.balanceRaw < requiredBalance) {
        return {
          success: false,
          error: needsApproval
            ? `Insufficient icUSD for this redemption and its approval/transfer fees. Required ${requiredBalance} raw units; available ${preflight.balanceRaw}.`
            : `Insufficient icUSD for this redemption and its transfer fee. Required ${requiredBalance} raw units; available ${preflight.balanceRaw}.`,
        };
      }

      if (expectedOisy) {
        await pnp.getSignerAgent();
        assertActionBoundContextCurrent(actionContext);
        if (!acceptedOfferIsCurrent()) {
          return { success: false, error: 'The accepted live offer expired while preparing the wallet. Check and accept a new offer.' };
        }
      }

      if (needsApproval) {
        assertActionBoundContextCurrent(actionContext);
        if (!acceptedOfferIsCurrent()) {
          return { success: false, error: 'The accepted live offer expired before approval. Check and accept a new offer.' };
        }
        const approvalActor = await walletStore.getActor(
          CONFIG.currentIcusdLedgerId, CONFIG.icusd_ledgerIDL
        ) as any;
        assertActionBoundContextCurrent(actionContext);
        if (!acceptedOfferIsCurrent()) {
          return { success: false, error: 'The accepted live offer expired before approval. Check and accept a new offer.' };
        }
        let approvalResult: any;
        try {
          approvalResult = await approvalActor.icrc2_approve({
            amount: requiredAllowance,
            spender: { owner: spender, subaccount: [] },
            expires_at: largeApprovalExpiry(),
            expected_allowance: [], memo: [], fee: [],
            from_subaccount: [], created_at_time: [],
          });
        } catch (err) {
          return {
            success: false,
            ambiguous: true,
            ambiguityStage: 'approval',
            error: err instanceof Error
              ? `The icUSD approval response was unclear. Check the allowance before retrying. (${err.message})`
              : 'The icUSD approval response was unclear. Check the allowance before retrying.',
          };
        }
        assertActionBoundContextCurrent(actionContext);
        if (!acceptedOfferIsCurrent()) {
          return { success: false, error: 'The approval completed, but the offer expired before redemption. No redemption was submitted; check and accept a new offer.' };
        }
        if (approvalResult && 'Err' in approvalResult) {
          return {
            success: false,
            ambiguityStage: 'approval',
            error: `icUSD approval failed: ${JSON.stringify(approvalResult.Err)}`,
          };
        }
      }

      assertActionBoundContextCurrent(actionContext);
      if (!acceptedOfferIsCurrent()) {
        return { success: false, error: 'The accepted live offer expired before redemption. Check and accept a new offer.' };
      }
      const actor = await ApiClient.getBoundAuthenticatedActor(actionContext);
      assertActionBoundContextCurrent(actionContext);
      if (!acceptedOfferIsCurrent()) {
        return { success: false, error: 'The accepted live offer expired before redemption. Check and accept a new offer.' };
      }

      let result: RedemptionResultVariant;
      submissionDispatched = true;
      try {
        result = await actor.redeem_quoted(request);
      } catch (err) {
        return {
          success: false,
          ambiguous: true,
          ambiguityStage: 'submission',
          error: err instanceof Error
            ? `The redemption request was submitted, but its reply was not received. Check the redemption queue before retrying. (${err.message})`
            : 'The redemption request was submitted, but its reply was not received. Check the redemption queue before retrying.',
        };
      }
      submissionReplyObserved = true;
      let sessionChangedAfterSubmission = false;
      try {
        assertActionBoundContextCurrent(actionContext);
      } catch (err) {
        if (!(err instanceof StaleActionSessionError)) throw err;
        sessionChangedAfterSubmission = true;
      }

      if ('Err' in result) {
        return {
          success: false,
          error: ApiClient.formatProtocolError(result.Err),
          ...(sessionChangedAfterSubmission ? { sessionChangedAfterSubmission: true } : {}),
        };
      }

      const redeemed = result.Ok;
      return {
        success: true,
        blockIndex: Number(redeemed.icusd_block_index),
        feePaid: Number(redeemed.fee_paid_e8s) / E8S,
        redemption: {
          collateralType: redeemed.collateral_type.toText(),
          symbol: redeemed.symbol,
          decimals: Number(redeemed.decimals),
          netCollateralRaw: redeemed.net_collateral_raw,
          payoutStatus: redeemed.payout_status,
        },
        ...(sessionChangedAfterSubmission
          ? {
              sessionChangedAfterSubmission: true,
              message: 'The backend confirmed this redemption for the previous wallet session. Check that wallet’s redemption queue before starting another redemption.',
            }
          : {}),
      };
    } catch (err) {
      console.error('Error submitting quoted redemption:', err);
      return {
        success: false,
        ...(submissionDispatched && !submissionReplyObserved
          ? { ambiguous: true, ambiguityStage: 'submission' as const }
          : {}),
        error: err instanceof Error ? err.message : 'Unknown error submitting quoted redemption',
      };
    }
  }

  /**
   * Step 1 of two-phase vault closing: Prepare to close vault
   */
  static async prepareCloseVault(vaultId: number): Promise<VaultOperationResult & { txHash?: string }> {
    try {
      console.log(`Preparing to close vault #${vaultId}`);
      
      // This is a two-phase implementation - in Step 1 we just validate and mark the vault
      // We don't make any blockchain changes yet
      
      // Check if the vault exists and can be closed
      const vault = await ApiClient.getVaultById(vaultId);
      
      if (!vault) {
        return {
          success: false,
          error: 'Vault not found'
        };
      }
      
      if (vault.borrowedIcusd > 0) {
        return {
          success: false,
          error: 'Cannot close vault with outstanding debt'
        };
      }
      
      // Store vault in preparation state locally
      // In a real implementation, we might lock the vault on-chain
      localStorage.setItem(`vault-closing-${vaultId}`, JSON.stringify({
        vaultId,
        icpMargin: vault.icpMargin,
        timestamp: Date.now(),
        phase: 'prepared'
      }));
      
      // Return success with a dummy transaction hash
      return {
        success: true,
        vaultId,
        txHash: `prep-${vaultId}-${Date.now().toString(36)}`
      };
    } catch (err) {
      console.error('Error preparing vault closure:', err);
      return {
        success: false,
        error: err instanceof Error ? err.message : 'Unknown error preparing vault closure'
      };
    }
  }

  /**
   * Step 2 of two-phase vault closing: Execute transfer and close vault
   */
  static async executeTransferAndClose(vaultId: number): Promise<VaultOperationResult & { txHash?: string }> {
    try {
      console.log(`Executing closure for vault #${vaultId}`);
      
      // Check if the vault was properly prepared
      const prepData = localStorage.getItem(`vault-closing-${vaultId}`);
      if (!prepData) {
        return {
          success: false,
          error: 'Vault was not properly prepared for closure'
        };
      }
      
      const prepInfo = JSON.parse(prepData);
      if (prepInfo.phase !== 'prepared') {
        return {
          success: false,
          error: 'Vault is not in the prepared state'
        };
      }
      
      // Now actually close the vault
      const result = await ApiClient.closeVault(vaultId);
      
      // Clean up preparation data
      localStorage.removeItem(`vault-closing-${vaultId}`);
      
      if (result.success) {
        return {
          ...result,
          txHash: result.blockIndex?.toString() || `close-${vaultId}-${Date.now().toString(36)}`
        };
      } else {
        return result;
      }
    } catch (err) {
      console.error('Error executing vault closure:', err);
      return {
        success: false,
        error: err instanceof Error ? err.message : 'Unknown error closing vault'
      };
    }
  }

/**
 * Withdraw collateral and close vault in one operation
 */
static async withdrawCollateralAndCloseVault(vaultId: number): Promise<VaultOperationResult> {
  return ApiClient.executeSequentialOperation(async () => {
    try {
      console.log(`Withdrawing collateral and closing vault #${vaultId} in one operation`);
      
      // First ensure the vault exists. Oisy reads the synchronous snapshot
      // cache — a getVaultById query here would burn the gesture window and
      // block the signer popup; non-Oisy queries fresh.
      let vaultExists: boolean;
      let borrowedIcusdHuman: number;
      if (isOisyWallet()) {
        const snap = ApiClient.getCachedRawSnapshot(vaultId);
        vaultExists = !!snap;
        borrowedIcusdHuman = snap ? Number(snap.borrowedIcusd) / E8S : 0;
      } else {
        const vault = await ApiClient.getVaultById(vaultId);
        vaultExists = !!vault;
        borrowedIcusdHuman = vault ? vault.borrowedIcusd : 0;
      }

      if (!vaultExists) {
        console.log(`Vault #${vaultId} not found, it may have already been closed`);
        return {
          success: true,
          message: `Vault #${vaultId} is already closed.`,
          vaultId
        };
      }

      // Verify the vault has no debt (dust below 0.0005 icUSD is forgiven by backend)
      const DUST_THRESHOLD = 0.0005;
      if (borrowedIcusdHuman > DUST_THRESHOLD) {
        return {
          success: false,
          error: `Cannot close vault while it has outstanding debt of ${borrowedIcusdHuman} icUSD. Please repay all debt first.`
        };
      }
      
      try {
        // Call the unified backend method
        const actor = await ApiClient.getAuthenticatedActor();

        const result = await callWithOisyFalseNegativeGuard(
          () => actor.withdraw_and_close_vault(BigInt(vaultId)),
          async () => {
            // After withdraw_and_close, the vault is gone. The
            // canister may keep a husk with zeroed fields, so accept
            // either form as proof.
            const snap = await ApiClient.fetchVaultRawSnapshot(vaultId);
            if (!snap) return false;
            if (!snap.exists) return true;
            return snap.collateralAmount === 0n && snap.borrowedIcusd === 0n;
          },
          `withdraw_and_close_vault #${vaultId}`
        );

        if (isOisyLandedSentinel(result)) {
          return {
            success: true,
            vaultId,
            blockIndex: undefined,
            message: `Successfully withdrew collateral and closed vault #${vaultId}`,
            vaultClosed: true,
            oisyResilient: true,
          };
        }

        if ('Ok' in result) {
          // If we got a block index back, there was an ICP transfer
          const blockIndex = result.Ok.length > 0 ? Number(result.Ok[0]) : undefined;

          return {
            success: true,
            vaultId,
            blockIndex,
            message: `Successfully withdrew collateral and closed vault #${vaultId}`,
            vaultClosed: true
          };
        } else {
          // Check for specific error conditions
          const errorMsg = ApiClient.formatProtocolError(result.Err);
          
          // If the error indicates the vault doesn't exist, treat as success
          if (ApiClient.isVaultNotFoundError(errorMsg)) {
            return {
              success: true,
              message: `Vault #${vaultId} has already been closed.`,
              vaultId,
              vaultClosed: true
            };
          }
          
          return {
            success: false,
            error: errorMsg
          };
        }
      } catch (err) {
        console.error('Error withdrawing collateral and closing vault:', err);
        return {
          success: false,
          error: err instanceof Error ? err.message : 'Unknown error during withdraw and close operation'
        };
      }
    } catch (err) {
      console.error('Error verifying vault before withdraw and close:', err);
      return {
        success: false,
        error: err instanceof Error ? err.message : 'Unknown error verifying vault'
      };
    }
    // REMOVED: Don't manually track operations here
  }, vaultId); // Pass vaultId here to let executeSequentialOperation handle tracking
}

  /**
   * Helper to check if an error indicates vault not found
   */
  private static isVaultNotFoundError(errorMsg: string): boolean {
    const lowerMsg = errorMsg.toLowerCase();
    return lowerMsg.includes('not found') || 
           lowerMsg.includes('unknown vault') ||
           lowerMsg.includes('tried to close unknown vault');
  }

    /**
     * Page size for the cursor-based vault enumeration helpers below. The
     * backend caps each page at 500 (Wave-9a DOS-004); we ask for that ceiling
     * so a typical-size protocol comes back in a single call. Pages are
     * stitched into one array so existing call sites still see the full set.
     */
    static readonly VAULT_PAGE_SIZE = 500n;

    /**
     * Defensive ceiling on the number of paginated calls we'll chain in
     * `getLiquidatableVaults` / `getAllVaults`. At 500 vaults per page this
     * caps a single fetch at 50,000 vaults — well above any realistic TVL,
     * but it stops a buggy `next_start_id` (e.g. cursor that fails to advance)
     * from spinning forever.
     */
    static readonly VAULT_PAGE_MAX_PAGES = 100;

    static async getLiquidatableVaults(): Promise<CandidVault[]> {
      try {
        const all: CandidVault[] = [];
        let startId = 0n;
        for (let page = 0; page < ApiClient.VAULT_PAGE_MAX_PAGES; page += 1) {
          const resp = await ApiClient.getPublicData<{
            vaults: CandidVault[];
            next_start_id: [] | [bigint];
          }>('get_liquidatable_vaults_page', startId, ApiClient.VAULT_PAGE_SIZE);
          all.push(...resp.vaults);
          if (resp.next_start_id.length === 0) break;
          startId = resp.next_start_id[0];
        }
        return all;
      } catch (err) {
        console.error('Failed to get liquidatable vaults:', err);
        return [];
      }
    }

    static async getAllVaults(): Promise<CandidVault[]> {
      try {
        const all: CandidVault[] = [];
        let startId = 0n;
        for (let page = 0; page < ApiClient.VAULT_PAGE_MAX_PAGES; page += 1) {
          const resp = await ApiClient.getPublicData<{
            vaults: CandidVault[];
            next_start_id: [] | [bigint];
          }>('get_vaults_page', startId, ApiClient.VAULT_PAGE_SIZE);
          all.push(...resp.vaults);
          if (resp.next_start_id.length === 0) break;
          startId = resp.next_start_id[0];
        }
        return all;
      } catch (err) {
        console.error('Failed to get all vaults:', err);
        return [];
      }
    }
  
    /**
     * Partially liquidate a specific vault
     * @param vaultId The ID of the vault to liquidate
     * @param icusdAmount The amount of icUSD to liquidate
     */
    static async liquidateVaultPartial(_vaultId: number, _icusdAmount: number): Promise<VaultOperationResult> {
      return {
        success: false,
        error: 'Manual liquidation is temporarily unavailable while receipt-backed recovery is being verified.'
      };
    }

    /**
     * Partially liquidate a vault using ckUSDT or ckUSDC
     */
    static async liquidateVaultPartialWithStable(
      _vaultId: number,
      _icusdAmount: number,
      _tokenType: 'CKUSDT' | 'CKUSDC'
    ): Promise<VaultOperationResult> {
      // The backend keeps this legacy no-request-ID route closed. Avoid asking
      // the wallet to approve funds for an operation that cannot be admitted.
      return {
        success: false,
        error: 'Stablecoin liquidation is temporarily unavailable while receipt-backed recovery is being verified.'
      };
    }

    /**
     * Liquidate a specific vault (complete liquidation)
     * @param vaultId The ID of the vault to liquidate
     */
    static async liquidateVault(_vaultId: number): Promise<VaultOperationResult> {
      return {
        success: false,
        error: 'Manual liquidation is temporarily unavailable while receipt-backed recovery is being verified.'
      };
    }

  /**
   * Clear all stale operation states
   */
  private static clearAllStaleOperations() {
    const now = Date.now();
    let clearedCount = 0;
      // Clear operation flags on page load to avoid stuck operations between sessions
      ApiClient.operationInProgress = false;
      ApiClient.operationTimestamps.clear();
       console.log(`Cleared ${clearedCount} stale vault operations`);
    }
  


}

// Add treasury service for accessing fee data
export class TreasuryService {
  private static readonly TREASURY_CANISTER_ID = CANISTER_IDS.TREASURY;
  
  // Get treasury status and balances
  static async getTreasuryStatus(): Promise<{
    totalDeposits: number;
    balances: { [key: string]: number };
    controller: string;
    isPaused: boolean;
  }> {
    try {
      // Create anonymous actor for treasury queries
      const treasuryActor = Actor.createActor(treasuryIDL as any, {
        agent: new HttpAgent({ host: CONFIG.host }),
        canisterId: this.TREASURY_CANISTER_ID
      }) as any; // Type as 'any' to handle the treasury service interface
      
      const status = await treasuryActor.get_status();
      
      // Convert the balances array to a more usable object format
      const balances: { [key: string]: number } = {};
      if (status.balances) {
        for (const [assetType, assetBalance] of status.balances) {
          let assetKey = 'UNKNOWN';
          if ('ICUSD' in assetType) assetKey = 'ICUSD';
          else if ('ICP' in assetType) assetKey = 'ICP';
          else if ('CKBTC' in assetType) assetKey = 'CKBTC';
          
          balances[assetKey] = Number(assetBalance.total || 0) / E8S;
        }
      }
      
      return {
        totalDeposits: Number(status.total_deposits || 0),
        balances,
        controller: status.controller?.toString() || '',
        isPaused: Boolean(status.is_paused)
      };
    } catch (error) {
      console.error('Error getting treasury status:', error);
      throw error;
    }
  }
  
  // Get fee history (deposit records)
  static async getFeeHistory(start?: number, limit: number = 100): Promise<Array<{
    id: number;
    feeType: string;
    assetType: string;
    amount: number;
    blockIndex: number;
    timestamp: Date;
    memo: string | null;
  }>> {
    try {
      const treasuryActor = Actor.createActor(treasuryIDL as any, {
        agent: new HttpAgent({ host: CONFIG.host }),
        canisterId: this.TREASURY_CANISTER_ID
      }) as any;
      
      const deposits = await treasuryActor.get_deposits(
        start ? [BigInt(start)] : [], 
        [limit]
      );
      
      return deposits.map((deposit: any) => {
        // Parse the deposit type
        let feeType = 'Unknown';
        if (deposit.deposit_type && typeof deposit.deposit_type === 'object') {
          if ('MintingFee' in deposit.deposit_type) feeType = 'MintingFee';
          else if ('RedemptionFee' in deposit.deposit_type) feeType = 'RedemptionFee';
          else if ('LiquidationSurplus' in deposit.deposit_type) feeType = 'LiquidationSurplus';
          else if ('StabilityFee' in deposit.deposit_type) feeType = 'StabilityFee';
        }
        
        // Parse the asset type
        let assetType = 'Unknown';
        if (deposit.asset_type && typeof deposit.asset_type === 'object') {
          if ('ICUSD' in deposit.asset_type) assetType = 'ICUSD';
          else if ('ICP' in deposit.asset_type) assetType = 'ICP';
          else if ('CKBTC' in deposit.asset_type) assetType = 'CKBTC';
        }
        
        return {
          id: Number(deposit.id || 0),
          feeType,
          assetType,
          amount: Number(deposit.amount || 0) / E8S,
          blockIndex: Number(deposit.block_index || 0),
          timestamp: new Date(Number(deposit.timestamp || 0) / 1000000), // Convert from nanos
          memo: deposit.memo && deposit.memo.length > 0 ? deposit.memo[0] : null
        };
      });
    } catch (error) {
      console.error('Error getting fee history:', error);
      throw error;
    }
  }
  
  // Get total fees collected by type
  static async getFeesByType(): Promise<{ [key: string]: number }> {
    try {
      const history = await this.getFeeHistory();
      const feesByType: { [key: string]: number } = {};
      
      for (const deposit of history) {
        const key = `${deposit.feeType}_${deposit.assetType}`;
        feesByType[key] = (feesByType[key] || 0) + deposit.amount;
      }
      
      return feesByType;
    } catch (error) {
      console.error('Error calculating fees by type:', error);
      throw error;
    }
  }

  // Withdraw funds from treasury (controller only)
  static async withdrawFromTreasury(
    assetType: 'ICUSD' | 'ICP' | 'CKBTC',
    amount: number,
    to: string,
    memo?: string
  ): Promise<{ success: boolean; blockIndex?: number; error?: string }> {
    try {
      // Get authenticated actor (must be controller)
      const treasuryActor = await walletStore.getActor(this.TREASURY_CANISTER_ID, treasuryIDL) as any;
      
      // Create the asset type object in the format expected by the treasury canister
      const assetTypeObj: any = {};
      assetTypeObj[assetType] = null;
      
      const withdrawArgs = {
        asset_type: assetTypeObj,
        amount: BigInt(Math.floor(amount * E8S)),
        to: Principal.fromText(to),
        memo: memo ? [memo] : []
      };
      
      const result = await treasuryActor.withdraw(withdrawArgs);
      
      if ('Ok' in result) {
        return {
          success: true,
          blockIndex: Number(result.Ok.block_index)
        };
      } else {
        return {
          success: false,
          error: result.Err || 'Unknown withdrawal error'
        };
      }
    } catch (error) {
      console.error('Error withdrawing from treasury:', error);
      return {
        success: false,
        error: error instanceof Error ? error.message : 'Unknown error'
      };
    }
  }
  
  // Check if current user is treasury controller
  static async isController(): Promise<boolean> {
    try {
      const status = await this.getTreasuryStatus();
      const walletState = get(walletStore);
      
      return walletState.principal?.toString() === status.controller;
    } catch (error) {
      console.error('Error checking controller status:', error);
      return false;
    }
  }
  
  // Get treasury withdrawal history
  static async getWithdrawalHistory(limit: number = 50): Promise<any[]> {
    try {
      const treasuryActor = Actor.createActor(treasuryIDL as any, {
        agent: new HttpAgent({ host: CONFIG.host }),
        canisterId: this.TREASURY_CANISTER_ID
      }) as any;
      
      // Get all deposits and filter for withdrawals (negative amounts or specific types)
      const deposits = await treasuryActor.get_deposits([], [limit * 2]);
      
      // In a full implementation, you'd have withdrawal events
      // For now, we return the deposit history as reference
      return deposits.map((deposit: any) => {
        // Parse deposit type
        let feeType = 'Unknown';
        if (deposit.deposit_type && typeof deposit.deposit_type === 'object') {
          if ('MintingFee' in deposit.deposit_type) feeType = 'MintingFee';
          else if ('RedemptionFee' in deposit.deposit_type) feeType = 'RedemptionFee';
          else if ('LiquidationSurplus' in deposit.deposit_type) feeType = 'LiquidationSurplus';
          else if ('StabilityFee' in deposit.deposit_type) feeType = 'StabilityFee';
        }
        
        // Parse asset type
        let assetType = 'Unknown';
        if (deposit.asset_type && typeof deposit.asset_type === 'object') {
          if ('ICUSD' in deposit.asset_type) assetType = 'ICUSD';
          else if ('ICP' in deposit.asset_type) assetType = 'ICP';
          else if ('CKBTC' in deposit.asset_type) assetType = 'CKBTC';
        }
        
        return {
          id: Number(deposit.id || 0),
          type: 'deposit', // In future: 'withdrawal'
          feeType,
          assetType,
          amount: Number(deposit.amount || 0) / E8S,
          blockIndex: Number(deposit.block_index || 0),
          timestamp: new Date(Number(deposit.timestamp || 0) / 1000000),
          memo: deposit.memo && deposit.memo.length > 0 ? deposit.memo[0] : null
        };
      });
    } catch (error) {
      console.error('Error getting withdrawal history:', error);
      throw error;
    }
  }
}

// Add Treasury Management Component for controller UI
export class TreasuryManagementService {
  // Get summary of all collected fees
  static async getFeeSummary(): Promise<{
    totalByAsset: { [key: string]: number };
    totalByType: { [key: string]: number };
    recentActivity: any[];
  }> {
    try {
      const [status, history] = await Promise.all([
        TreasuryService.getTreasuryStatus(),
        TreasuryService.getFeeHistory()
      ]);
      
      // Calculate totals by asset type
      const totalByAsset = {
        ICUSD: status.balances.ICUSD || 0,
        ICP: status.balances.ICP || 0,
        CKBTC: status.balances.CKBTC || 0
      };
      
      // Calculate totals by fee type
      const totalByType: { [key: string]: number } = {};
      history.forEach(fee => {
        const key = `${fee.feeType}_${fee.assetType}`;
        totalByType[key] = (totalByType[key] || 0) + fee.amount;
      });
      
      // Get recent activity (last 10 items)
      const recentActivity = history.slice(0, 10);
      
      return {
        totalByAsset,
        totalByType,
        recentActivity
      };
    } catch (error) {
      console.error('Error getting fee summary:', error);
      throw error;
    }
  }
  
  // Estimate protocol revenue in USD
  static async getRevenueEstimate(icpPrice: number): Promise<{
    totalUSD: number;
    breakdown: { [key: string]: number };
  }> {
    try {
      const summary = await this.getFeeSummary();
      
      // Convert to USD values (assuming icUSD = $1)
      const icusdUSD = summary.totalByAsset.ICUSD * 1.0;  // 1:1 with USD
      const icpUSD = summary.totalByAsset.ICP * icpPrice;
      const ckbtcUSD = summary.totalByAsset.CKBTC * 50000; // Rough BTC price estimate
      
      return {
        totalUSD: icusdUSD + icpUSD + ckbtcUSD,
        breakdown: {
          icUSD: icusdUSD,
          ICP: icpUSD,
          ckBTC: ckbtcUSD
        }
      };
    } catch (error) {
      console.error('Error calculating revenue estimate:', error);
      throw error;
    }
  }
}
