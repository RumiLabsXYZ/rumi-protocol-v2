import { interpolateMultiplier } from './interpolate';

/**
 * Pure, storage/wallet-agnostic logic for the /doge/borrow guided flow:
 * persisted principal-bound progress, minted-receipt dedup, wallet-change
 * guards, open_vault_and_borrow reconciliation (it is NOT atomic — see
 * classifyOpenAndBorrowOutcome), and the calculator's risk math. No Svelte,
 * no stores, no network calls — the route wires this to live data.
 */

export const DOGE_BORROW_STORAGE_PREFIX = 'rumi_doge_borrow_intent_';
export const DOGE_BORROW_INTENT_VERSION = 2;
/** Matches TransferReceiptManager's own auto-clean window for principal-keyed flow state. */
export const DOGE_BORROW_INTENT_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

export type DogeBorrowStep = 'choose' | 'signin' | 'send' | 'confirm' | 'done';

/**
 * Which mutating call, if any, was last dispatched without a confirmed resolution.
 * 'open_and_borrow' means we do not yet know whether a vault was created at all.
 * 'finish_borrow' means a vault is already known (zero-debt) and a borrow_from_vault
 * call against that exact vault is outstanding. Cleared only once an outcome is
 * classified as 'success' or a provably no-mutation 'failed'.
 */
export type DogeBorrowPendingAction = 'open_and_borrow' | 'finish_borrow' | null;

export interface DogeBorrowIntentRecord {
  version: 2;
  principal: string;
  createdAt: number;
  updatedAt: number;
  step: DogeBorrowStep;
  collateralAmountDoge: number;
  icusdAmount: number;
  /** Deduped Minted UTXO block indices observed this record's lifetime, as decimal strings (bigint-safe). */
  mintedBlockIndices: string[];
  /** Sum of Minted koinu for the indices above, as a decimal string (bigint-safe). */
  sessionMintedKoinu: string;
  vaultId: number | null;
  borrowConfirmed: boolean;
  /**
   * Set BEFORE the mutating call is dispatched; cleared only on a resolved (non-ambiguous)
   * outcome. Optional (not required) purely so older in-memory object literals built before
   * this field existed remain structurally assignable; createInitialIntent/beginPendingAction
   * always populate it, and isValidIntentRecord still enforces its presence on anything
   * loaded from storage.
   */
  pendingAction?: DogeBorrowPendingAction;
  /** Snapshot of the account's vault ids taken immediately before an open_and_borrow attempt, so a
   * reload/crash that loses the vault id can still be reconciled read-only against this snapshot. */
  preActionVaultIds?: number[];
  /** Exact koinu (decimal bigint string) submitted in the most recent mutating attempt, or null. */
  submittedCollateralKoinu?: string | null;
  /** Human icUSD amount submitted in the most recent mutating attempt, or null (display/audit only). */
  submittedIcusdAmount?: number | null;
  /** Exact raw e8s submitted by the bound API in the most recent attempt, or null. */
  submittedIcusdAmountRaw?: string | null;
  /** True only after the backend explicitly acknowledged vault creation with zero debt. */
  partialBorrowAcknowledged?: boolean;
}

/** Default network scope for callers that don't need to distinguish environments (e.g. tests). */
export const DEFAULT_NETWORK_SCOPE = 'mainnet';

/**
 * Storage keys are scoped by BOTH principal and network (local vs mainnet): the same principal
 * text can otherwise mean a different account depending on which environment issued it, and a
 * developer switching between local and mainnet during testing must never have one environment's
 * in-flight money-operation record bleed into the other's.
 */
export function storageKeyForPrincipal(principalText: string, networkScope: string = DEFAULT_NETWORK_SCOPE): string {
  return `${DOGE_BORROW_STORAGE_PREFIX}${networkScope}_${principalText}`;
}

export function createInitialIntent(
  principalText: string,
  collateralAmountDoge: number,
  icusdAmount: number,
  now: number
): DogeBorrowIntentRecord {
  return {
    version: DOGE_BORROW_INTENT_VERSION,
    principal: principalText,
    createdAt: now,
    updatedAt: now,
    step: 'choose',
    collateralAmountDoge,
    icusdAmount,
    mintedBlockIndices: [],
    sessionMintedKoinu: '0',
    vaultId: null,
    borrowConfirmed: false,
    pendingAction: null,
    preActionVaultIds: [],
    submittedCollateralKoinu: null,
    submittedIcusdAmount: null,
    submittedIcusdAmountRaw: null,
    partialBorrowAcknowledged: false,
  };
}

function isFiniteNumber(v: unknown): v is number {
  return typeof v === 'number' && Number.isFinite(v);
}

function isDecimalBigIntString(v: unknown): v is string {
  return typeof v === 'string' && /^[0-9]+$/.test(v);
}

const VALID_STEPS: DogeBorrowStep[] = ['choose', 'signin', 'send', 'confirm', 'done'];

export function isValidIntentRecord(value: unknown): value is DogeBorrowIntentRecord {
  if (!value || typeof value !== 'object') return false;
  const r = value as Record<string, unknown>;
  return (
    r.version === DOGE_BORROW_INTENT_VERSION &&
    typeof r.principal === 'string' &&
    r.principal.length > 0 &&
    isFiniteNumber(r.createdAt) &&
    isFiniteNumber(r.updatedAt) &&
    typeof r.step === 'string' &&
    VALID_STEPS.includes(r.step as DogeBorrowStep) &&
    isFiniteNumber(r.collateralAmountDoge) &&
    isFiniteNumber(r.icusdAmount) &&
    Array.isArray(r.mintedBlockIndices) &&
    r.mintedBlockIndices.every((x) => typeof x === 'string') &&
    isDecimalBigIntString(r.sessionMintedKoinu) &&
    (r.vaultId === null || isFiniteNumber(r.vaultId)) &&
    typeof r.borrowConfirmed === 'boolean' &&
    (r.pendingAction === null || r.pendingAction === 'open_and_borrow' || r.pendingAction === 'finish_borrow') &&
    Array.isArray(r.preActionVaultIds) &&
    r.preActionVaultIds.every((x) => isFiniteNumber(x)) &&
    (r.submittedCollateralKoinu === null || isDecimalBigIntString(r.submittedCollateralKoinu)) &&
    (r.submittedIcusdAmount === null || isFiniteNumber(r.submittedIcusdAmount)) &&
    (r.submittedIcusdAmountRaw === undefined || r.submittedIcusdAmountRaw === null || isDecimalBigIntString(r.submittedIcusdAmountRaw))
    && (r.partialBorrowAcknowledged === undefined || typeof r.partialBorrowAcknowledged === 'boolean')
  );
}

/**
 * Safe parse: malformed JSON, wrong version, or missing fields all return null — never throws.
 * Age-based expiry applies ONLY to a non-pending draft (pendingAction === null): an unresolved
 * money-moving attempt (pendingAction set) must never be silently discarded just because 7 days
 * elapsed — the vault/mint it refers to still exists on-chain and needs read-only reconciliation,
 * not amnesia.
 */
export function parseStoredIntent(raw: string | null, now: number): DogeBorrowIntentRecord | null {
  if (!raw) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!isValidIntentRecord(parsed)) return null;
  if (!parsed.pendingAction && now - parsed.updatedAt > DOGE_BORROW_INTENT_MAX_AGE_MS) return null;
  return parsed;
}

export function serializeIntent(record: DogeBorrowIntentRecord): string {
  return JSON.stringify(record);
}

export function loadIntent(
  storage: Pick<Storage, 'getItem'>,
  principalText: string,
  now: number,
  networkScope: string = DEFAULT_NETWORK_SCOPE
): DogeBorrowIntentRecord | null {
  try {
    return parseStoredIntent(storage.getItem(storageKeyForPrincipal(principalText, networkScope)), now);
  } catch {
    return null;
  }
}

/** Never throws on storage failure (quota exceeded, private-browsing lockout, etc.) — returns success as a boolean. */
export function saveIntent(
  storage: Pick<Storage, 'setItem'>,
  record: DogeBorrowIntentRecord,
  networkScope: string = DEFAULT_NETWORK_SCOPE
): boolean {
  try {
    storage.setItem(storageKeyForPrincipal(record.principal, networkScope), serializeIntent(record));
    return true;
  } catch {
    return false;
  }
}

/** Clears only the given principal's record — switching/disconnecting one account never deletes another's. */
export function clearIntent(
  storage: Pick<Storage, 'removeItem'>,
  principalText: string,
  networkScope: string = DEFAULT_NETWORK_SCOPE
): void {
  try {
    storage.removeItem(storageKeyForPrincipal(principalText, networkScope));
  } catch {
    // Best-effort; a failed clear just leaves stale state to be overwritten or aged out later.
  }
}

/**
 * Writes a resolved record ONLY if it still belongs to the same intent lineage as whatever is
 * CURRENTLY persisted for that principal (or nothing is persisted yet). `createdAt` is the
 * lineage identity: it is stable for the life of one intent (never touched by beginPendingAction/
 * resolution updates) and only changes when a genuinely new intent is created. Without this guard,
 * a late continuation from an OLD session — e.g. a disconnect/reconnect of the SAME principal
 * where the user started a brand-new loan in the new session — could blindly clobber the new
 * intent with its own stale resolution. Used by call sites writing directly to storage when the
 * live Svelte state no longer belongs to this action (see isStillLiveSession).
 */
export function saveIntentIfSameLineage(
  storage: Pick<Storage, 'getItem' | 'setItem'>,
  resolved: DogeBorrowIntentRecord,
  now: number,
  networkScope: string = DEFAULT_NETWORK_SCOPE
): boolean {
  const current = loadIntent(storage, resolved.principal, now, networkScope);
  if (current && current.createdAt !== resolved.createdAt) {
    return false;
  }
  return saveIntent(storage, resolved, networkScope);
}

/**
 * Records a Minted UTXO receipt, deduped by block index, so a re-poll (e.g.
 * after a reload re-fetches the same address and polls again) never counts
 * the same deposit twice toward the collateral amount offered at Step 4.
 */
export function addMintedReceipt(
  record: DogeBorrowIntentRecord,
  blockIndex: bigint,
  koinuAmount: bigint,
  now: number
): DogeBorrowIntentRecord {
  const key = blockIndex.toString();
  if (record.mintedBlockIndices.includes(key)) return record;
  const newSum = BigInt(record.sessionMintedKoinu) + koinuAmount;
  return {
    ...record,
    mintedBlockIndices: [...record.mintedBlockIndices, key],
    sessionMintedKoinu: newSum.toString(),
    updatedAt: now,
  };
}

export type CollateralSource = 'session_deposit' | 'existing_balance_opt_in' | 'none';

/**
 * The collateral amount offered for the final borrow step. Defaults strictly
 * to what THIS session's deposit(s) actually minted — never the wallet's
 * full ckDOGE balance, which could include unrelated pre-existing ckDOGE.
 * Falling back to the wallet balance requires an explicit, separately
 * tracked user opt-in (e.g. after a NoNewUtxos reload where this session's
 * own receipt evidence is incomplete).
 *
 * Submitting collateral also costs TWO ledger fees out of that same balance: the icrc2_approve
 * call (always dispatched fresh on Oisy; dispatched whenever the standing allowance is
 * insufficient on every other wallet) charges one fee immediately, and the backend's
 * icrc2_transfer_from pull charges a second fee on top of the amount it pulls. Offering the
 * caller's entire ckDOGE balance/session-mint as the collateral amount therefore always fails
 * with InsufficientFunds — there is never enough left over to cover either fee. Reserving 2x the
 * live ledger fee here is the single source of truth for the safe amount, so validation, the
 * final confirmation screen, and the actual open_vault_and_borrow submission all agree.
 */
export const COLLATERAL_FEE_RESERVE_MULTIPLE = 2n;

export function resolveCollateralAmountForBorrow(params: {
  sessionMintedKoinu: bigint;
  walletCkdogeBalanceKoinu: bigint;
  useAvailableBalanceOptIn: boolean;
  ledgerFeeKoinu: bigint;
}): { koinuAmount: bigint; source: CollateralSource; feeReservedKoinu: bigint } {
  const feeReservedKoinu = params.ledgerFeeKoinu > 0n ? params.ledgerFeeKoinu * COLLATERAL_FEE_RESERVE_MULTIPLE : 0n;
  const safeAmount = (raw: bigint) => (raw > feeReservedKoinu ? raw - feeReservedKoinu : 0n);

  if (params.sessionMintedKoinu > 0n) {
    return { koinuAmount: safeAmount(params.sessionMintedKoinu), source: 'session_deposit', feeReservedKoinu };
  }
  if (params.useAvailableBalanceOptIn && params.walletCkdogeBalanceKoinu > 0n) {
    return { koinuAmount: safeAmount(params.walletCkdogeBalanceKoinu), source: 'existing_balance_opt_in', feeReservedKoinu };
  }
  return { koinuAmount: 0n, source: 'none', feeReservedKoinu };
}

/** Used to guard async continuations after every await: only proceed if the connected principal is unchanged. */
export function isStillSamePrincipal(captured: string | null, current: string | null): boolean {
  return captured !== null && current !== null && captured === current;
}

/**
 * Guards a mutating action's continuation against BOTH an account switch and a
 * same-principal disconnect/reconnect cycle. Principal text alone is not enough:
 * disconnecting and reconnecting the identical account is a distinct "session"
 * (the caller bumps a generation counter on every principal-store transition,
 * including null transitions), and a late callback from the OLD session must
 * never write into the new session's live state even though the principal text
 * matches again.
 */
export function isStillLiveSession(
  captured: { principalText: string | null; generation: number },
  current: { principalText: string | null; generation: number }
): boolean {
  return captured.generation === current.generation && isStillSamePrincipal(captured.principalText, current.principalText);
}

/**
 * Sets up (or clears) the record's in-flight marker. Always called BEFORE the
 * corresponding mutating call is dispatched, so a reload/crash mid-call still has
 * enough information (pendingAction + preActionVaultIds + submitted amounts) to
 * recover read-only instead of silently losing track of an in-flight attempt.
 */
export function beginPendingAction(
  record: DogeBorrowIntentRecord,
  action: Exclude<DogeBorrowPendingAction, null>,
  now: number,
  updates: Partial<Pick<DogeBorrowIntentRecord, 'preActionVaultIds' | 'submittedCollateralKoinu' | 'submittedIcusdAmount' | 'submittedIcusdAmountRaw'>> = {}
): DogeBorrowIntentRecord {
  return {
    ...record,
    pendingAction: action,
    preActionVaultIds: updates.preActionVaultIds ?? record.preActionVaultIds,
    submittedCollateralKoinu:
      updates.submittedCollateralKoinu !== undefined ? updates.submittedCollateralKoinu : record.submittedCollateralKoinu,
    submittedIcusdAmount: updates.submittedIcusdAmount !== undefined ? updates.submittedIcusdAmount : record.submittedIcusdAmount,
    submittedIcusdAmountRaw: updates.submittedIcusdAmountRaw !== undefined ? updates.submittedIcusdAmountRaw : record.submittedIcusdAmountRaw,
    partialBorrowAcknowledged: action === 'finish_borrow' ? false : record.partialBorrowAcknowledged,
    updatedAt: now,
  };
}

/**
 * What the persisted pendingAction marker should become once an outcome is
 * classified. 'success' and 'failed' (provably no-mutation) both clear it;
 * 'partial_zero_debt' narrows it to the exact remaining action (borrow on the
 * now-known vault, never another open). 'ambiguous_pending' NEVER relabels or
 * clears an in-flight marker — it always preserves whatever pendingAction was
 * already set (e.g. a still-zero debt on a recheck of a 'finish_borrow'
 * attempt must stay 'finish_borrow', not silently revert to 'open_and_borrow'
 * and imply a fresh open_vault_and_borrow would be safe).
 */
export function nextPendingActionForOutcome(
  kind: OpenAndBorrowOutcomeKind,
  currentPendingAction: DogeBorrowPendingAction
): DogeBorrowPendingAction {
  switch (kind) {
    case 'success':
    case 'failed':
      return null;
    case 'partial_zero_debt':
      return 'finish_borrow';
    case 'ambiguous_pending':
      return currentPendingAction;
  }
}

/**
 * True only for backend/pre-call rejections that are provably safe to retry
 * without risking a duplicate mutation. Verified against the actual backend
 * source (src/rumi_protocol_backend/src/vault.rs): in both open_vault_and_borrow
 * and borrow_from_vault_internal, every Err-returning check happens strictly
 * before the one state-changing step in each function (the ICRC2 collateral
 * pull + vault creation, and mint_icusd, respectively) — EXCEPT the borrow
 * sub-call failing after a vault was already created, which is always
 * embedded in the vault.rs "Vault created (id=...)" GenericError text and is
 * handled separately by extractPartialFailureVaultId, never reaching here.
 * A message that does NOT match one of these known backend/pre-call shapes
 * (e.g. a bare thrown network/timeout exception, where no typed answer was
 * ever received at all) defaults to false — never guess mutation didn't
 * happen just because the message looks unfamiliar.
 */
export function isDeterministicNoMutationError(message: string | null): boolean {
  if (!message) return false;
  const lower = message.toLowerCase();
  const DETERMINISTIC_PATTERNS = [
    'wallet not connected', // apiClient's own pre-call check, before any actor call
    'amount too low', // AmountTooLow, checked before the collateral pull / before mint_icusd
    'minimum required', // apiClient's own pre-call min-deposit check
    'minimum borrowing amount', // apiClient's own pre-call check in borrowFromVault
    'invalid borrowing amount', // apiClient's own pre-call finite/positive check
    'insufficient', // InsufficientAllowance / InsufficientFunds from the collateral pull, before vault creation
    'unexpected fee', // BadFee from the collateral pull or fee-ledger step, before vault creation / mint
    'transfer error', // mint_icusd's Err path in borrow_from_vault_internal — mint never landed
    'you must connect your wallet', // AnonymousCallerNotAllowed
    'you do not have permission', // CallerNotOwner, checked before mint_icusd
    'this operation is already in progress', // AlreadyProcessing, the very first guard check
    'service temporarily unavailable', // StaleOperation, before any mutation
    'collateral type not supported', // GenericError, before the collateral pull
    'not accepting new vaults', // GenericError, before the collateral pull
    'native-xrp collateral uses', // GenericError, before the collateral pull
    'borrowing is not allowed for this collateral type', // GenericError, before mint_icusd
    'vault not found', // GenericError, before mint_icusd
    'max borrowable', // GenericError (CR/debt-ceiling check), before mint_icusd
  ];
  return DETERMINISTIC_PATTERNS.some((p) => lower.includes(p));
}

/** icUSD uses 8 decimals on the wire, same as ckDOGE koinu. */
const ICUSD_E8S_DECIMALS = 8;

/**
 * Converts a human-units icUSD amount to the EXACT raw e8s bigint, using only string operations
 * — never a float multiply-then-floor (`Math.floor(amount * 1e8)`), which can misround because
 * `amount * 1e8` is itself a lossy IEEE-754 operation before the floor ever runs.
 * `Number.prototype.toFixed(8)` instead produces a correctly-rounded decimal-string
 * representation of the double directly (a single well-defined rounding step, not a multiply),
 * which is then parsed digit-for-digit into a bigint — no "prove it with a handful of example
 * inputs" gap, since there is no second floating-point operation left to go wrong. This value is
 * used as BOTH the actual raw amount submitted to `openVaultAndBorrowBound`/`borrowFromVaultBound`
 * (see /private/tmp/doge-boundary-interface.md) and the expected-debt figure for exact-match
 * verification — the same bigint, referenced once, never independently re-derived.
 */
export function icusdAmountToRawE8s(icusdAmount: number): bigint {
  if (!(icusdAmount > 0) || !Number.isFinite(icusdAmount)) return 0n;
  const [wholePart, fracPart = ''] = icusdAmount.toFixed(ICUSD_E8S_DECIMALS).split('.');
  return BigInt(wholePart) * 100_000_000n + BigInt(fracPart.padEnd(ICUSD_E8S_DECIMALS, '0').slice(0, ICUSD_E8S_DECIMALS));
}

export interface VaultLite {
  vaultId: number;
  collateralPrincipal: string;
  collateralAmount: bigint;
  borrowedIcusd: bigint;
}

/**
 * Every vault whose id did not exist before the intent's action started, that is denominated in
 * ckDOGE specifically (never matches a same-moment, different-collateral vault opened from
 * another tab/flow), and whose collateral is EXACTLY the expected raw amount — not a tolerance
 * band. Per src/rumi_protocol_backend/src/vault.rs, `open_vault_and_borrow` credits the vault
 * with exactly `collateral_amount_raw`, no ledger-fee deduction, so anything less than an exact
 * match is not evidence of this request's vault; a 95%-style heuristic can misattribute a
 * genuinely different, smaller deposit. Returns every match (not just the first) so the caller
 * can detect an ambiguous tie instead of silently picking one.
 */
export function findCkdogeVaultCandidates(params: {
  vaults: VaultLite[];
  beforeIds: Set<number>;
  ckdogePrincipal: string;
  expectedCollateralAmountRaw: bigint;
}): VaultLite[] {
  return params.vaults.filter(
    (v) =>
      !params.beforeIds.has(v.vaultId) &&
      v.collateralPrincipal === params.ckdogePrincipal &&
      v.collateralAmount === params.expectedCollateralAmountRaw
  );
}

/** Convenience wrapper over findCkdogeVaultCandidates for callers that only want an unambiguous single match. */
export function findMatchingCkdogeVault(params: {
  vaults: VaultLite[];
  beforeIds: Set<number>;
  ckdogePrincipal: string;
  expectedCollateralAmountRaw: bigint;
}): VaultLite | null {
  const candidates = findCkdogeVaultCandidates(params);
  return candidates.length === 1 ? candidates[0] : null;
}

/** Extracts the vault id from the backend's `open_vault_and_borrow` partial-failure GenericError text. */
export function extractPartialFailureVaultId(message: string): number | null {
  const m = /Vault created \(id=(\d+)\)/.exec(message);
  return m ? Number(m[1]) : null;
}

export type OpenAndBorrowOutcomeKind = 'success' | 'partial_zero_debt' | 'ambiguous_pending' | 'failed';

export interface OpenAndBorrowOutcome {
  kind: OpenAndBorrowOutcomeKind;
  vaultId: number | null;
  message: string;
}

/** The exact raw wire amounts THIS specific attempt submitted (or is about to submit), used to require an exact debt/collateral match rather than trusting any positive number. */
export interface ExpectedWire {
  collateralAmountRaw: bigint;
  borrowedAmountRaw: bigint;
}

/**
 * Classifies a vault's evidence against the expected exact wire amounts. Debt is never treated
 * as "close enough": per vault.rs, `record_borrow_from_vault` adds exactly the requested raw
 * icUSD amount to `borrowed_icusd_amount` (the borrowing fee is minted to treasury separately
 * and never reduces recorded debt; a fresh/zero-debt vault accrues no interest before its first
 * borrow, since `accrue_single_vault` only runs when existing debt is already nonzero) — so the
 * expected debt after a landed borrow is exactly `expected.borrowedAmountRaw`, no fee/interest
 * tolerance applies to THIS flow. A positive debt that does not match exactly is a mismatch, not
 * proof of success — reported honestly as still-pending, never silently accepted.
 */
function classifyVaultAgainstExpected(vault: VaultLite, expected: ExpectedWire): 'success' | 'zero_debt' | 'mismatch' {
  if (vault.collateralAmount !== expected.collateralAmountRaw) return 'mismatch';
  if (vault.borrowedIcusd === expected.borrowedAmountRaw) return 'success';
  if (vault.borrowedIcusd === 0n) return 'zero_debt';
  return 'mismatch';
}

/**
 * Classifies the outcome of the LIVE `open_vault_and_borrow` (or `borrow_from_vault` finish-step)
 * call's OWN resolution — the only place `partial_zero_debt` may ever originate, because only
 * here do we have the call's own explicit signal: either a direct vault id from a successful
 * response, or the backend's own "Vault created (id=X) but borrow of Y failed" text (an explicit
 * terminal partial acknowledgment). A LATER recheck/reconcile query — even one that finds the
 * SAME vault still at zero debt — must call classifyRecheckOutcome instead, never this function:
 * a query alone is never "explicit terminal backend partial acknowledgment," so it can confirm
 * success but must never manufacture a fresh partial_zero_debt or failed.
 *
 * A query-only match (no attributed id from this call's own response) is accepted as evidence
 * ONLY when it is the unique new ckDOGE vault at the exact expected collateral amount — an
 * ambiguous tie (multiple candidates) is exposed for inspection, never guessed at.
 */
export function classifyLiveOpenAndBorrowOutcome(params: {
  apiSuccess: boolean;
  apiVaultId: number | null;
  apiErrorMessage: string | null;
  vaults: VaultLite[];
  beforeIds: Set<number>;
  ckdogePrincipal: string;
  expected: ExpectedWire;
  apiErrorIsDeterministic?: boolean;
}): OpenAndBorrowOutcome {
  const { apiSuccess, apiVaultId, apiErrorMessage, vaults, beforeIds, ckdogePrincipal, expected, apiErrorIsDeterministic = false } = params;

  // The explicit partial-failure text is the strongest, most direct signal — checked first,
  // regardless of apiSuccess (it can only appear inside an error message).
  const partialId = apiErrorMessage ? extractPartialFailureVaultId(apiErrorMessage) : null;
  if (partialId !== null) {
    const seen = vaults.find((v) => v.vaultId === partialId) ?? null;
    if (seen) {
      const verdict = classifyVaultAgainstExpected(seen, expected);
      if (verdict === 'success') {
        return { kind: 'success', vaultId: partialId, message: 'Confirmed on-chain despite an error response.' };
      }
      if (verdict === 'mismatch') {
        return { kind: 'ambiguous_pending', vaultId: partialId, message: 'The vault does not exactly match the expected amount — recheck before continuing.' };
      }
    }
    // Not yet visible (replica lag) or confirmed still zero-debt: the backend told us directly
    // this exact vault was created by THIS call, so this is genuine explicit acknowledgment.
    return {
      kind: 'partial_zero_debt',
      vaultId: partialId,
      message: apiErrorMessage ?? 'Vault created but the borrow step failed. Borrow separately to finish.',
    };
  }

  const attributedVault = apiSuccess && apiVaultId !== null ? vaults.find((v) => v.vaultId === apiVaultId) ?? null : null;
  const candidates = findCkdogeVaultCandidates({ vaults, beforeIds, ckdogePrincipal, expectedCollateralAmountRaw: expected.collateralAmountRaw });

  if (attributedVault) {
    const verdict = classifyVaultAgainstExpected(attributedVault, expected);
    if (verdict === 'success') return { kind: 'success', vaultId: attributedVault.vaultId, message: 'Vault opened and icUSD borrowed.' };
    if (verdict === 'zero_debt') {
      return {
        kind: 'partial_zero_debt',
        vaultId: attributedVault.vaultId,
        message: 'Your DOGE collateral is locked in a vault, but the borrow step did not complete. Borrow separately to finish.',
      };
    }
    return { kind: 'ambiguous_pending', vaultId: attributedVault.vaultId, message: 'The vault does not exactly match the expected amount — recheck before continuing.' };
  }

  if (candidates.length === 1) {
    // A query-only candidate is never attributable proof of THIS request. Even
    // an exact collateral/debt match can be an interleaved identical action from
    // another tab, so do not adopt its id or promote it to success.
    return { kind: 'ambiguous_pending', vaultId: null, message: 'Found a possible matching vault, but this request cannot be attributed yet — recheck before continuing.' };
  }

  if (candidates.length > 1) {
    return {
      kind: 'ambiguous_pending',
      vaultId: null,
      message: `Found ${candidates.length} possible matching vaults; cannot confirm which is yours yet. Recheck shortly.`,
    };
  }

  if (apiSuccess) {
    return { kind: 'ambiguous_pending', vaultId: apiVaultId, message: 'Submitted. Confirming on-chain, check back in a moment.' };
  }

  if (!apiErrorIsDeterministic) {
    return {
      kind: 'ambiguous_pending',
      vaultId: apiVaultId,
      message: apiErrorMessage
        ? `${apiErrorMessage} The result could not be confirmed either way — recheck before trying again.`
        : 'The result could not be confirmed. Recheck before trying again.',
    };
  }

  return { kind: 'failed', vaultId: null, message: apiErrorMessage ?? 'Failed to open vault and borrow.' };
}

/**
 * Classifies the LIVE `borrow_from_vault` call's own resolution when finishing a known
 * partial_zero_debt vault. There is no "which vault" ambiguity here — the vault id is already
 * attributed — but the same exact-debt rule applies: apiClient's raw success boolean is not
 * itself proof of the intended debt; this always cross-checks the vault's actual on-chain debt
 * against the exact expected amount before calling anything a confirmed success. A deterministic
 * no-mutation error (see isDeterministicNoMutationError) safely reports as still
 * partial_zero_debt (nothing changed, safe to retry the finish step later); anything else
 * unresolved stays ambiguous_pending, never a guessed success or failure.
 */
export function classifyFinishBorrowOutcome(params: {
  apiErrorMessage: string | null;
  vaultId: number;
  vaultAfter: VaultLite | null;
  expectedBorrowedRaw: bigint;
  apiErrorIsDeterministic?: boolean;
}): OpenAndBorrowOutcome {
  const { vaultId, vaultAfter, expectedBorrowedRaw, apiErrorMessage, apiErrorIsDeterministic = false } = params;
  if (vaultAfter && vaultAfter.borrowedIcusd === expectedBorrowedRaw) {
    return { kind: 'success', vaultId, message: 'Borrow completed.' };
  }
  if (vaultAfter && vaultAfter.borrowedIcusd > 0n) {
    return { kind: 'ambiguous_pending', vaultId, message: 'The vault shows an unexpected debt amount — recheck before continuing.' };
  }
  if (apiErrorIsDeterministic) {
    return {
      kind: 'partial_zero_debt',
      vaultId,
      message: apiErrorMessage || 'Borrow failed. Your DOGE collateral is still safely locked in the vault.',
    };
  }
  return {
    kind: 'ambiguous_pending',
    vaultId,
    message: apiErrorMessage
      ? `${apiErrorMessage} The result could not be confirmed either way — recheck before trying again.`
      : 'The response was lost. Recheck on-chain before trying again.',
  };
}

/**
 * Duck-typed subset of apiClient's `BoundOpenVaultAndBorrowResult` (see
 * /private/tmp/doge-boundary-interface.md) this module needs — kept structural rather than
 * importing the real type, so this module stays network/wallet-agnostic per its own header
 * comment; the actual call site imports and passes the real typed result, which satisfies this
 * shape.
 */
export interface BoundOpenAndBorrowSignal {
  kind: 'predispatch_aborted' | 'dispatched_ok' | 'dispatched_err' | 'ambiguous_transport';
  vaultId: number | null;
  errorMessage: string | null;
}

/**
 * Adapts the shared boundary layer's typed result into classifyLiveOpenAndBorrowOutcome's
 * params. Per the boundary interface's "Coordinator acceptance clarification": 'dispatched_err'
 * (a typed Err actually returned by the canister) is now source-proven deterministic evidence —
 * vault.rs guarantees every Err-returning check runs strictly before the one state-changing step,
 * except the partial-failure shape, which classifyLiveOpenAndBorrowOutcome already detects via
 * extractPartialFailureVaultId on the (identically-formatted) error text. 'predispatch_aborted'
 * means the mutating call was never dispatched at all — also deterministic no-vault/no-borrow
 * mutation (though the caller must still separately disclose approvalMayHaveMutated in the
 * message, since an ICRC-2 approve is a distinct mutation this classification does not cover).
 * 'ambiguous_transport' (a lost/thrown reply after dispatch) is explicitly NEVER deterministic —
 * it must never be upgraded to 'failed', only 'ambiguous_pending'.
 */
export function classifyLiveOpenAndBorrowOutcomeFromBound(params: {
  signal: BoundOpenAndBorrowSignal;
  vaults: VaultLite[];
  beforeIds: Set<number>;
  ckdogePrincipal: string;
  expected: ExpectedWire;
}): OpenAndBorrowOutcome {
  const { signal, vaults, beforeIds, ckdogePrincipal, expected } = params;
  return classifyLiveOpenAndBorrowOutcome({
    apiSuccess: signal.kind === 'dispatched_ok',
    apiVaultId: signal.kind === 'dispatched_ok' ? signal.vaultId : null,
    apiErrorMessage: signal.errorMessage,
    vaults,
    beforeIds,
    ckdogePrincipal,
    expected,
    apiErrorIsDeterministic: signal.kind === 'predispatch_aborted' || signal.kind === 'dispatched_err',
  });
}

/** Duck-typed subset of apiClient's `BoundBorrowFromVaultResult` this module needs — see BoundOpenAndBorrowSignal for why this stays structural. */
export interface BoundFinishBorrowSignal {
  kind: 'predispatch_aborted' | 'dispatched_ok' | 'dispatched_err' | 'ambiguous_transport';
  vaultId: number;
  errorMessage: string | null;
}

/**
 * Adapts the shared boundary layer's typed borrow_from_vault result into
 * classifyFinishBorrowOutcome's params. Same deterministic-evidence reasoning as
 * classifyLiveOpenAndBorrowOutcomeFromBound: 'predispatch_aborted' and 'dispatched_err' are both
 * proof no borrow mutation occurred (there is no separate approval sub-step on this leg, so
 * 'predispatch_aborted' here always means nothing at all was submitted); 'ambiguous_transport'
 * never is.
 */
export function classifyFinishBorrowOutcomeFromBound(params: {
  signal: BoundFinishBorrowSignal;
  vaultAfter: VaultLite | null;
  expectedBorrowedRaw: bigint;
}): OpenAndBorrowOutcome {
  const { signal, vaultAfter, expectedBorrowedRaw } = params;
  return classifyFinishBorrowOutcome({
    apiErrorMessage: signal.errorMessage,
    vaultId: signal.vaultId,
    vaultAfter,
    expectedBorrowedRaw,
    apiErrorIsDeterministic: signal.kind === 'predispatch_aborted' || signal.kind === 'dispatched_err',
  });
}

/**
 * Classifies a QUERY-only reconciliation: a recheck after a lost response, a page reload, or
 * another tab's write. Used by recheckOutcome and reconcileForPrincipal, for both a known vault
 * id and an unknown one (reload/crash lost the response before a vault id was ever recorded —
 * reconciles against the pre-action vault-id snapshot instead of giving up). A query alone can
 * confirm an exact-match success, but — per classifyLiveOpenAndBorrowOutcome's doc comment — can
 * NEVER produce partial_zero_debt or failed: a still-zero or mismatched debt, or no vault found
 * at all, always stays ambiguous_pending, so the caller never re-offers a fresh mutating action
 * (including a resubmitted borrow_from_vault) on top of an attempt whose true resolution is still
 * unknown ("query after timeout showing zero remains pending, no resubmit").
 */
export function classifyRecheckOutcome(params: {
  knownVaultId: number | null;
  vaults: VaultLite[];
  beforeIds: Set<number>;
  ckdogePrincipal: string;
  expected: ExpectedWire;
}): OpenAndBorrowOutcome {
  const { knownVaultId, vaults, beforeIds, ckdogePrincipal, expected } = params;

  if (knownVaultId !== null) {
    const direct = vaults.find((v) => v.vaultId === knownVaultId) ?? null;
    if (!direct) {
      return { kind: 'ambiguous_pending', vaultId: knownVaultId, message: 'Still no confirmed vault found on-chain yet. Check back shortly.' };
    }
    const verdict = classifyVaultAgainstExpected(direct, expected);
    return verdict === 'success'
      ? { kind: 'success', vaultId: direct.vaultId, message: 'Vault opened and icUSD borrowed.' }
      : { kind: 'ambiguous_pending', vaultId: direct.vaultId, message: 'Still no confirmed borrow on this vault yet. Check back shortly.' };
  }

  const candidates = findCkdogeVaultCandidates({ vaults, beforeIds, ckdogePrincipal, expectedCollateralAmountRaw: expected.collateralAmountRaw });
  if (candidates.length > 1) {
    return {
      kind: 'ambiguous_pending',
      vaultId: null,
      message: `Found ${candidates.length} possible matching vaults; cannot confirm which is yours yet. Recheck shortly.`,
    };
  }
  if (candidates.length === 1) {
    // Exact query matches remain non-authoritative when the mutating call did
    // not return its vault id. Keep the pending record unattributed until a
    // typed success or explicit backend partial acknowledgment supplies it.
    return { kind: 'ambiguous_pending', vaultId: null, message: 'Found a possible matching vault, but this request cannot be attributed yet — recheck before continuing.' };
  }
  return { kind: 'ambiguous_pending', vaultId: null, message: 'No matching vault found yet. Recheck shortly.' };
}

/** True if a vault's on-chain debt already reflects a completed borrow — used to preflight a partial-zero-debt retry so a lost response can never trigger a second mint. */
export function hasVaultAlreadyBorrowed(vault: VaultLite | null): boolean {
  return !!vault && vault.borrowedIcusd > 0n;
}

export interface RiskInputs {
  collateralAmountDoge: number;
  icusdAmount: number;
  collateralPriceUsd: number;
  liquidationCr: number;
  minimumCr: number;
  borrowingFeeRate: number;
  /** Optional [crDecimal, multiplier] curve, same shape as protocolStatus.borrowingFeeCurveResolved. */
  feeCurve?: [number, number][];
}

export interface RiskOutputs {
  collateralValueUsd: number;
  collateralRatioPct: number;
  borrowFeeIcusd: number;
  icusdReceived: number;
  liquidationPriceUsd: number;
  liqPriceRatio: number;
  safetyDeltaPct: number;
  isValidCr: boolean;
}

/** Mirrors the reactive risk math in routes/+page.svelte — ckDOGE is a plain ICRC collateral, no reserve deduction. */
export function computeDogeBorrowRisk(inputs: RiskInputs): RiskOutputs {
  const { collateralAmountDoge, icusdAmount, collateralPriceUsd, liquidationCr, minimumCr, borrowingFeeRate, feeCurve } = inputs;

  const collateralValueUsd = collateralAmountDoge * collateralPriceUsd;

  const projectedCr =
    icusdAmount > 0 && collateralValueUsd > 0 ? collateralValueUsd / icusdAmount : Infinity;
  const feeMultiplier = feeCurve && feeCurve.length > 0 ? interpolateMultiplier(feeCurve, projectedCr) : 1;
  const effectiveFeeRate = borrowingFeeRate * feeMultiplier;
  const borrowFeeIcusd = icusdAmount * effectiveFeeRate;
  const icusdReceived = icusdAmount - borrowFeeIcusd;

  const collateralRatioPct =
    collateralAmountDoge > 0 && icusdAmount >= 0.001
      ? projectedCr * 100
      : collateralAmountDoge > 0
        ? Infinity
        : 0;

  const liquidationPriceUsd =
    collateralAmountDoge > 0 && icusdAmount > 0 ? (icusdAmount * liquidationCr) / collateralAmountDoge : 0;
  const liqPriceRatio = collateralPriceUsd > 0 && liquidationPriceUsd > 0 ? liquidationPriceUsd / collateralPriceUsd : 0;
  const safetyDeltaPct =
    collateralPriceUsd > 0 && liquidationPriceUsd > 0
      ? ((collateralPriceUsd - liquidationPriceUsd) / collateralPriceUsd) * 100
      : 0;

  const isValidCr = collateralRatioPct >= minimumCr * 100;

  return {
    collateralValueUsd,
    collateralRatioPct,
    borrowFeeIcusd,
    icusdReceived,
    liquidationPriceUsd,
    liqPriceRatio,
    safetyDeltaPct,
    isValidCr,
  };
}

/** 0.5% haircut off the oracle price so "Max" never requests more than the backend will accept. */
export function computeMaxBorrow(collateralAmountDoge: number, collateralPriceUsd: number, minimumCr: number): number {
  if (collateralAmountDoge <= 0 || collateralPriceUsd <= 0) return 0;
  return Math.floor(((collateralAmountDoge * collateralPriceUsd) / minimumCr) * 0.995 * 100) / 100;
}

/** For the Step 1 downside slider: "If DOGE fell by X%, your ratio would be Y%." */
export function projectRatioAtPriceDrop(
  inputs: Pick<RiskInputs, 'collateralAmountDoge' | 'icusdAmount' | 'collateralPriceUsd'>,
  dropPct: number
): { impliedPriceUsd: number; impliedCrPct: number } {
  const impliedPriceUsd = inputs.collateralPriceUsd * (1 - dropPct / 100);
  const collateralValueUsd = inputs.collateralAmountDoge * impliedPriceUsd;
  const impliedCrPct = inputs.icusdAmount > 0 ? (collateralValueUsd / inputs.icusdAmount) * 100 : 0;
  return { impliedPriceUsd, impliedCrPct };
}

export interface TermsSnapshot {
  collateralPriceUsd: number;
  collateralRatioPct: number;
  liquidationPriceUsd: number;
  borrowFeeIcusd: number;
  borrowingFeeRate?: number;
  minimumCr?: number;
  debtCeiling?: number;
  collateralStatus?: string;
  borrowingFeeCurve?: [number, number][];
}

/**
 * True if price/ratio/liquidation terms moved enough since the last snapshot
 * that the user must explicitly reconfirm before the final borrow call —
 * guards against silently borrowing against stale Step-1 numbers after the
 * ~1 hour deposit wait.
 */
export function haveTermsChangedMaterially(a: TermsSnapshot, b: TermsSnapshot, thresholdPct = 1): boolean {
  const relDiff = (x: number, y: number) => {
    if (x === 0 && y === 0) return 0;
    const denom = Math.max(Math.abs(x), Math.abs(y), 1e-9);
    return (Math.abs(x - y) / denom) * 100;
  };
  const optionalRelDiff = (x: number | undefined, y: number | undefined) =>
    x === undefined || y === undefined ? x !== y : relDiff(x, y) > thresholdPct;
  const curveChanged = (a: [number, number][] | undefined, b: [number, number][] | undefined) => {
    if (a === undefined || b === undefined) return a !== b;
    return a.length !== b.length || a.some((point, i) =>
      relDiff(point[0], b[i][0]) > thresholdPct || relDiff(point[1], b[i][1]) > thresholdPct
    );
  };
  return (
    optionalRelDiff(a.collateralRatioPct, b.collateralRatioPct) ||
    relDiff(a.collateralPriceUsd, b.collateralPriceUsd) > thresholdPct ||
    relDiff(a.liquidationPriceUsd, b.liquidationPriceUsd) > thresholdPct ||
    relDiff(a.borrowFeeIcusd, b.borrowFeeIcusd) > thresholdPct ||
    optionalRelDiff(a.borrowingFeeRate, b.borrowingFeeRate) ||
    optionalRelDiff(a.minimumCr, b.minimumCr) ||
    optionalRelDiff(a.debtCeiling, b.debtCeiling) ||
    (a.collateralStatus === undefined || b.collateralStatus === undefined
      ? a.collateralStatus !== b.collateralStatus
      : a.collateralStatus !== b.collateralStatus) ||
    curveChanged(a.borrowingFeeCurve, b.borrowingFeeCurve)
  );
}

/**
 * Final gate for the "Confirm and borrow" button. All four conditions must
 * hold: no concurrent submit already in flight, a wallet connected, that
 * wallet still matching the principal the intent was bound to, and the user
 * having explicitly reconfirmed the currently-displayed terms (reset to
 * false by the caller whenever haveTermsChangedMaterially flags a refresh).
 */
export function canSubmitBorrow(params: {
  actionInProgress: boolean;
  isConnected: boolean;
  principalMatchesIntent: boolean;
  termsConfirmed: boolean;
}): boolean {
  return (
    !params.actionInProgress &&
    params.isConnected &&
    params.principalMatchesIntent &&
    params.termsConfirmed
  );
}

// ── Cross-tab exclusive action guard ─────────────────────────────────────
//
// The single-tab `actionInProgress` flag and the persisted `pendingAction` marker (checked via
// `hasUnresolvedPendingAction` in the route) narrow most double-submits, but neither is a real
// mutual-exclusion primitive across tabs/windows: two tabs can both read "no pendingAction yet"
// before either has persisted its own write. A `storage` event listener alone does not close
// this window either — it only fires AFTER a write lands, so two tabs that both start within the
// same event loop tick can both pass the gate. The Web Locks API (`navigator.locks`) is a real
// browser-level mutex scoped to the origin, so it closes that window; where it is unavailable
// (very old Safari, non-browser test environments) the caller must fail closed — refuse to
// dispatch rather than proceed unprotected — per the coordinator's "conservative disabled
// behavior if unavailable" decision.

/** Web Locks API surface this module depends on — narrow on purpose so tests can inject a fake. */
export interface ExclusiveLocksLike {
  request<T>(name: string, options: { ifAvailable: boolean }, callback: (lock: unknown | null) => Promise<T>): Promise<T>;
}

/** One lock per (principal, network/canister) pair — matches the intent's own storage-key scoping. */
export function dogeBorrowActionLockName(principalText: string, networkScope: string): string {
  return `rumi_doge_borrow_action_lock_${networkScope}_${principalText}`;
}

export type ExclusiveActionResult<T> = { ran: true; result: T } | { ran: false; reason: 'locked' | 'unsupported' };

/**
 * Runs `action` exclusively for `lockName`, using `{ ifAvailable: true }` so a contending tab
 * fails fast (never queues behind the lock and then fires a stale/late duplicate once released).
 * When `locks` is null/undefined (unsupported), fails closed with reason 'unsupported' — the
 * caller must treat this the same as 'locked' (refuse to proceed), not fall through to an
 * unprotected dispatch.
 */
export async function runExclusiveAction<T>(
  locks: ExclusiveLocksLike | null | undefined,
  lockName: string,
  action: () => Promise<T>
): Promise<ExclusiveActionResult<T>> {
  if (!locks) {
    return { ran: false, reason: 'unsupported' };
  }
  let acquired = false;
  let result: T | undefined;
  await locks.request(lockName, { ifAvailable: true }, async (lock) => {
    if (!lock) return;
    acquired = true;
    result = await action();
  });
  if (!acquired) {
    return { ran: false, reason: 'locked' };
  }
  return { ran: true, result: result as T };
}
