import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  DOGE_BORROW_INTENT_VERSION,
  DOGE_BORROW_INTENT_MAX_AGE_MS,
  storageKeyForPrincipal,
  createInitialIntent,
  parseStoredIntent,
  loadIntent,
  saveIntent,
  saveIntentIfSameLineage,
  clearIntent,
  addMintedReceipt,
  resolveCollateralAmountForBorrow,
  isStillSamePrincipal,
  isStillLiveSession,
  beginPendingAction,
  nextPendingActionForOutcome,
  isDeterministicNoMutationError,
  icusdAmountToRawE8s,
  hasVaultAlreadyBorrowed,
  findMatchingCkdogeVault,
  findCkdogeVaultCandidates,
  extractPartialFailureVaultId,
  classifyLiveOpenAndBorrowOutcome,
  classifyLiveOpenAndBorrowOutcomeFromBound,
  classifyFinishBorrowOutcome,
  classifyFinishBorrowOutcomeFromBound,
  classifyRecheckOutcome,
  dogeBorrowActionLockName,
  runExclusiveAction,
  computeDogeBorrowRisk,
  computeMaxBorrow,
  projectRatioAtPriceDrop,
  haveTermsChangedMaterially,
  canSubmitBorrow,
  type VaultLite,
  type ExpectedWire,
  type ExclusiveLocksLike,
} from './dogeBorrowWizard';

const OWNER = 'aaaaa-aa';
const OTHER_OWNER = 'bbbbb-bb';
const CKDOGE_PRINCIPAL = 'efmc5-wyaaa-aaaar-qb3wa-cai';
const ICP_PRINCIPAL = 'ryjl3-tyaaa-aaaaa-aaaba-cai';

describe('storage key + persisted intent (principal-bound, versioned, safe parse)', () => {
  beforeEach(() => localStorage.clear());

  it('keys storage per principal', () => {
    expect(storageKeyForPrincipal(OWNER)).not.toBe(storageKeyForPrincipal(OTHER_OWNER));
    expect(storageKeyForPrincipal(OWNER)).toContain(OWNER);
  });

  it('keys storage per network scope too, so a local-environment record never collides with a mainnet one for the same principal text', () => {
    expect(storageKeyForPrincipal(OWNER, 'local')).not.toBe(storageKeyForPrincipal(OWNER, 'mainnet'));
  });

  it('loadIntent/saveIntent/clearIntent are isolated per network scope', () => {
    const record = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    saveIntent(localStorage, record, 'local');
    expect(loadIntent(localStorage, OWNER, 1_700_000_000_000, 'mainnet')).toBeNull();
    expect(loadIntent(localStorage, OWNER, 1_700_000_000_000, 'local')).toEqual(record);
    clearIntent(localStorage, OWNER, 'mainnet');
    expect(loadIntent(localStorage, OWNER, 1_700_000_000_000, 'local')).toEqual(record);
  });

  it('round-trips a freshly created intent through save/load', () => {
    const record = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    expect(saveIntent(localStorage, record)).toBe(true);
    const loaded = loadIntent(localStorage, OWNER, 1_700_000_000_000);
    expect(loaded).toEqual(record);
  });

  it('never throws when storage.setItem rejects the write, and reports failure', () => {
    const throwingStorage: Pick<Storage, 'setItem'> = {
      setItem: () => {
        throw new Error('QuotaExceededError');
      },
    };
    const record = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    expect(() => saveIntent(throwingStorage, record)).not.toThrow();
    expect(saveIntent(throwingStorage, record)).toBe(false);
  });

  it('returns null for missing storage entries', () => {
    expect(loadIntent(localStorage, OWNER, 1_700_000_000_000)).toBeNull();
  });

  it('returns null and does not throw for malformed JSON', () => {
    localStorage.setItem(storageKeyForPrincipal(OWNER), '{not json');
    expect(parseStoredIntent('{not json', 1_700_000_000_000)).toBeNull();
    expect(loadIntent(localStorage, OWNER, 1_700_000_000_000)).toBeNull();
  });

  it('returns null for a record with the wrong version (future schema change)', () => {
    const record = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    const raw = JSON.stringify({ ...record, version: 999 });
    expect(parseStoredIntent(raw, 1_700_000_000_000)).toBeNull();
  });

  it('returns null for a record missing required fields', () => {
    const raw = JSON.stringify({ version: DOGE_BORROW_INTENT_VERSION, principal: OWNER });
    expect(parseStoredIntent(raw, 1_700_000_000_000)).toBeNull();
  });

  it('returns null for a record with non-finite numeric fields', () => {
    const record = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    const raw = JSON.stringify({ ...record, collateralAmountDoge: Number.NaN });
    expect(parseStoredIntent(raw, 1_700_000_000_000)).toBeNull();
  });

  it('treats a record older than the max age as stale and returns null', () => {
    const record = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    const raw = JSON.stringify(record);
    const farFuture = 1_700_000_000_000 + DOGE_BORROW_INTENT_MAX_AGE_MS + 1;
    expect(parseStoredIntent(raw, farFuture)).toBeNull();
  });

  it('accepts a record right at the age boundary', () => {
    const record = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    const raw = JSON.stringify(record);
    const justBefore = 1_700_000_000_000 + DOGE_BORROW_INTENT_MAX_AGE_MS - 1;
    expect(parseStoredIntent(raw, justBefore)).not.toBeNull();
  });

  it('never expires an UNRESOLVED money-operation record (pendingAction set), no matter how old', () => {
    const base = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    const pending = beginPendingAction(base, 'open_and_borrow', 1_700_000_000_000, {
      preActionVaultIds: [],
      submittedCollateralKoinu: '100000000000',
      submittedIcusdAmount: 50,
    });
    const raw = JSON.stringify(pending);
    const farFuture = 1_700_000_000_000 + DOGE_BORROW_INTENT_MAX_AGE_MS * 10;
    const loaded = parseStoredIntent(raw, farFuture);
    expect(loaded).not.toBeNull();
    expect(loaded?.pendingAction).toBe('open_and_borrow');
  });

  it('still expires a non-pending draft (pendingAction null) past the age cutoff', () => {
    const record = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    expect(record.pendingAction).toBeNull();
    const raw = JSON.stringify(record);
    const farFuture = 1_700_000_000_000 + DOGE_BORROW_INTENT_MAX_AGE_MS + 1;
    expect(parseStoredIntent(raw, farFuture)).toBeNull();
  });

  it('clearIntent removes only the given principal key, never throws on failure', () => {
    const recordA = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    const recordB = createInitialIntent(OTHER_OWNER, 500, 25, 1_700_000_000_000);
    saveIntent(localStorage, recordA);
    saveIntent(localStorage, recordB);

    clearIntent(localStorage, OWNER);

    expect(loadIntent(localStorage, OWNER, 1_700_000_000_000)).toBeNull();
    // Switching/clearing one principal's record must never delete another's.
    expect(loadIntent(localStorage, OTHER_OWNER, 1_700_000_000_000)).not.toBeNull();
  });
});

describe('saveIntentIfSameLineage (a late completion from an old session must never clobber a newer intent)', () => {
  beforeEach(() => localStorage.clear());

  it('writes normally when nothing is currently persisted for this principal', () => {
    const record = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    expect(saveIntentIfSameLineage(localStorage, record, 1_700_000_001_000)).toBe(true);
    expect(loadIntent(localStorage, OWNER, 1_700_000_001_000)).toEqual(record);
  });

  it('writes when the persisted record is the SAME lineage (same createdAt), e.g. a later resolution of the same intent', () => {
    const original = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    saveIntent(localStorage, original);
    const resolved = { ...original, vaultId: 7, borrowConfirmed: true, updatedAt: 1_700_000_002_000 };
    expect(saveIntentIfSameLineage(localStorage, resolved, 1_700_000_003_000)).toBe(true);
    expect(loadIntent(localStorage, OWNER, 1_700_000_003_000)?.vaultId).toBe(7);
  });

  it('refuses to write a stale completion whose createdAt no longer matches a NEWER intent already persisted for the same principal', () => {
    const oldIntent = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    // The old session's late resolution, still carrying the OLD createdAt.
    const staleResolution = { ...oldIntent, vaultId: 7, borrowConfirmed: true, updatedAt: 1_700_000_005_000 };

    // Meanwhile a NEW session (disconnect/reconnect of the same principal) started a brand-new intent.
    const newIntent = createInitialIntent(OWNER, 2000, 90, 1_700_000_003_000);
    saveIntent(localStorage, newIntent);

    expect(saveIntentIfSameLineage(localStorage, staleResolution, 1_700_000_005_000)).toBe(false);
    // The new intent must be untouched.
    expect(loadIntent(localStorage, OWNER, 1_700_000_005_000)).toEqual(newIntent);
  });
});

describe('duplicate mint receipt dedup (no double-counting a deposit)', () => {
  it('sums a new Minted receipt into sessionMintedKoinu', () => {
    const record = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    const updated = addMintedReceipt(record, 42n, 100_000_000n, 1_700_000_001_000);
    expect(updated.sessionMintedKoinu).toBe('100000000');
    expect(updated.mintedBlockIndices).toEqual(['42']);
  });

  it('ignores a repeat of the same block index (e.g. a re-poll after reload)', () => {
    const record = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    const once = addMintedReceipt(record, 42n, 100_000_000n, 1_700_000_001_000);
    const twice = addMintedReceipt(once, 42n, 100_000_000n, 1_700_000_002_000);
    expect(twice.sessionMintedKoinu).toBe('100000000');
    expect(twice.mintedBlockIndices).toEqual(['42']);
  });

  it('accumulates two distinct block indices', () => {
    const record = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);
    const first = addMintedReceipt(record, 42n, 100_000_000n, 1_700_000_001_000);
    const second = addMintedReceipt(first, 43n, 50_000_000n, 1_700_000_002_000);
    expect(second.sessionMintedKoinu).toBe('150000000');
    expect(second.mintedBlockIndices).toEqual(['42', '43']);
  });
});

describe('existing-balance protection (never silently sweep pre-existing ckDOGE)', () => {
  it('uses only this session\'s minted koinu when it is nonzero, ignoring wallet balance', () => {
    const result = resolveCollateralAmountForBorrow({
      sessionMintedKoinu: 100_000_000n,
      walletCkdogeBalanceKoinu: 5_000_000_000n, // large pre-existing balance
      useAvailableBalanceOptIn: false,
    });
    expect(result).toEqual({ koinuAmount: 100_000_000n, source: 'session_deposit' });
  });

  it('never uses the wallet balance unless the user explicitly opts in', () => {
    const result = resolveCollateralAmountForBorrow({
      sessionMintedKoinu: 0n,
      walletCkdogeBalanceKoinu: 5_000_000_000n,
      useAvailableBalanceOptIn: false,
    });
    expect(result).toEqual({ koinuAmount: 0n, source: 'none' });
  });

  it('uses the wallet balance only after explicit opt-in, labeled honestly', () => {
    const result = resolveCollateralAmountForBorrow({
      sessionMintedKoinu: 0n,
      walletCkdogeBalanceKoinu: 5_000_000_000n,
      useAvailableBalanceOptIn: true,
    });
    expect(result).toEqual({ koinuAmount: 5_000_000_000n, source: 'existing_balance_opt_in' });
  });
});

describe('principal-switch guard (mid-await reconciliation)', () => {
  it('matches only when both sides are the same non-null principal', () => {
    expect(isStillSamePrincipal(OWNER, OWNER)).toBe(true);
    expect(isStillSamePrincipal(OWNER, OTHER_OWNER)).toBe(false);
    expect(isStillSamePrincipal(OWNER, null)).toBe(false);
    expect(isStillSamePrincipal(null, null)).toBe(false);
  });
});

describe('unrelated-new-vault false match guard (EXACT collateral match, no 95% tolerance)', () => {
  const before = new Set([1, 2]);
  const EXPECTED = 100_000_000n;

  it('never matches a new vault of a different collateral type', () => {
    const vaults: VaultLite[] = [
      { vaultId: 3, collateralPrincipal: ICP_PRINCIPAL, collateralAmount: EXPECTED, borrowedIcusd: 5_000_000n },
    ];
    const match = findMatchingCkdogeVault({ vaults, beforeIds: before, ckdogePrincipal: CKDOGE_PRINCIPAL, expectedCollateralAmountRaw: EXPECTED });
    expect(match).toBeNull();
  });

  it('never matches a vault that already existed before the intent', () => {
    const vaults: VaultLite[] = [
      { vaultId: 2, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: EXPECTED, borrowedIcusd: 5_000_000n },
    ];
    const match = findMatchingCkdogeVault({ vaults, beforeIds: before, ckdogePrincipal: CKDOGE_PRINCIPAL, expectedCollateralAmountRaw: EXPECTED });
    expect(match).toBeNull();
  });

  it('never matches a new ckDOGE vault whose collateral is even slightly below the exact intended amount (no 95% tolerance)', () => {
    const vaults: VaultLite[] = [
      { vaultId: 3, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 99_000_000n, borrowedIcusd: 0n },
    ];
    const match = findMatchingCkdogeVault({ vaults, beforeIds: before, ckdogePrincipal: CKDOGE_PRINCIPAL, expectedCollateralAmountRaw: EXPECTED });
    expect(match).toBeNull();
  });

  it('never matches a new ckDOGE vault whose collateral is even slightly ABOVE the exact intended amount', () => {
    const vaults: VaultLite[] = [
      { vaultId: 3, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_001n, borrowedIcusd: 0n },
    ];
    const match = findMatchingCkdogeVault({ vaults, beforeIds: before, ckdogePrincipal: CKDOGE_PRINCIPAL, expectedCollateralAmountRaw: EXPECTED });
    expect(match).toBeNull();
  });

  it('matches a new ckDOGE vault at exactly the expected collateral amount', () => {
    const vaults: VaultLite[] = [
      { vaultId: 3, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: EXPECTED, borrowedIcusd: 0n },
    ];
    const match = findMatchingCkdogeVault({ vaults, beforeIds: before, ckdogePrincipal: CKDOGE_PRINCIPAL, expectedCollateralAmountRaw: EXPECTED });
    expect(match).toEqual(vaults[0]);
  });

  it('refuses to pick either candidate when TWO new ckDOGE vaults tie on the exact expected amount (ambiguous, no proof of attribution)', () => {
    const vaults: VaultLite[] = [
      { vaultId: 3, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: EXPECTED, borrowedIcusd: 0n },
      { vaultId: 4, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: EXPECTED, borrowedIcusd: 0n },
    ];
    const match = findMatchingCkdogeVault({ vaults, beforeIds: before, ckdogePrincipal: CKDOGE_PRINCIPAL, expectedCollateralAmountRaw: EXPECTED });
    expect(match).toBeNull();
    const candidates = findCkdogeVaultCandidates({ vaults, beforeIds: before, ckdogePrincipal: CKDOGE_PRINCIPAL, expectedCollateralAmountRaw: EXPECTED });
    expect(candidates).toHaveLength(2);
  });
});

describe('extractPartialFailureVaultId', () => {
  it('extracts the vault id from the backend\'s partial-failure GenericError text', () => {
    const msg = 'Vault created (id=42) but borrow of 5000000000 failed: TemporarilyUnavailable. You can borrow separately.';
    expect(extractPartialFailureVaultId(msg)).toBe(42);
  });

  it('returns null for unrelated error text', () => {
    expect(extractPartialFailureVaultId('Insufficient allowance')).toBeNull();
  });
});

const EXPECTED_WIRE: ExpectedWire = { collateralAmountRaw: 100_000_000n, borrowedAmountRaw: 5_000_000n };

describe('classifyLiveOpenAndBorrowOutcome (the LIVE call\'s own resolution — the only source of partial_zero_debt)', () => {
  it('classifies success when the API reports success and the attributed vault has EXACTLY the expected debt', () => {
    const vault: VaultLite = { vaultId: 7, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 5_000_000n };
    const outcome = classifyLiveOpenAndBorrowOutcome({
      apiSuccess: true, apiVaultId: 7, apiErrorMessage: null,
      vaults: [vault], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome).toEqual({ kind: 'success', vaultId: 7, message: expect.any(String) });
  });

  it('does NOT classify success on a POSITIVE but WRONG debt — reports honest mismatch/pending, never success', () => {
    const vault: VaultLite = { vaultId: 7, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 4_000_000n };
    const outcome = classifyLiveOpenAndBorrowOutcome({
      apiSuccess: true, apiVaultId: 7, apiErrorMessage: null,
      vaults: [vault], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
    expect(outcome.kind).not.toBe('success');
  });

  it('classifies partial_zero_debt when the API reports success but the attributed vault has zero debt (open_vault_and_borrow is not atomic)', () => {
    const vault: VaultLite = { vaultId: 7, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 0n };
    const outcome = classifyLiveOpenAndBorrowOutcome({
      apiSuccess: true, apiVaultId: 7, apiErrorMessage: null,
      vaults: [vault], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('partial_zero_debt');
    expect(outcome.vaultId).toBe(7);
  });

  it('classifies partial_zero_debt from the backend GenericError text alone, even with no reconciled vault yet (explicit terminal acknowledgment)', () => {
    const outcome = classifyLiveOpenAndBorrowOutcome({
      apiSuccess: false, apiVaultId: null,
      apiErrorMessage: 'Vault created (id=99) but borrow of 5000000000 failed: GenericError. You can borrow separately.',
      vaults: [], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('partial_zero_debt');
    expect(outcome.vaultId).toBe(99);
  });

  it('keeps a UNIQUE query candidate ambiguous when the API call did not return an authoritative vault id', () => {
    const vault: VaultLite = { vaultId: 11, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 5_000_000n };
    const outcome = classifyLiveOpenAndBorrowOutcome({
      apiSuccess: false, apiVaultId: null, apiErrorMessage: 'Wallet signature request timed out',
      vaults: [vault], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
    expect(outcome.vaultId).toBeNull();
  });

  it('does NOT classify partial_zero_debt from a query-only unique candidate (no explicit backend signal) — stays ambiguous_pending', () => {
    const vault: VaultLite = { vaultId: 12, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 0n };
    const outcome = classifyLiveOpenAndBorrowOutcome({
      apiSuccess: false, apiVaultId: null, apiErrorMessage: 'Wallet signature request timed out',
      vaults: [vault], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
    expect(outcome.vaultId).toBeNull();
  });

  it('classifies ambiguous_pending when the API reports success but no vault is visible yet (query lag), never treated as failure or retried blindly', () => {
    const outcome = classifyLiveOpenAndBorrowOutcome({
      apiSuccess: true, apiVaultId: 20, apiErrorMessage: null,
      vaults: [], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
    expect(outcome.vaultId).toBe(20);
  });

  it('refuses to pick a vault when TWO candidates tie on the exact expected amount — exposes the ambiguity instead of guessing', () => {
    const v1: VaultLite = { vaultId: 21, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 0n };
    const v2: VaultLite = { vaultId: 22, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 0n };
    const outcome = classifyLiveOpenAndBorrowOutcome({
      apiSuccess: false, apiVaultId: null, apiErrorMessage: 'Wallet signature request timed out',
      vaults: [v1, v2], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
    expect(outcome.vaultId).toBeNull();
  });

  it('classifies failed only when the failure is asserted deterministic (proven no-mutation), no partial-failure vault id, and no vault reconciles', () => {
    const outcome = classifyLiveOpenAndBorrowOutcome({
      apiSuccess: false, apiVaultId: null, apiErrorMessage: 'Amount too low. Minimum amount: 500000000 raw units',
      vaults: [], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
      apiErrorIsDeterministic: true,
    });
    expect(outcome.kind).toBe('failed');
    expect(outcome.vaultId).toBeNull();
  });

  it('defaults an unrecognized/unproven failure to ambiguous_pending, never "failed" (no blind retry on an unresolved transport result)', () => {
    const outcome = classifyLiveOpenAndBorrowOutcome({
      apiSuccess: false, apiVaultId: null, apiErrorMessage: 'Wallet signature request timed out',
      vaults: [], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
      // apiErrorIsDeterministic omitted — must default to false/uncertain, not guess "failed".
    });
    expect(outcome.kind).toBe('ambiguous_pending');
  });

  it('defaults to ambiguous_pending even with no error message at all', () => {
    const outcome = classifyLiveOpenAndBorrowOutcome({
      apiSuccess: false, apiVaultId: null, apiErrorMessage: null,
      vaults: [], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
  });
});

describe('classifyLiveOpenAndBorrowOutcomeFromBound (adapter for the shared boundary layer\'s typed result)', () => {
  it('classifies success from a dispatched_ok signal with a matching vault id and exact debt', () => {
    const vault: VaultLite = { vaultId: 7, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 5_000_000n };
    const outcome = classifyLiveOpenAndBorrowOutcomeFromBound({
      signal: { kind: 'dispatched_ok', vaultId: 7, errorMessage: null },
      vaults: [vault], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('success');
  });

  it('treats predispatch_aborted as deterministic (safe to retry/edit), never ambiguous', () => {
    const outcome = classifyLiveOpenAndBorrowOutcomeFromBound({
      signal: { kind: 'predispatch_aborted', vaultId: null, errorMessage: 'Invalid collateral amount.' },
      vaults: [], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('failed');
  });

  it('treats a typed dispatched_err as deterministic (no substring whitelist needed) when it is not a partial-failure shape', () => {
    const outcome = classifyLiveOpenAndBorrowOutcomeFromBound({
      signal: { kind: 'dispatched_err', vaultId: null, errorMessage: 'Some brand-new backend error text never seen before' },
      vaults: [], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('failed');
  });

  it('still routes a dispatched_err partial-failure shape to partial_zero_debt', () => {
    const outcome = classifyLiveOpenAndBorrowOutcomeFromBound({
      signal: { kind: 'dispatched_err', vaultId: null, errorMessage: 'Vault created (id=42) but borrow of 5000000000 failed: GenericError. You can borrow separately.' },
      vaults: [], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('partial_zero_debt');
    expect(outcome.vaultId).toBe(42);
  });

  it('never treats ambiguous_transport as deterministic — stays ambiguous_pending, never failed', () => {
    const outcome = classifyLiveOpenAndBorrowOutcomeFromBound({
      signal: { kind: 'ambiguous_transport', vaultId: null, errorMessage: 'Network error after dispatch.' },
      vaults: [], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
  });
});

describe('classifyFinishBorrowOutcomeFromBound (adapter for the shared boundary layer\'s typed borrow_from_vault result)', () => {
  it('classifies success when vaultAfter shows exactly the expected debt', () => {
    const outcome = classifyFinishBorrowOutcomeFromBound({
      signal: { kind: 'dispatched_ok', vaultId: 7, errorMessage: null },
      vaultAfter: { vaultId: 7, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 5_000_000n },
      expectedBorrowedRaw: 5_000_000n,
    });
    expect(outcome.kind).toBe('success');
  });

  it('treats a typed dispatched_err as deterministic partial_zero_debt (safe to retry later)', () => {
    const outcome = classifyFinishBorrowOutcomeFromBound({
      signal: { kind: 'dispatched_err', vaultId: 7, errorMessage: 'Vault not found.' },
      vaultAfter: { vaultId: 7, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 0n },
      expectedBorrowedRaw: 5_000_000n,
    });
    expect(outcome.kind).toBe('partial_zero_debt');
  });

  it('never treats ambiguous_transport as deterministic — stays ambiguous_pending', () => {
    const outcome = classifyFinishBorrowOutcomeFromBound({
      signal: { kind: 'ambiguous_transport', vaultId: 7, errorMessage: 'Network error after dispatch.' },
      vaultAfter: { vaultId: 7, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 0n },
      expectedBorrowedRaw: 5_000_000n,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
  });
});

describe('classifyFinishBorrowOutcome (LIVE borrow_from_vault resolution — never trusts the raw success boolean alone)', () => {
  it('classifies success only when the post-call vault debt EXACTLY matches the expected amount', () => {
    const outcome = classifyFinishBorrowOutcome({
      apiErrorMessage: null,
      vaultId: 7,
      vaultAfter: { vaultId: 7, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 5_000_000n },
      expectedBorrowedRaw: 5_000_000n,
    });
    expect(outcome.kind).toBe('success');
  });

  it('does not classify success when the API said success but the vault debt is a positive MISMATCH', () => {
    const outcome = classifyFinishBorrowOutcome({
      apiErrorMessage: null,
      vaultId: 7,
      vaultAfter: { vaultId: 7, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 1n },
      expectedBorrowedRaw: 5_000_000n,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
  });

  it('reports partial_zero_debt (safe to retry later) only for a deterministic no-mutation error', () => {
    const outcome = classifyFinishBorrowOutcome({
      apiErrorMessage: 'Insufficient allowance',
      vaultId: 7,
      vaultAfter: { vaultId: 7, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 0n },
      expectedBorrowedRaw: 5_000_000n,
      apiErrorIsDeterministic: true,
    });
    expect(outcome.kind).toBe('partial_zero_debt');
  });

  it('stays ambiguous_pending (never partial_zero_debt) for an unresolved transport error, even with zero debt observed', () => {
    const outcome = classifyFinishBorrowOutcome({
      apiErrorMessage: 'The response was lost',
      vaultId: 7,
      vaultAfter: { vaultId: 7, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 0n },
      expectedBorrowedRaw: 5_000_000n,
      apiErrorIsDeterministic: false,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
  });

  it('stays ambiguous_pending when the post-call vault fetch itself failed (vaultAfter null) and the error is not deterministic', () => {
    const outcome = classifyFinishBorrowOutcome({
      apiErrorMessage: 'Wallet signature request timed out',
      vaultId: 7,
      vaultAfter: null,
      expectedBorrowedRaw: 5_000_000n,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
  });
});

describe('classifyRecheckOutcome (query-only reconciliation — can confirm success, can NEVER produce partial_zero_debt or failed)', () => {
  it('classifies success on a known vault id with the exact expected debt', () => {
    const vault: VaultLite = { vaultId: 7, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 5_000_000n };
    const outcome = classifyRecheckOutcome({
      knownVaultId: 7, vaults: [vault], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('success');
    expect(outcome.vaultId).toBe(7);
  });

  it('a known vault still at zero debt stays ambiguous_pending — NEVER partial_zero_debt (a query is not an explicit backend acknowledgment)', () => {
    const vault: VaultLite = { vaultId: 7, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 0n };
    const outcome = classifyRecheckOutcome({
      knownVaultId: 7, vaults: [vault], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
    expect(outcome.vaultId).toBe(7);
  });

  it('a known vault with a positive but WRONG debt stays ambiguous_pending, never success', () => {
    const vault: VaultLite = { vaultId: 7, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 1n };
    const outcome = classifyRecheckOutcome({
      knownVaultId: 7, vaults: [vault], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
  });

  it('a known vault id not found in the query (no-vault query) stays ambiguous_pending, not failed', () => {
    const outcome = classifyRecheckOutcome({
      knownVaultId: 7, vaults: [], beforeIds: new Set([1]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
    expect(outcome.vaultId).toBe(7);
  });

  it('keeps a unique candidate snapshot ambiguous when the vault id was unknown (reload/crash lost the response)', () => {
    const vault: VaultLite = { vaultId: 5, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 5_000_000n };
    const outcome = classifyRecheckOutcome({
      knownVaultId: null, vaults: [vault], beforeIds: new Set([1, 2]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
    expect(outcome.vaultId).toBeNull();
  });

  it('a zero-debt unique candidate (unknown vault id) stays ambiguous_pending, never partial_zero_debt', () => {
    const vault: VaultLite = { vaultId: 5, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 0n };
    const outcome = classifyRecheckOutcome({
      knownVaultId: null, vaults: [vault], beforeIds: new Set([1, 2]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
    expect(outcome.vaultId).toBeNull();
  });

  it('does not adopt the only new candidate when an identical vault was interleaved before the action', () => {
    const outcome = classifyRecheckOutcome({
      knownVaultId: null,
      vaults: [
        { vaultId: 5, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 5_000_000n },
        { vaultId: 6, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 5_000_000n },
      ],
      beforeIds: new Set([5]),
      ckdogePrincipal: CKDOGE_PRINCIPAL,
      expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
    expect(outcome.vaultId).toBeNull();
  });

  it('stays ambiguous_pending (not failed) when no matching vault is found yet, so the caller can recheck again rather than assume failure', () => {
    const outcome = classifyRecheckOutcome({
      knownVaultId: null,
      vaults: [{ vaultId: 1, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 1n, borrowedIcusd: 0n }],
      beforeIds: new Set([1, 2]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
    expect(outcome.vaultId).toBeNull();
  });

  it('never matches a vault that already existed before the action (beforeIds exclusion still applies)', () => {
    const outcome = classifyRecheckOutcome({
      knownVaultId: null,
      vaults: [{ vaultId: 2, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 5_000_000n }],
      beforeIds: new Set([1, 2]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
  });

  it('exposes (never guesses) an ambiguous tie between two unknown-id candidates', () => {
    const v1: VaultLite = { vaultId: 5, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 5_000_000n };
    const v2: VaultLite = { vaultId: 6, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 100_000_000n, borrowedIcusd: 5_000_000n };
    const outcome = classifyRecheckOutcome({
      knownVaultId: null, vaults: [v1, v2], beforeIds: new Set([1, 2]), ckdogePrincipal: CKDOGE_PRINCIPAL, expected: EXPECTED_WIRE,
    });
    expect(outcome.kind).toBe('ambiguous_pending');
    expect(outcome.vaultId).toBeNull();
  });
});

describe('isDeterministicNoMutationError (proven no-mutation vs unresolved transport result)', () => {
  it('recognizes known pre-mutation backend/pre-call rejections as deterministic', () => {
    expect(isDeterministicNoMutationError('Wallet not connected. Please connect your wallet and try again.')).toBe(true);
    expect(isDeterministicNoMutationError('Amount too low. Minimum required: 5 DOGE')).toBe(true);
    expect(isDeterministicNoMutationError('Invalid borrowing amount: -1. Amount must be a finite positive number.')).toBe(true);
    expect(isDeterministicNoMutationError('Insufficient allowance (have: 0). Please approve the tokens first.')).toBe(true);
    expect(isDeterministicNoMutationError('This operation is already in progress. Please wait.')).toBe(true);
    expect(isDeterministicNoMutationError('Service temporarily unavailable: cleanup in progress')).toBe(true);
    expect(isDeterministicNoMutationError('Transfer error: {"BadFee":{}}')).toBe(true);
    expect(isDeterministicNoMutationError('Collateral type is not accepting new vaults.')).toBe(true);
    expect(isDeterministicNoMutationError('Borrowing is not allowed for this collateral type.')).toBe(true);
    expect(isDeterministicNoMutationError('Vault not found. Please check the vault ID.')).toBe(true);
  });

  it('never treats a bare transport/timeout exception, or an unfamiliar message, as deterministic', () => {
    expect(isDeterministicNoMutationError('Wallet signature request timed out')).toBe(false);
    expect(isDeterministicNoMutationError('Unknown error opening vault')).toBe(false);
    expect(isDeterministicNoMutationError('Failed to fetch')).toBe(false);
    expect(isDeterministicNoMutationError('An error occurred with the operation')).toBe(false);
    expect(isDeterministicNoMutationError(null)).toBe(false);
    expect(isDeterministicNoMutationError('')).toBe(false);
  });

  it('never lets the "Vault created (id=...)" partial-failure message masquerade as a plain deterministic no-mutation error on its own text alone', () => {
    // Guarded in practice by classifyOpenAndBorrowOutcome checking extractPartialFailureVaultId
    // FIRST, unconditionally — this only documents that the text itself doesn't match our safe list.
    const msg = 'Vault created (id=42) but borrow of 5000000000 failed: GenericError. You can borrow separately.';
    expect(isDeterministicNoMutationError(msg)).toBe(false);
  });
});

describe('icusdAmountToRawE8s (exact string-based conversion — the ONE value used for both submission and expected-debt verification)', () => {
  it('converts round and fractional amounts to the exact expected raw e8s', () => {
    expect(icusdAmountToRawE8s(1)).toBe(100_000_000n);
    expect(icusdAmountToRawE8s(50)).toBe(5_000_000_000n);
    expect(icusdAmountToRawE8s(49.84)).toBe(4_984_000_000n);
    expect(icusdAmountToRawE8s(0.1)).toBe(10_000_000n);
    expect(icusdAmountToRawE8s(0.00000001)).toBe(1n);
    expect(icusdAmountToRawE8s(123.456789)).toBe(12_345_678_900n);
  });

  it('produces the same bigint whether computed once or recomputed (deterministic, no example-dependent proof needed)', () => {
    const amount = 999999.99999999;
    expect(icusdAmountToRawE8s(amount)).toBe(icusdAmountToRawE8s(amount));
    expect(icusdAmountToRawE8s(amount)).toBe(99_999_999_999_999n);
  });

  it('returns 0n for zero, negative, or non-finite amounts', () => {
    expect(icusdAmountToRawE8s(0)).toBe(0n);
    expect(icusdAmountToRawE8s(-5)).toBe(0n);
    expect(icusdAmountToRawE8s(NaN)).toBe(0n);
    expect(icusdAmountToRawE8s(Infinity)).toBe(0n);
  });
});

describe('beginPendingAction / nextPendingActionForOutcome (persist in-flight intent BEFORE mutation)', () => {
  const base = createInitialIntent(OWNER, 1000, 50, 1_700_000_000_000);

  it('persists the pending marker, snapshot ids, and submitted amounts before a mutating call', () => {
    const pending = beginPendingAction(base, 'open_and_borrow', 1_700_000_001_000, {
      preActionVaultIds: [1, 2],
      submittedCollateralKoinu: '100000000000',
      submittedIcusdAmount: 50,
    });
    expect(pending.pendingAction).toBe('open_and_borrow');
    expect(pending.preActionVaultIds).toEqual([1, 2]);
    expect(pending.submittedCollateralKoinu).toBe('100000000000');
    expect(pending.submittedIcusdAmount).toBe(50);
    expect(pending.updatedAt).toBe(1_700_000_001_000);
  });

  it('leaves fields not passed in updates unchanged', () => {
    const first = beginPendingAction(base, 'open_and_borrow', 1_700_000_001_000, {
      preActionVaultIds: [1, 2],
      submittedCollateralKoinu: '100000000000',
      submittedIcusdAmount: 50,
    });
    const second = beginPendingAction(first, 'finish_borrow', 1_700_000_002_000);
    expect(second.preActionVaultIds).toEqual([1, 2]);
    expect(second.submittedCollateralKoinu).toBe('100000000000');
    expect(second.pendingAction).toBe('finish_borrow');
  });

  it('maps outcome kinds to the correct next pendingAction, clearing it only on success/failed, and PRESERVING the current one on ambiguous_pending', () => {
    expect(nextPendingActionForOutcome('success', 'open_and_borrow')).toBeNull();
    expect(nextPendingActionForOutcome('failed', 'open_and_borrow')).toBeNull();
    expect(nextPendingActionForOutcome('partial_zero_debt', 'open_and_borrow')).toBe('finish_borrow');
    expect(nextPendingActionForOutcome('ambiguous_pending', 'open_and_borrow')).toBe('open_and_borrow');
    // The critical fix: an ambiguous recheck of a finish_borrow attempt must NOT relabel back to
    // open_and_borrow (which would imply a fresh open_vault_and_borrow is safe — it is not).
    expect(nextPendingActionForOutcome('ambiguous_pending', 'finish_borrow')).toBe('finish_borrow');
  });
});

describe('hasVaultAlreadyBorrowed (preflight before a partial-zero-debt retry — never double-borrow on a lost response)', () => {
  it('is false for a null vault (not yet reconciled)', () => {
    expect(hasVaultAlreadyBorrowed(null)).toBe(false);
  });

  it('is false for a vault with zero debt', () => {
    expect(hasVaultAlreadyBorrowed({ vaultId: 1, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 1n, borrowedIcusd: 0n })).toBe(false);
  });

  it('is true once the vault shows nonzero debt (a prior attempt already landed)', () => {
    expect(hasVaultAlreadyBorrowed({ vaultId: 1, collateralPrincipal: CKDOGE_PRINCIPAL, collateralAmount: 1n, borrowedIcusd: 1n })).toBe(true);
  });
});

describe('isStillLiveSession (account switch AND same-principal disconnect/reconnect guard)', () => {
  it('is live when principal and generation both match', () => {
    expect(
      isStillLiveSession({ principalText: OWNER, generation: 3 }, { principalText: OWNER, generation: 3 })
    ).toBe(true);
  });

  it('is not live when the principal changed (account switch)', () => {
    expect(
      isStillLiveSession({ principalText: OWNER, generation: 3 }, { principalText: OTHER_OWNER, generation: 4 })
    ).toBe(false);
  });

  it('is not live when the SAME principal reconnected (generation bumped) mid-await — a late callback from the old session must not treat itself as live', () => {
    expect(
      isStillLiveSession({ principalText: OWNER, generation: 3 }, { principalText: OWNER, generation: 4 })
    ).toBe(false);
  });

  it('is not live once disconnected to null, even before a new connect', () => {
    expect(
      isStillLiveSession({ principalText: OWNER, generation: 3 }, { principalText: null, generation: 4 })
    ).toBe(false);
  });
});

describe('computeDogeBorrowRisk (fee / CR / liquidation-price math)', () => {
  it('matches hand-computed values for a simple round-number scenario', () => {
    // 1000 DOGE @ $0.10 = $100 collateral, borrowing 50 icUSD.
    const risk = computeDogeBorrowRisk({
      collateralAmountDoge: 1000,
      icusdAmount: 50,
      collateralPriceUsd: 0.10,
      liquidationCr: 1.2,
      minimumCr: 1.35,
      borrowingFeeRate: 0.002,
    });
    expect(risk.collateralValueUsd).toBeCloseTo(100, 6);
    expect(risk.collateralRatioPct).toBeCloseTo(200, 6); // 100/50 * 100
    expect(risk.borrowFeeIcusd).toBeCloseTo(0.1, 6); // 50 * 0.002
    expect(risk.icusdReceived).toBeCloseTo(49.9, 6);
    // liquidationPrice = (icusdAmount * liquidationCr) / collateralAmount = (50*1.2)/1000 = 0.06
    expect(risk.liquidationPriceUsd).toBeCloseTo(0.06, 6);
    expect(risk.liqPriceRatio).toBeCloseTo(0.6, 6);
    expect(risk.safetyDeltaPct).toBeCloseTo(40, 6); // (0.10-0.06)/0.10*100
    expect(risk.isValidCr).toBe(true); // 200% >= 135%
  });

  it('flags an invalid (too-low) collateral ratio', () => {
    const risk = computeDogeBorrowRisk({
      collateralAmountDoge: 100,
      icusdAmount: 50,
      collateralPriceUsd: 0.10,
      liquidationCr: 1.2,
      minimumCr: 1.35,
      borrowingFeeRate: 0.002,
    });
    // 100*0.10=10 value / 50 icUSD = 20% CR, far below 135%
    expect(risk.isValidCr).toBe(false);
  });

  it('applies the fee curve multiplier when provided', () => {
    const risk = computeDogeBorrowRisk({
      collateralAmountDoge: 1000,
      icusdAmount: 50,
      collateralPriceUsd: 0.10,
      liquidationCr: 1.2,
      minimumCr: 1.35,
      borrowingFeeRate: 0.002,
      feeCurve: [[1, 2], [3, 1]], // higher fee near the floor CR
    });
    // projected CR (decimal) = 100/50 = 2 -> interpolated multiplier between (1,2) and (3,1) at cr=2 => 1.5
    expect(risk.borrowFeeIcusd).toBeCloseTo(50 * 0.002 * 1.5, 6);
  });

  it('treats zero icUSD amount as an infinite ratio, not a division error', () => {
    const risk = computeDogeBorrowRisk({
      collateralAmountDoge: 1000,
      icusdAmount: 0,
      collateralPriceUsd: 0.10,
      liquidationCr: 1.2,
      minimumCr: 1.35,
      borrowingFeeRate: 0.002,
    });
    expect(risk.collateralRatioPct).toBe(Infinity);
    expect(risk.liquidationPriceUsd).toBe(0);
  });
});

describe('computeMaxBorrow (0.5% haircut)', () => {
  it('applies the haircut so Max never overshoots the oracle price', () => {
    // 1000 DOGE @ $0.10 = $100, / 1.35 minCR * 0.995 haircut
    const max = computeMaxBorrow(1000, 0.10, 1.35);
    expect(max).toBeCloseTo((100 / 1.35) * 0.995, 2);
  });

  it('returns 0 for zero collateral or zero price', () => {
    expect(computeMaxBorrow(0, 0.10, 1.35)).toBe(0);
    expect(computeMaxBorrow(1000, 0, 1.35)).toBe(0);
  });
});

describe('projectRatioAtPriceDrop (downside slider)', () => {
  it('computes the implied CR at a given price-drop percentage', () => {
    const result = projectRatioAtPriceDrop(
      { collateralAmountDoge: 1000, icusdAmount: 50, collateralPriceUsd: 0.10 },
      50 // 50% drop
    );
    expect(result.impliedPriceUsd).toBeCloseTo(0.05, 6);
    expect(result.impliedCrPct).toBeCloseTo(100, 6); // (1000*0.05)/50*100
  });
});

describe('haveTermsChangedMaterially (price-refresh-before-confirm gate)', () => {
  it('flags a material liquidation-price change as changed', () => {
    const before = { collateralPriceUsd: 0.10, collateralRatioPct: 200, liquidationPriceUsd: 0.06, borrowFeeIcusd: 0.1 };
    const after = { collateralPriceUsd: 0.12, collateralRatioPct: 240, liquidationPriceUsd: 0.06, borrowFeeIcusd: 0.1 };
    expect(haveTermsChangedMaterially(before, after)).toBe(true);
  });

  it('does not flag a negligible sub-threshold change', () => {
    const before = { collateralPriceUsd: 0.10, collateralRatioPct: 200, liquidationPriceUsd: 0.06, borrowFeeIcusd: 0.1 };
    const after = { collateralPriceUsd: 0.1001, collateralRatioPct: 200.2, liquidationPriceUsd: 0.06, borrowFeeIcusd: 0.1 };
    expect(haveTermsChangedMaterially(before, after)).toBe(false);
  });
});

describe('canSubmitBorrow (submit lock + explicit final confirmation, no auto-borrow)', () => {
  const base = { actionInProgress: false, isConnected: true, principalMatchesIntent: true, termsConfirmed: true };

  it('allows submit when every gate is satisfied', () => {
    expect(canSubmitBorrow(base)).toBe(true);
  });

  it('blocks a second concurrent submit (submit lock)', () => {
    expect(canSubmitBorrow({ ...base, actionInProgress: true })).toBe(false);
  });

  it('blocks submit when disconnected', () => {
    expect(canSubmitBorrow({ ...base, isConnected: false })).toBe(false);
  });

  it('blocks submit when the connected principal no longer matches the intent (wallet switched mid-flow)', () => {
    expect(canSubmitBorrow({ ...base, principalMatchesIntent: false })).toBe(false);
  });

  it('blocks submit until the user has explicitly reconfirmed refreshed terms', () => {
    expect(canSubmitBorrow({ ...base, termsConfirmed: false })).toBe(false);
  });
});

describe('dogeBorrowActionLockName (per principal + network scope)', () => {
  it('produces distinct names for distinct principals and distinct network scopes', () => {
    expect(dogeBorrowActionLockName(OWNER, 'mainnet')).not.toBe(dogeBorrowActionLockName(OTHER_OWNER, 'mainnet'));
    expect(dogeBorrowActionLockName(OWNER, 'mainnet')).not.toBe(dogeBorrowActionLockName(OWNER, 'local'));
    expect(dogeBorrowActionLockName(OWNER, 'mainnet')).toContain(OWNER);
  });
});

describe('runExclusiveAction (real cross-tab mutual exclusion, conservative fail-closed when unsupported)', () => {
  function fakeLocks(available: boolean): ExclusiveLocksLike {
    return {
      request: vi.fn(async (_name, _options, callback) => {
        return callback(available ? {} : null);
      }),
    };
  }

  it('runs the action and returns its result when the lock is available', async () => {
    const locks = fakeLocks(true);
    const result = await runExclusiveAction(locks, 'lock-a', async () => 42);
    expect(result).toEqual({ ran: true, result: 42 });
  });

  it('requests with ifAvailable:true so a contending tab fails fast instead of queueing behind a released lock', async () => {
    const locks = fakeLocks(true);
    await runExclusiveAction(locks, 'lock-a', async () => 1);
    expect(locks.request).toHaveBeenCalledWith('lock-a', { ifAvailable: true }, expect.any(Function));
  });

  it('refuses to run the action when another tab already holds the lock', async () => {
    const locks = fakeLocks(false);
    const action = vi.fn(async () => 42);
    const result = await runExclusiveAction(locks, 'lock-a', action);
    expect(result).toEqual({ ran: false, reason: 'locked' });
    expect(action).not.toHaveBeenCalled();
  });

  it('fails closed (never runs unprotected) when the Locks API is unsupported', async () => {
    const action = vi.fn(async () => 42);
    const result = await runExclusiveAction(null, 'lock-a', action);
    expect(result).toEqual({ ran: false, reason: 'unsupported' });
    expect(action).not.toHaveBeenCalled();

    const result2 = await runExclusiveAction(undefined, 'lock-a', action);
    expect(result2).toEqual({ ran: false, reason: 'unsupported' });
    expect(action).not.toHaveBeenCalled();
  });

  it('two contenders: only the one that acquires the lock runs; the other is refused (simulated sequential Web Locks semantics)', async () => {
    let holder: 'A' | 'B' | null = null;
    const locks: ExclusiveLocksLike = {
      request: vi.fn(async (_name, _options, callback) => {
        if (holder !== null) return callback(null); // ifAvailable: true — busy, fail fast
        holder = 'A';
        try {
          return await callback({});
        } finally {
          holder = null;
        }
      }),
    };
    const resultA = runExclusiveAction(locks, 'lock-a', async () => {
      const resultB = await runExclusiveAction(locks, 'lock-a', async () => 'B-ran');
      expect(resultB).toEqual({ ran: false, reason: 'locked' });
      return 'A-ran';
    });
    expect(await resultA).toEqual({ ran: true, result: 'A-ran' });
  });
});
