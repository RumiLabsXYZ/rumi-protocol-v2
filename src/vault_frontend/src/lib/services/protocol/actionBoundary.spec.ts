import { describe, it, expect, vi, beforeEach } from 'vitest';
import { Principal } from '@dfinity/principal';
import { get } from 'svelte/store';

/**
 * Boundary tests for the action-bound (identity/session-pinned) mutating
 * methods added to ApiClient/walletOperations to close audit Q1
 * (/private/tmp/doge-borrow-action-audit.md): the shared call chain used to
 * re-read the live global wallet identity independently at every internal
 * await, so a mid-flow account switch (or same-account disconnect/reconnect)
 * could sign different sub-steps of one logical action under different
 * identities.
 *
 * These tests call the REAL ApiClient.openVaultAndBorrowBound /
 * ApiClient.borrowFromVaultBound / walletOperations.approveCollateralTransferBound
 * — only the actor/wallet-store/collateral-store/pnp/network boundaries are
 * mocked, never a copied helper that reimplements the logic under test.
 */

const mocks = vi.hoisted(() => ({
  getActor: vi.fn(),
  anonAllowance: vi.fn(),
  anonBalance: vi.fn(),
  ledgerFee: vi.fn(),
  getRedemptionPreview: vi.fn(),
  prepareRedemptionOffer: vi.fn(),
  getSignerAgent: vi.fn(),
  walletState: { isConnected: true, principal: null as any, loading: false },
  // Literal, not sourced from '../../config': vi.mock factories are hoisted
  // above this file's own top-level imports/consts, so a factory can never
  // reference a later `const` (temporal dead zone). Cross-checked against
  // CANISTER_IDS.CKDOGE_LEDGER in a sanity test below.
  ckdogeLedgerId: 'efmc5-wyaaa-aaaar-qb3wa-cai',
}));

vi.mock('@dfinity/agent', async () => {
  const actual = await vi.importActual<typeof import('@dfinity/agent')>('@dfinity/agent');
  return {
    ...actual,
    Actor: {
      ...actual.Actor,
      createActor: vi.fn(() => ({
        icrc2_allowance: mocks.anonAllowance,
        icrc1_balance_of: mocks.anonBalance,
        icrc1_fee: mocks.ledgerFee,
        get_redemption_preview: mocks.getRedemptionPreview,
        prepare_redemption_offer: mocks.prepareRedemptionOffer,
      })),
    },
    HttpAgent: vi.fn(() => ({ fetchRootKey: vi.fn().mockResolvedValue(undefined) })),
    AnonymousIdentity: vi.fn(() => ({})),
  };
});

vi.mock('../../stores/wallet', () => ({
  walletStore: {
    subscribe: (run: (v: any) => void) => {
      run(mocks.walletState);
      return () => {};
    },
    getActor: mocks.getActor,
  },
}));

vi.mock('$lib/stores/collateralStore', () => ({
  collateralStore: {
    getCollateralInfo: vi.fn(() => ({
      ledgerCanisterId: mocks.ckdogeLedgerId,
      symbol: 'ckDOGE',
      decimals: 8,
      ledgerFee: 10_000,
      minCollateralDeposit: 0,
    })),
  },
}));

vi.mock('../pnp', () => ({
  pnp: { getSignerAgent: mocks.getSignerAgent },
}));

// Avoids pulling in auth.ts / AuthClient / oisySigner transitively — nothing
// under test touches permissionManager.
vi.mock('../PermissionManager', () => ({ permissionManager: {} }));

import { CONFIG, CANISTER_IDS } from '../../config';
import { currentWalletType, walletSessionGeneration, WALLET_TYPES } from '../auth';
import {
  ApiClient,
  type BoundOpenVaultAndBorrowResult,
  type BoundBorrowFromVaultResult,
  parseIcusdAmountToE8s,
} from './apiClient';
import type { AcceptedRedemptionOffer } from '$lib/utils/redemptionPreview';
import { walletOperations, StaleActionSessionError, type ActionBoundContext } from './walletOperations';

const CKDOGE_LEDGER_ID = CANISTER_IDS.CKDOGE_LEDGER;
const BACKEND_ID = CONFIG.currentCanisterId;

if (CKDOGE_LEDGER_ID !== mocks.ckdogeLedgerId) {
  throw new Error(
    `Test fixture drift: mocks.ckdogeLedgerId ('${mocks.ckdogeLedgerId}') no longer matches CANISTER_IDS.CKDOGE_LEDGER ('${CKDOGE_LEDGER_ID}') — update the hoisted literal.`
  );
}

const PRINCIPAL_A = Principal.fromUint8Array(new Uint8Array([1, 1, 1, 1])).toText();
const PRINCIPAL_B = Principal.fromUint8Array(new Uint8Array([2, 2, 2, 2])).toText();

function setLivePrincipal(text: string | null) {
  mocks.walletState.principal = text ? Principal.fromText(text) : null;
  mocks.walletState.isConnected = !!text;
}

function acceptedOfferFor(request: {
  amount_e8s: bigint;
  expected_collateral_type: Principal;
  min_net_collateral_raw: bigint;
}, expiryNs = BigInt(Date.now() + 60_000) * 1_000_000n): AcceptedRedemptionOffer {
  return {
    amountE8s: request.amount_e8s,
    collateralTypeText: request.expected_collateral_type.toText(),
    minimumNetCollateralRaw: request.min_net_collateral_raw,
    validUntilNs: expiryNs,
    context: {
      principalText: PRINCIPAL_A,
      ledgerId: CONFIG.currentIcusdLedgerId,
      walletType: get(currentWalletType),
      sessionGeneration: get(walletSessionGeneration),
      networkKey: CONFIG.host,
    },
  };
}

/** A context that is current until `live` is flipped false — models both an
 * account switch (principal text mismatch, checked separately by
 * assertActionBoundContextCurrent) and a same-principal disconnect/reconnect
 * (generation bump — assertCurrent alone flips false, principal text unchanged). */
function makeCtx(expectedPrincipalText: string, assertCurrent: () => boolean = () => true): ActionBoundContext {
  return { expectedPrincipalText, assertCurrent };
}

let backendActor: {
  open_vault_and_borrow: ReturnType<typeof vi.fn>;
  borrow_from_vault: ReturnType<typeof vi.fn>;
  redeem_quoted: ReturnType<typeof vi.fn>;
  get_my_liquidity_withdrawal_status: ReturnType<typeof vi.fn>;
  withdraw_liquidity_with_id: ReturnType<typeof vi.fn>;
};
let ledgerActor: { icrc2_approve: ReturnType<typeof vi.fn> };

beforeEach(() => {
  vi.clearAllMocks();
  vi.spyOn(console, 'log').mockImplementation(() => {});
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  vi.spyOn(console, 'error').mockImplementation(() => {});
  localStorage.clear();
  walletSessionGeneration.set(0);
  currentWalletType.set(WALLET_TYPES.PLUG);

  backendActor = {
    open_vault_and_borrow: vi.fn().mockResolvedValue({ Ok: { vault_id: 7n, block_index: 99n } }),
    borrow_from_vault: vi.fn().mockResolvedValue({ Ok: { block_index: 55n, fee_amount_paid: 1_000n } }),
    redeem_quoted: vi.fn().mockResolvedValue({ Ok: {
      icusd_block_index: 88n,
      fee_paid_e8s: 1_000n,
      collateral_type: Principal.fromText(CKDOGE_LEDGER_ID),
      symbol: 'ckDOGE',
      decimals: 8,
      net_collateral_raw: 123_000n,
      payout_status: 'queued',
    } }),
    get_my_liquidity_withdrawal_status: vi.fn().mockResolvedValue([]),
    withdraw_liquidity_with_id: vi.fn().mockResolvedValue({ Ok: 101n }),
  };
  ledgerActor = {
    icrc2_approve: vi.fn().mockResolvedValue({ Ok: 1n }),
  };

  mocks.getActor.mockImplementation(async (canisterId: string) => {
    if (canisterId === BACKEND_ID) return backendActor;
    if (canisterId === CKDOGE_LEDGER_ID || canisterId === CONFIG.currentIcusdLedgerId) return ledgerActor;
    throw new Error(`unexpected getActor(${canisterId})`);
  });
  mocks.anonAllowance.mockResolvedValue({ allowance: 0n });
  mocks.anonBalance.mockResolvedValue(100_000_000_000n);
  mocks.ledgerFee.mockResolvedValue(100_000n);
  mocks.getRedemptionPreview.mockReset().mockResolvedValue({ queue: { entries: [] }, estimate: { Err: { RedemptionQuoteUnavailable: 'test' } } });
  mocks.prepareRedemptionOffer.mockReset().mockResolvedValue({ Err: { RefreshCooldown: { retry_after_ns: 1n } } });
  mocks.getSignerAgent.mockResolvedValue(null);

  setLivePrincipal(PRINCIPAL_A);
  mocks.walletState.loading = false;
});

describe('ApiClient.withdrawLiquidity — journal identity and session boundary', () => {
  const storageKey = `rumi:liquidity-withdrawal:${PRINCIPAL_A}`;
  const saved = () => ApiClient.getFromLocalStorage<{ requestId: bigint; amountE8s: bigint }>(storageKey);

  it('persists the same ID across a lost reply and retries it after reload', async () => {
    backendActor.withdraw_liquidity_with_id
      .mockRejectedValueOnce(new Error('reply lost'))
      .mockResolvedValueOnce({ Ok: 101n });
    const first = await ApiClient.withdrawLiquidity('0.29');
    expect(first.success).toBe(false);
    const persisted = saved();
    expect(persisted?.requestId).toBe(1n);
    expect(persisted?.amountE8s).toBe(29_000_000n);

    backendActor.get_my_liquidity_withdrawal_status.mockResolvedValueOnce([{
      request_id: 1n,
      amount_e8s: 29_000_000n,
      phase: { SubmittedOrUnknown: null },
    }]);
    const retry = await ApiClient.withdrawLiquidity('0.29');
    expect(retry.success).toBe(true);
    expect(backendActor.withdraw_liquidity_with_id).toHaveBeenNthCalledWith(2, 1n, 29_000_000n);
    expect(localStorage.getItem(storageKey)).toBeNull();
  });

  it('retains the request ID when the wallet session changes during dispatch', async () => {
    let resolveDispatch!: (value: any) => void;
    backendActor.withdraw_liquidity_with_id.mockImplementationOnce(() => new Promise(resolve => { resolveDispatch = resolve; }));
    const action = ApiClient.withdrawLiquidity('0.2');
    await vi.waitFor(() => expect(backendActor.withdraw_liquidity_with_id).toHaveBeenCalledTimes(1));
    walletSessionGeneration.set(1);
    resolveDispatch({ Ok: 101n });
    const result = await action;
    expect(result.success).toBe(false);
    expect(saved()).toEqual({ requestId: 1n, amountE8s: 20_000_000n });
  });

  it('reuses a completed owner request after a lost reply and provider switch', async () => {
    currentWalletType.set(WALLET_TYPES.OISY);
    backendActor.get_my_liquidity_withdrawal_status.mockResolvedValueOnce([{
      request_id: 10n,
      amount_e8s: 20_000_000n,
      phase: { Completed: { block_index: 101n } },
    }]);
    const result = await ApiClient.withdrawLiquidity('0.2');
    expect(result.success).toBe(true);
    expect(backendActor.withdraw_liquidity_with_id).toHaveBeenCalledWith(10n, 20_000_000n);
  });

  it('requires an explicit new intent after a completed withdrawal', async () => {
    backendActor.get_my_liquidity_withdrawal_status.mockResolvedValue([{
      request_id: 10n,
      amount_e8s: 20_000_000n,
      phase: { Completed: { block_index: 101n } },
    }]);
    const uncertain = await ApiClient.withdrawLiquidity('0.3');
    expect(uncertain.success).toBe(false);
    expect(backendActor.withdraw_liquidity_with_id).not.toHaveBeenCalled();
    const fresh = await ApiClient.withdrawLiquidity('0.3', true);
    expect(fresh.success).toBe(true);
    expect(backendActor.withdraw_liquidity_with_id).toHaveBeenCalledWith(11n, 30_000_000n);
  });

  it('keeps an intent through status lag, then advances after a durable no-effect tombstone', async () => {
    backendActor.withdraw_liquidity_with_id.mockResolvedValueOnce({
      Err: { AmountTooLow: { minimum_amount: 10_000_000n } },
    });
    // The first status read is served before the rejection tombstone is visible.
    backendActor.get_my_liquidity_withdrawal_status.mockResolvedValueOnce([]);
    const rejected = await ApiClient.withdrawLiquidity('0.01');
    expect(rejected.success).toBe(false);
    expect(saved()).toEqual({ requestId: 1n, amountE8s: 1_000_000n });

    const stillLagging = await ApiClient.withdrawLiquidity('0.2', true);
    expect(stillLagging.success).toBe(false);
    expect(backendActor.withdraw_liquidity_with_id).toHaveBeenCalledTimes(1);
    expect(saved()).toEqual({ requestId: 1n, amountE8s: 1_000_000n });

    // Once status proves request 1 was rejected without dispatch, the user
    // can change the amount and start request 2. It must not reuse request 1.
    backendActor.get_my_liquidity_withdrawal_status.mockResolvedValueOnce([{
      request_id: 1n,
      amount_e8s: 1_000_000n,
      phase: { RejectedNoEffect: null },
    }]);
    const accepted = await ApiClient.withdrawLiquidity('0.2', true);
    expect(accepted.success).toBe(true);
    expect(backendActor.withdraw_liquidity_with_id).toHaveBeenNthCalledWith(2, 2n, 20_000_000n);
    expect(localStorage.getItem(storageKey)).toBeNull();
  });

  it('cannot reuse a still-saved fresh intent to mint a second time after its reply is lost', async () => {
    ApiClient.saveToLocalStorage(storageKey, { requestId: 10n, amountE8s: 20_000_000n });
    backendActor.get_my_liquidity_withdrawal_status.mockResolvedValue([{
      request_id: 10n,
      amount_e8s: 20_000_000n,
      phase: { Completed: { block_index: 101n } },
    }]);
    const accidentalNew = await ApiClient.withdrawLiquidity('0.2', true);
    expect(accidentalNew.success).toBe(false);
    expect(backendActor.withdraw_liquidity_with_id).not.toHaveBeenCalled();
    const recovered = await ApiClient.withdrawLiquidity('0.2');
    expect(recovered.success).toBe(true);
    expect(backendActor.withdraw_liquidity_with_id).toHaveBeenCalledWith(10n, 20_000_000n);
    expect(localStorage.getItem(storageKey)).toBeNull();
  });

  it('fails before dispatch if the retry identity cannot be persisted', async () => {
    const save = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('storage unavailable'); });
    try {
      const result = await ApiClient.withdrawLiquidity('0.2');
      expect(result.success).toBe(false);
      expect(backendActor.withdraw_liquidity_with_id).not.toHaveBeenCalled();
    } finally {
      save.mockRestore();
    }
  });

  it('does not capture an old principal while wallet connection is loading', async () => {
    mocks.walletState.loading = true;
    const result = await ApiClient.withdrawLiquidity('0.2');
    expect(result.success).toBe(false);
    expect(mocks.getActor).not.toHaveBeenCalled();
  });

  it('recovers another tab’s unresolved ID from owner status without replacing it with a changed amount', async () => {
    backendActor.get_my_liquidity_withdrawal_status.mockResolvedValueOnce([{
      request_id: 9n,
      amount_e8s: 30_000_000n,
      phase: { SubmittedOrUnknown: null },
    }]);
    const result = await ApiClient.withdrawLiquidity('0.2');
    expect(result.success).toBe(false);
    expect(result.error).toContain('exact amount');
    expect(backendActor.withdraw_liquidity_with_id).not.toHaveBeenCalled();
    expect(localStorage.getItem(storageKey)).toBeNull();
  });

  it('aborts before dispatch after a session switch during actor acquisition and preserves the saved identity', async () => {
    ApiClient.saveToLocalStorage(storageKey, { requestId: 7n, amountE8s: 20_000_000n });
    let resolveActor!: (actor: any) => void;
    mocks.getActor.mockImplementationOnce(() => new Promise(resolve => { resolveActor = resolve; }));
    const action = ApiClient.withdrawLiquidity('0.2');
    await vi.waitFor(() => expect(mocks.getActor).toHaveBeenCalled());
    walletSessionGeneration.set(1);
    resolveActor(backendActor);
    const result = await action;
    expect(result.success).toBe(false);
    expect(backendActor.withdraw_liquidity_with_id).not.toHaveBeenCalled();
    expect(saved()).toEqual({ requestId: 7n, amountE8s: 20_000_000n });
  });
});

describe('exact icUSD withdrawal amount parsing', () => {
  it('preserves decimal text exactly when dispatching and journaling a keyed withdrawal', async () => {
    const result = await ApiClient.withdrawLiquidity('0.29');
    expect(result.success).toBe(true);
    expect(backendActor.withdraw_liquidity_with_id).toHaveBeenCalledWith(1n, 29_000_000n);
  });

  it('accepts at most eight decimal places and enforces the nat64 maximum', () => {
    expect(parseIcusdAmountToE8s('0.12345678')).toBe(12_345_678n);
    expect(parseIcusdAmountToE8s('184467440737.09551615')).toBe(18_446_744_073_709_551_615n);
    expect(() => parseIcusdAmountToE8s('0.123456789')).toThrow('up to 8 decimal places');
    expect(() => parseIcusdAmountToE8s('184467440737.09551616')).toThrow('maximum supported');
  });
});

it('closes legacy liquidity deposits before constructing an actor', async () => {
  const result = await ApiClient.provideLiquidity(1);
  expect(result.success).toBe(false);
  expect(result.error).toContain('closed');
  expect(mocks.getActor).not.toHaveBeenCalled();
});

describe('ApiClient.openVaultAndBorrowBound — standard ICRC-2 path (Internet Identity, Plug)', () => {
  const COLLATERAL_RAW = 123_456_789n; // exact koinu, deliberately not a round number
  const ICUSD_RAW = 250_000_000n; // exact e8s

  it('happy path: dispatches with the exact raw bigint amounts, no float round-trip', async () => {
    const ctx = makeCtx(PRINCIPAL_A);
    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(backendActor.open_vault_and_borrow).toHaveBeenCalledTimes(1);
    expect(backendActor.open_vault_and_borrow).toHaveBeenCalledWith(
      COLLATERAL_RAW,
      ICUSD_RAW,
      [Principal.fromText(CKDOGE_LEDGER_ID)]
    );
    expect(result).toEqual<BoundOpenVaultAndBorrowResult>({
      kind: 'dispatched_ok',
      vaultId: 7,
      blockIndex: 99,
      partialZeroDebtVaultId: null,
      errorMessage: null,
      approvalMayHaveMutated: true,
      submittedCollateralRaw: COLLATERAL_RAW,
      submittedIcusdRaw: ICUSD_RAW,
    });
  });

  it('a false assertCurrent() aborts before any actor/network call is made', async () => {
    const ctx = makeCtx(PRINCIPAL_A, () => false);
    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(result.kind).toBe('predispatch_aborted');
    expect((result as BoundOpenVaultAndBorrowResult).approvalMayHaveMutated).toBe(false);
    expect(mocks.getActor).not.toHaveBeenCalled();
    expect(mocks.anonAllowance).not.toHaveBeenCalled();
  });

  it('a throwing assertCurrent() propagates as the predispatch_aborted errorMessage', async () => {
    const ctx = makeCtx(PRINCIPAL_A, () => {
      throw new Error('boom - synchronous liveness check failed');
    });
    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(result.kind).toBe('predispatch_aborted');
    expect(result.errorMessage).toContain('boom - synchronous liveness check failed');
    expect(mocks.getActor).not.toHaveBeenCalled();
  });

  it('account already switched A→B before the click resolves: aborts immediately, no B approval, no B dispatch', async () => {
    setLivePrincipal(PRINCIPAL_B);
    const ctx = makeCtx(PRINCIPAL_A); // pinned to A, but live wallet is now B

    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(result.kind).toBe('predispatch_aborted');
    expect(mocks.getActor).not.toHaveBeenCalled();
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.open_vault_and_borrow).not.toHaveBeenCalled();
  });

  it('account switches A→B DURING the allowance-check await: approval is never dispatched under B', async () => {
    mocks.anonAllowance.mockImplementation(async () => {
      setLivePrincipal(PRINCIPAL_B); // simulate the switch completing mid-await
      return { allowance: 0n };
    });
    const ctx = makeCtx(PRINCIPAL_A);

    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(result.kind).toBe('predispatch_aborted');
    expect((result as BoundOpenVaultAndBorrowResult).approvalMayHaveMutated).toBe(false);
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.open_vault_and_borrow).not.toHaveBeenCalled();
  });

  it('account switches A→B DURING the approve await: approval under A may have landed, but the backend call is never dispatched under B', async () => {
    ledgerActor.icrc2_approve.mockImplementation(async () => {
      setLivePrincipal(PRINCIPAL_B);
      return { Ok: 1n };
    });
    const ctx = makeCtx(PRINCIPAL_A);

    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(result.kind).toBe('predispatch_aborted');
    expect((result as BoundOpenVaultAndBorrowResult).approvalMayHaveMutated).toBe(true);
    expect(ledgerActor.icrc2_approve).toHaveBeenCalledTimes(1);
    // getActor was called once for the ledger approval actor, never again for the backend.
    expect(mocks.getActor).toHaveBeenCalledTimes(1);
    expect(mocks.getActor).not.toHaveBeenCalledWith(BACKEND_ID, expect.anything());
    expect(backendActor.open_vault_and_borrow).not.toHaveBeenCalled();
  });

  it('same-principal disconnect/reconnect (generation bump, principal text unchanged) aborts even though the text still matches', async () => {
    let live = true;
    mocks.anonAllowance.mockImplementation(async () => {
      // Principal text is untouched — only the caller's own session-liveness
      // check (generation) flips, simulating a disconnect+reconnect of the
      // SAME account between the allowance read and the approval dispatch.
      live = false;
      return { allowance: 0n };
    });
    const ctx = makeCtx(PRINCIPAL_A, () => live);

    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(result.kind).toBe('predispatch_aborted');
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.open_vault_and_borrow).not.toHaveBeenCalled();
  });

  it('a thrown network error after dispatch is ambiguous_transport, never predispatch_aborted or a silent success', async () => {
    backendActor.open_vault_and_borrow.mockRejectedValue(new Error('deadline exceeded'));
    const ctx = makeCtx(PRINCIPAL_A);

    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(result.kind).toBe('ambiguous_transport');
    expect(result.vaultId).toBeNull();
    expect(result.errorMessage).toContain('deadline exceeded');
    expect((result as BoundOpenVaultAndBorrowResult).approvalMayHaveMutated).toBe(true);
  });

  it('the Oisy `_arr` false-negative pattern after dispatch is ALSO ambiguous_transport — no heuristic on-chain recovery in the bound path', async () => {
    backendActor.open_vault_and_borrow.mockRejectedValue(
      new Error("Cannot read properties of undefined (reading '_arr')")
    );
    const ctx = makeCtx(PRINCIPAL_A);

    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(result.kind).toBe('ambiguous_transport');
    // No extra actor was fetched to run a landed-heuristic scan.
    expect(mocks.getActor).toHaveBeenCalledTimes(2); // ledger approval + backend actor only
  });

  it('a typed Err whose GenericError text proves a zero-debt vault was created surfaces partialZeroDebtVaultId', async () => {
    backendActor.open_vault_and_borrow.mockResolvedValue({
      Err: { GenericError: 'Vault created (id=42) but the borrow step failed: mint_icusd rejected' },
    });
    const ctx = makeCtx(PRINCIPAL_A);

    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(result.kind).toBe('dispatched_err');
    expect((result as BoundOpenVaultAndBorrowResult).partialZeroDebtVaultId).toBe(42);
    expect(result.errorMessage).toContain('Vault created (id=42)');
  });

  it('a typed Err with no partial-vault text leaves partialZeroDebtVaultId null', async () => {
    backendActor.open_vault_and_borrow.mockResolvedValue({ Err: { CallerNotOwner: null } });
    const ctx = makeCtx(PRINCIPAL_A);

    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(result.kind).toBe('dispatched_err');
    expect((result as BoundOpenVaultAndBorrowResult).partialZeroDebtVaultId).toBeNull();
  });

  it('skips the approval dispatch entirely when the existing allowance already covers the amount', async () => {
    mocks.anonAllowance.mockResolvedValue({ allowance: 999_999_999_999n });
    const ctx = makeCtx(PRINCIPAL_A);

    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect((result as BoundOpenVaultAndBorrowResult).approvalMayHaveMutated).toBe(false);
    expect(result.kind).toBe('dispatched_ok');
  });
});

describe('ApiClient.redeemQuoted — bounded ICRC-2 allowance and submission outcome', () => {
  const request = {
    amount_e8s: 12_345_678n,
    expected_collateral_type: Principal.fromText(CKDOGE_LEDGER_ID),
    min_net_collateral_raw: 100_000n,
  };

  it('loads cached advisory preview through the generated public query without ledger or wallet actions', async () => {
    const preview = { queue: { entries: [] }, estimate: { Err: { RedemptionQuoteUnavailable: 'stale price' } } };
    mocks.getRedemptionPreview.mockResolvedValue(preview);

    const result = await ApiClient.getRedemptionPreview(request.amount_e8s);

    expect(result).toEqual(preview);
    expect(mocks.getRedemptionPreview).toHaveBeenCalledWith(request.amount_e8s);
    expect(mocks.prepareRedemptionOffer).not.toHaveBeenCalled();
    expect(mocks.anonAllowance).not.toHaveBeenCalled();
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.redeem_quoted).not.toHaveBeenCalled();
  });

  it('prepares a live offer through the anonymous update without reading allowance or submitting', async () => {
    const response = { Ok: { queue: { entries: [] }, quote: { Err: { RedemptionCapacityExceeded: { max_input_icusd_e8s: 1n } } } } };
    mocks.prepareRedemptionOffer.mockResolvedValue(response);

    const result = await ApiClient.prepareRedemptionOffer(request.amount_e8s);

    expect(result).toEqual(response);
    expect(mocks.prepareRedemptionOffer).toHaveBeenCalledWith(request.amount_e8s);
    expect(mocks.getRedemptionPreview).not.toHaveBeenCalled();
    expect(mocks.anonAllowance).not.toHaveBeenCalled();
    expect(mocks.anonBalance).not.toHaveBeenCalled();
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.redeem_quoted).not.toHaveBeenCalled();
  });

  it('fails before any ledger read, approval, or submit when no accepted offer is provided', async () => {
    const result = await ApiClient.redeemQuoted(request, undefined, null as unknown as AcceptedRedemptionOffer);

    expect(result.success).toBe(false);
    expect(result.error).toContain('Accept a fresh live offer');
    expect(mocks.anonAllowance).not.toHaveBeenCalled();
    expect(mocks.anonBalance).not.toHaveBeenCalled();
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.redeem_quoted).not.toHaveBeenCalled();
  });

  it('fails before any ledger read when accepted amount, collateral, or minimum differs from the request', async () => {
    const accepted = acceptedOfferFor(request);
    const result = await ApiClient.redeemQuoted(request, undefined, {
      ...accepted,
      minimumNetCollateralRaw: request.min_net_collateral_raw + 1n,
    });

    expect(result.success).toBe(false);
    expect(mocks.anonAllowance).not.toHaveBeenCalled();
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.redeem_quoted).not.toHaveBeenCalled();
  });

  it('approves the exact requested icUSD amount before submission when allowance is absent', async () => {
    mocks.anonAllowance.mockResolvedValue({ allowance: 0n });

    const result = await ApiClient.redeemQuoted(request, undefined, acceptedOfferFor(request));

    expect(ledgerActor.icrc2_approve).toHaveBeenCalledOnce();
    expect(ledgerActor.icrc2_approve).toHaveBeenCalledWith(expect.objectContaining({
      amount: request.amount_e8s + 100_000n,
      spender: { owner: Principal.fromText(BACKEND_ID), subaccount: [] },
    }));
    expect(typeof ledgerActor.icrc2_approve.mock.calls[0][0].expires_at[0]).toBe('bigint');
    expect(backendActor.redeem_quoted).toHaveBeenCalledOnce();
    expect(ledgerActor.icrc2_approve.mock.invocationCallOrder[0])
      .toBeLessThan(backendActor.redeem_quoted.mock.invocationCallOrder[0]);
    expect(result).toMatchObject({ success: true, blockIndex: 88, redemption: { payoutStatus: 'queued' } });
  });

  it('skips approval when the existing allowance covers the quoted amount', async () => {
    mocks.anonAllowance.mockResolvedValue({ allowance: request.amount_e8s + 100_000n });

    const result = await ApiClient.redeemQuoted(request, undefined, acceptedOfferFor(request));

    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.redeem_quoted).toHaveBeenCalledOnce();
    expect(result.success).toBe(true);
  });

  it('does not dispatch redemption when the wallet switches during approval', async () => {
    mocks.anonAllowance.mockResolvedValue({ allowance: 0n });
    ledgerActor.icrc2_approve.mockImplementation(async () => {
      setLivePrincipal(PRINCIPAL_B);
      return { Ok: 3n };
    });

    const result = await ApiClient.redeemQuoted(request, undefined, acceptedOfferFor(request));

    expect(ledgerActor.icrc2_approve).toHaveBeenCalledOnce();
    expect(backendActor.redeem_quoted).not.toHaveBeenCalled();
    expect(result.success).toBe(false);
    expect(result.ambiguous).toBeUndefined();
  });

  it('does not dispatch redemption after a same-principal wallet session transition during approval', async () => {
    mocks.anonAllowance.mockResolvedValue({ allowance: 0n });
    ledgerActor.icrc2_approve.mockImplementation(async () => {
      walletSessionGeneration.update((generation) => generation + 1);
      return { Ok: 3n };
    });

    const result = await ApiClient.redeemQuoted(request, undefined, acceptedOfferFor(request));

    expect(backendActor.redeem_quoted).not.toHaveBeenCalled();
    expect(result.success).toBe(false);
    expect(result.ambiguous).toBeUndefined();
  });

  it('does not dispatch after approval finishes if the accepted offer expired during the approval await', async () => {
    mocks.anonAllowance.mockResolvedValue({ allowance: 0n });
    ledgerActor.icrc2_approve.mockImplementation(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
      return { Ok: 3n };
    });
    const accepted = acceptedOfferFor(request, BigInt(Date.now() + 5) * 1_000_000n);

    const result = await ApiClient.redeemQuoted(request, undefined, accepted);

    expect(ledgerActor.icrc2_approve).toHaveBeenCalledOnce();
    expect(backendActor.redeem_quoted).not.toHaveBeenCalled();
    expect(result.success).toBe(false);
    expect(result.error).toContain('offer expired');
  });

  it('preserves the approval fee when checking a first-use maximum balance', async () => {
    mocks.anonAllowance.mockResolvedValue({ allowance: 0n });
    mocks.anonBalance.mockResolvedValue(request.amount_e8s + 100_000n);

    const result = await ApiClient.redeemQuoted(request, undefined, acceptedOfferFor(request));

    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.redeem_quoted).not.toHaveBeenCalled();
    expect(result.success).toBe(false);
    expect(result.error).toContain(`${request.amount_e8s + 200_000n} raw units`);
  });

  it('reports a lost approval reply at the approval stage and never submits redemption', async () => {
    mocks.anonAllowance.mockResolvedValue({ allowance: 0n });
    ledgerActor.icrc2_approve.mockRejectedValue(new Error('approval reply timed out'));

    const result = await ApiClient.redeemQuoted(request, undefined, acceptedOfferFor(request));

    expect(backendActor.redeem_quoted).not.toHaveBeenCalled();
    expect(result).toMatchObject({ success: false, ambiguous: true, ambiguityStage: 'approval' });
  });

  it('fails closed on missing or stale Oisy preflight before acquiring a signer', async () => {
    localStorage.setItem('rumi_last_wallet', 'oisy');
    currentWalletType.set(WALLET_TYPES.OISY);

    const result = await ApiClient.redeemQuoted(request, undefined, acceptedOfferFor(request));

    expect(mocks.getSignerAgent).not.toHaveBeenCalled();
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.redeem_quoted).not.toHaveBeenCalled();
    expect(result.success).toBe(false);
    expect(result.error).toContain('check is missing or stale');
  });

  it('fails closed when an Oisy preflight snapshot contains invalid fee or balance data', async () => {
    localStorage.setItem('rumi_last_wallet', 'oisy');
    currentWalletType.set(WALLET_TYPES.OISY);
    const preparedPreflight = {
      principalText: PRINCIPAL_A,
      walletType: WALLET_TYPES.OISY,
      sessionGeneration: 0,
      ledgerId: CONFIG.currentIcusdLedgerId,
      observedAtMs: Date.now(),
      allowanceRaw: 0n,
      balanceRaw: request.amount_e8s + 200_000n,
      feeRaw: -1n,
    };

    const result = await ApiClient.redeemQuoted(request, preparedPreflight, acceptedOfferFor(request));

    expect(mocks.getSignerAgent).not.toHaveBeenCalled();
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.redeem_quoted).not.toHaveBeenCalled();
    expect(result.success).toBe(false);
  });

  it('uses the Oisy preflight when wallet storage is absent but the active wallet type is Oisy', async () => {
    currentWalletType.set(WALLET_TYPES.OISY);
    mocks.getSignerAgent.mockResolvedValue({});
    const preparedPreflight = {
      principalText: PRINCIPAL_A,
      walletType: WALLET_TYPES.OISY,
      sessionGeneration: 0,
      ledgerId: CONFIG.currentIcusdLedgerId,
      observedAtMs: Date.now(),
      allowanceRaw: request.amount_e8s + 100_000n,
      balanceRaw: request.amount_e8s + 100_000n,
      feeRaw: 100_000n,
    };

    const result = await ApiClient.redeemQuoted(request, preparedPreflight, acceptedOfferFor(request));

    expect(mocks.getSignerAgent).toHaveBeenCalledOnce();
    expect(mocks.anonAllowance).not.toHaveBeenCalled();
    expect(mocks.anonBalance).not.toHaveBeenCalled();
    expect(mocks.ledgerFee).not.toHaveBeenCalled();
    expect(backendActor.redeem_quoted).toHaveBeenCalledOnce();
    expect(result.success).toBe(true);
  });

  it('does not use a balance change to upgrade a lost Oisy submission reply', async () => {
    localStorage.setItem('rumi_last_wallet', 'oisy');
    currentWalletType.set(WALLET_TYPES.OISY);
    mocks.getSignerAgent.mockResolvedValue({});
    ledgerActor.icrc2_approve.mockResolvedValue({ Ok: 3n });
    backendActor.redeem_quoted.mockRejectedValue(new Error("Cannot read properties of undefined (reading '_arr')"));
    const preparedPreflight = {
      principalText: PRINCIPAL_A,
      walletType: WALLET_TYPES.OISY,
      sessionGeneration: 0,
      ledgerId: CONFIG.currentIcusdLedgerId,
      observedAtMs: Date.now(),
      allowanceRaw: 0n,
      balanceRaw: request.amount_e8s + 200_000n,
      feeRaw: 100_000n,
    };

    const result = await ApiClient.redeemQuoted(request, preparedPreflight, acceptedOfferFor(request));

    expect(backendActor.redeem_quoted).toHaveBeenCalledOnce();
    expect(mocks.anonBalance).not.toHaveBeenCalled();
    expect(result).toMatchObject({
      success: false,
      ambiguous: true,
      ambiguityStage: 'submission',
    });
    expect(result.redemption).toBeUndefined();
  });

  it('rechecks session after submit and labels a typed queued result from the previous session', async () => {
    backendActor.redeem_quoted.mockImplementation(async () => {
      walletSessionGeneration.update((generation) => generation + 1);
      return {
        Ok: {
          icusd_block_index: 88n,
          fee_paid_e8s: 1_000n,
          collateral_type: Principal.fromText(CKDOGE_LEDGER_ID),
          symbol: 'ckDOGE',
          decimals: 8,
          net_collateral_raw: 123_000n,
          payout_status: 'queued',
        },
      };
    });

    const result = await ApiClient.redeemQuoted(request, undefined, acceptedOfferFor(request));

    expect(result).toMatchObject({
      success: true,
      sessionChangedAfterSubmission: true,
      message: expect.stringContaining('previous wallet session'),
      redemption: { payoutStatus: 'queued' },
    });
  });
});

describe('ApiClient.openVaultAndBorrowBound — Oisy ICRC-112 batched path', () => {
  const COLLATERAL_RAW = 50_000_000n;
  const ICUSD_RAW = 10_000_000n;

  beforeEach(() => {
    localStorage.setItem('rumi_last_wallet', 'oisy');
    mocks.getSignerAgent.mockResolvedValue({ id: 'fake-signer' });
  });

  it('happy path: approve then open_vault_and_borrow, exact raw amounts, both under principal A', async () => {
    const ctx = makeCtx(PRINCIPAL_A);
    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(ledgerActor.icrc2_approve).toHaveBeenCalledTimes(1);
    expect(backendActor.open_vault_and_borrow).toHaveBeenCalledWith(
      COLLATERAL_RAW,
      ICUSD_RAW,
      [Principal.fromText(CKDOGE_LEDGER_ID)]
    );
    expect(result.kind).toBe('dispatched_ok');
  });

  it('account switches A→B between the Oisy approve and the backend dispatch: backend call never fires under B', async () => {
    ledgerActor.icrc2_approve.mockImplementation(async () => {
      setLivePrincipal(PRINCIPAL_B);
      return { Ok: 1n };
    });
    const ctx = makeCtx(PRINCIPAL_A);

    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(result.kind).toBe('predispatch_aborted');
    expect((result as BoundOpenVaultAndBorrowResult).approvalMayHaveMutated).toBe(true);
    expect(backendActor.open_vault_and_borrow).not.toHaveBeenCalled();
  });

  it('an Oisy approve Err response aborts predispatch without touching the backend actor', async () => {
    ledgerActor.icrc2_approve.mockResolvedValue({ Err: { GenericError: 'InsufficientFunds' } });
    const ctx = makeCtx(PRINCIPAL_A);

    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(result.kind).toBe('predispatch_aborted');
    expect((result as BoundOpenVaultAndBorrowResult).approvalMayHaveMutated).toBe(true);
    expect(backendActor.open_vault_and_borrow).not.toHaveBeenCalled();
  });
});

describe('ApiClient.borrowFromVaultBound — finish-borrow leg', () => {
  const VAULT_ID = 5;
  const ICUSD_RAW = 200_000_000n;

  it('happy path: dispatches the exact raw e8s amount and surfaces the exact fee paid', async () => {
    const ctx = makeCtx(PRINCIPAL_A);
    const result = await ApiClient.borrowFromVaultBound(ctx, VAULT_ID, ICUSD_RAW);

    expect(backendActor.borrow_from_vault).toHaveBeenCalledWith({ vault_id: BigInt(VAULT_ID), amount: ICUSD_RAW });
    expect(result).toEqual<BoundBorrowFromVaultResult>({
      kind: 'dispatched_ok',
      vaultId: VAULT_ID,
      blockIndex: 55,
      feePaidRaw: 1_000n,
      errorMessage: null,
      submittedIcusdRaw: ICUSD_RAW,
    });
  });

  it('rejects a non-positive amount predispatch', async () => {
    const ctx = makeCtx(PRINCIPAL_A);
    const result = await ApiClient.borrowFromVaultBound(ctx, VAULT_ID, 0n);

    expect(result.kind).toBe('predispatch_aborted');
    expect(mocks.getActor).not.toHaveBeenCalled();
  });

  it('journal retry preserves exact nat64 vault ID and raw amount below current client minimum', async () => {
    const ctx = makeCtx(PRINCIPAL_A);
    const vaultId = 9_007_199_254_740_993n;
    const originalRawAmount = 5_000_001n;
    const largeMintBlock = 9_007_199_254_740_995n;

    const normalBorrow = await ApiClient.borrowFromVaultBound(ctx, vaultId, originalRawAmount);
    expect(normalBorrow.kind).toBe('predispatch_aborted');
    expect(backendActor.borrow_from_vault).not.toHaveBeenCalled();

    backendActor.borrow_from_vault.mockResolvedValueOnce({ Ok: { block_index: largeMintBlock, fee_amount_paid: 1_000n } });
    const retry = await ApiClient.retryPendingBorrowMintBound(ctx, vaultId, originalRawAmount);
    expect(backendActor.borrow_from_vault).toHaveBeenCalledWith({
      vault_id: vaultId,
      amount: originalRawAmount,
    });
    expect(retry.kind).toBe('dispatched_ok');
    expect(retry.vaultId).toBe(vaultId);
    expect(retry.blockIndex).toBe(largeMintBlock);
  });

  it('a false assertCurrent() aborts before the actor is ever fetched', async () => {
    const ctx = makeCtx(PRINCIPAL_A, () => false);
    const result = await ApiClient.borrowFromVaultBound(ctx, VAULT_ID, ICUSD_RAW);

    expect(result.kind).toBe('predispatch_aborted');
    expect(mocks.getActor).not.toHaveBeenCalled();
    expect(backendActor.borrow_from_vault).not.toHaveBeenCalled();
  });

  it('account switches A→B DURING the actor-fetch await: the borrow call never dispatches', async () => {
    mocks.getActor.mockImplementation(async (canisterId: string) => {
      if (canisterId === BACKEND_ID) {
        setLivePrincipal(PRINCIPAL_B);
        return backendActor;
      }
      throw new Error(`unexpected getActor(${canisterId})`);
    });
    const ctx = makeCtx(PRINCIPAL_A);

    const result = await ApiClient.borrowFromVaultBound(ctx, VAULT_ID, ICUSD_RAW);

    expect(result.kind).toBe('predispatch_aborted');
    expect(backendActor.borrow_from_vault).not.toHaveBeenCalled();
  });

  it('a lost reply (thrown error after dispatch) is ambiguous_transport, not a definitive failure', async () => {
    backendActor.borrow_from_vault.mockRejectedValue(new Error('connection reset'));
    const ctx = makeCtx(PRINCIPAL_A);

    const result = await ApiClient.borrowFromVaultBound(ctx, VAULT_ID, ICUSD_RAW);

    expect(result.kind).toBe('ambiguous_transport');
    expect(result.errorMessage).toContain('connection reset');
    // vaultId is retained (it was already known — this is the finish-borrow leg on a known vault),
    // but no debt/fee figures are asserted since none are proven.
    expect((result as BoundBorrowFromVaultResult).vaultId).toBe(VAULT_ID);
    expect((result as BoundBorrowFromVaultResult).feePaidRaw).toBeNull();
  });

  it('a typed Err response is dispatched_err with the formatted backend message', async () => {
    backendActor.borrow_from_vault.mockResolvedValue({ Err: { CallerNotOwner: null } });
    const ctx = makeCtx(PRINCIPAL_A);

    const result = await ApiClient.borrowFromVaultBound(ctx, VAULT_ID, ICUSD_RAW);

    expect(result.kind).toBe('dispatched_err');
    expect(result.errorMessage).toContain('permission');
  });
});

describe('walletOperations.approveCollateralTransferBound', () => {
  it('a stale ctx throws StaleActionSessionError before any actor is fetched', async () => {
    const ctx = makeCtx(PRINCIPAL_A, () => false);

    await expect(
      walletOperations.approveCollateralTransferBound(ctx, 1_000_000n, BACKEND_ID, CKDOGE_LEDGER_ID)
    ).rejects.toBeInstanceOf(StaleActionSessionError);
    expect(mocks.getActor).not.toHaveBeenCalled();
  });

  it('an account switch A→B between actor acquisition and dispatch throws before icrc2_approve is called', async () => {
    mocks.getActor.mockImplementation(async (canisterId: string) => {
      if (canisterId === CKDOGE_LEDGER_ID) {
        setLivePrincipal(PRINCIPAL_B);
        return ledgerActor;
      }
      throw new Error(`unexpected getActor(${canisterId})`);
    });
    const ctx = makeCtx(PRINCIPAL_A);

    await expect(
      walletOperations.approveCollateralTransferBound(ctx, 1_000_000n, BACKEND_ID, CKDOGE_LEDGER_ID)
    ).rejects.toBeInstanceOf(StaleActionSessionError);
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
  });

  it('happy path dispatches icrc2_approve with the exact raw amount', async () => {
    const ctx = makeCtx(PRINCIPAL_A);
    const result = await walletOperations.approveCollateralTransferBound(ctx, 1_234_567n, BACKEND_ID, CKDOGE_LEDGER_ID);

    expect(ledgerActor.icrc2_approve).toHaveBeenCalledTimes(1);
    expect(ledgerActor.icrc2_approve.mock.calls[0][0].amount).toBe(1_234_567n);
    expect(result).toEqual({ success: true });
  });
});
