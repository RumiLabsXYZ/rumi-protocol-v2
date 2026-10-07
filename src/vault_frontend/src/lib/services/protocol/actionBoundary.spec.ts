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
  walletState: { isConnected: true, principal: null as any },
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
    HttpAgent: vi.fn(class MockHttpAgent {
      fetchRootKey = vi.fn().mockResolvedValue(undefined);
    }),
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
} from './apiClient';
import type { AcceptedRedemptionOffer } from '$lib/utils/redemptionPreview';
import { walletOperations, StaleActionSessionError, type ActionBoundContext } from './walletOperations';
import { _clearLedgerFeeCache, getFreshCachedLedgerFee } from '../ledgerFeeService';

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
  Object.defineProperty(window, 'ic', {
    configurable: true,
    value: text ? { plug: { principalId: text } } : {},
  });
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
  open_vault_v2: ReturnType<typeof vi.fn>;
  add_margin_v2: ReturnType<typeof vi.fn>;
  get_my_collateral_ingress_state: ReturnType<typeof vi.fn>;
  get_my_collateral_ingress: ReturnType<typeof vi.fn>;
  borrow_from_vault: ReturnType<typeof vi.fn>;
  redeem_quoted: ReturnType<typeof vi.fn>;
  get_my_repayment_v2_status: ReturnType<typeof vi.fn>;
  get_my_repayment_v2_request_state: ReturnType<typeof vi.fn>;
  repay_to_vault_v2: ReturnType<typeof vi.fn>;
};
let ledgerActor: { icrc2_approve: ReturnType<typeof vi.fn> };

beforeEach(() => {
  vi.clearAllMocks();
  _clearLedgerFeeCache();
  vi.spyOn(console, 'log').mockImplementation(() => {});
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  vi.spyOn(console, 'error').mockImplementation(() => {});
  localStorage.clear();
  walletSessionGeneration.set(0);
  currentWalletType.set(WALLET_TYPES.PLUG);

  backendActor = {
    open_vault_and_borrow: vi.fn().mockResolvedValue({ Ok: { vault_id: 7n, block_index: 99n } }),
    open_vault_v2: vi.fn().mockResolvedValue({ Err: { CallerNotOwner: null } }),
    add_margin_v2: vi.fn().mockResolvedValue({ Err: { CallerNotOwner: null } }),
    get_my_collateral_ingress_state: vi.fn().mockResolvedValue({ Ok: {
      next_request_id: 7n, active_request: [], latest_result: [],
    } }),
    get_my_collateral_ingress: vi.fn().mockResolvedValue([]),
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
    get_my_repayment_v2_status: vi.fn().mockResolvedValue({ Ok: [] }),
    get_my_repayment_v2_request_state: vi.fn().mockResolvedValue({ Ok: {
      next_request_id: 7n, active_request: [], latest_result: [],
    } }),
    repay_to_vault_v2: vi.fn().mockImplementation(async (requestId: bigint, arg: { vault_id: bigint; amount: bigint }) => ({ Ok: {
      request_id: requestId,
      last_error: [],
      result: [],
      tuple: [],
      had_ambiguous_attempt: false,
      effective_amount_raw: arg.amount,
      owner: Principal.fromText(PRINCIPAL_A),
      vault_id: arg.vault_id,
      ledger: Principal.fromText(CONFIG.currentIcusdLedgerId),
      candidate_block_index: [],
      close_after_repay: false,
      phase: { PendingPull: null },
      requested_amount_raw: arg.amount,
    } })),
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
});

describe('ApiClient.repayV2Bound approval amount', () => {
  const amountRaw = 25_000_000n;
  const requiredAllowance = amountRaw + 2n * 100_000n;
  const intent = () => ({
    version: 1 as const,
    owner: PRINCIPAL_A,
    network: `${CONFIG.host}|${CONFIG.currentCanisterId}`,
    requestId: '7',
    vaultId: '12',
    requestedAmountRaw: amountRaw.toString(),
    closeAfterRepay: false,
    approvalAttempted: false,
    backendDispatchAttempted: false,
  });

  it.each([
    { wallet: 'Plug', isOisy: false },
    { wallet: 'Oisy', isOisy: true },
  ])('approves only amount plus two ledger fees for $wallet', async ({ isOisy }) => {
    mocks.anonAllowance.mockResolvedValue({ allowance: 0n });
    if (isOisy) {
      currentWalletType.set(WALLET_TYPES.OISY);
      localStorage.setItem('rumi_last_wallet', 'oisy');
      mocks.getSignerAgent.mockResolvedValue({ id: 'fake-signer' });
    }

    await ApiClient.repayV2Bound(
      makeCtx(PRINCIPAL_A), intent(), vi.fn(), () => false, vi.fn(),
    );

    expect(ledgerActor.icrc2_approve).toHaveBeenCalledOnce();
    expect(ledgerActor.icrc2_approve).toHaveBeenCalledWith(expect.objectContaining({ amount: requiredAllowance }));
    expect(ledgerActor.icrc2_approve.mock.calls[0][0].amount).toBeLessThan(100_000_000_000_000_000n);
    expect(backendActor.repay_to_vault_v2).toHaveBeenCalledOnce();
  });
});

describe('ApiClient.openVaultV2Bound — request journal before approval', () => {
  const ledger = CKDOGE_LEDGER_ID;
  const amount = 123_456_789n;
  const requestId = 7n;
  const openStatus = (phase: object, result: object[] = []) => ({
    request_id: requestId,
    owner: Principal.fromText(PRINCIPAL_A),
    ledger: Principal.fromText(ledger),
    operation: { Open: { collateral_type: Principal.fromText(ledger) } },
    phase,
    amount_raw: amount,
    fee_raw: 10_000n,
    memo: [],
    created_at_time_ns: 1n,
    candidate_block_index: [],
    result,
    had_ambiguous_attempt: false,
    last_error: [],
  });

  it('replays an exact completed open without requesting another approval', async () => {
    const completed = openStatus({ Complete: null }, [{ Open: { vault_id: 42n, block_index: 88n } }]);
    backendActor.get_my_collateral_ingress_state.mockResolvedValue({ Ok: {
      next_request_id: 8n, active_request: [], latest_result: [completed],
    } });
    const result = await ApiClient.openVaultV2Bound(makeCtx(PRINCIPAL_A), requestId, amount, ledger);

    expect(result).toMatchObject({ kind: 'dispatched_ok', status: completed });
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.open_vault_v2).not.toHaveBeenCalled();
  });

  it('replays an exact Pending/Held request without approving again', async () => {
    const pending = openStatus({ Held: null });
    backendActor.get_my_collateral_ingress_state.mockResolvedValue({ Ok: {
      next_request_id: 8n, active_request: [pending], latest_result: [],
    } });
    backendActor.open_vault_v2.mockResolvedValue({ Ok: pending });

    const result = await ApiClient.openVaultV2Bound(makeCtx(PRINCIPAL_A), requestId, amount, ledger);

    expect(result.kind).toBe('dispatched_ok');
    expect(backendActor.open_vault_v2).toHaveBeenCalledOnce();
    expect(backendActor.open_vault_v2).toHaveBeenCalledWith(requestId, amount, [Principal.fromText(ledger)]);
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
  });

  it('returns a terminal Rejected request as an error before approval or replay', async () => {
    const rejected = openStatus({ Rejected: null }, [{ Rejected: { message: 'source paused' } }]);
    backendActor.get_my_collateral_ingress_state.mockResolvedValue({ Ok: {
      next_request_id: 8n, active_request: [], latest_result: [rejected],
    } });

    const result = await ApiClient.openVaultV2Bound(makeCtx(PRINCIPAL_A), requestId, amount, ledger);

    expect(result).toMatchObject({ kind: 'predispatch_aborted', status: rejected, errorMessage: 'source paused' });
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.open_vault_v2).not.toHaveBeenCalled();
  });
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

  it('returns a typed collateral BadFee once without upgrading it to a retry-safe result', async () => {
    backendActor.open_vault_and_borrow.mockResolvedValue({
      Err: { TransferFromError: [{ BadFee: { expected_fee: 10_000n } }, false] },
    });
    const ctx = makeCtx(PRINCIPAL_A);

    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(result.kind).toBe('dispatched_err');
    expect(result.errorMessage).toContain('Unexpected fee');
    expect(backendActor.open_vault_and_borrow).toHaveBeenCalledTimes(1);
  });

  it('does not label a generic collateral transfer_from error as proven no-effect', async () => {
    backendActor.open_vault_and_borrow.mockResolvedValue({
      Err: { TransferFromError: [{ GenericError: { message: 'ledger call outcome unknown', error_code: 1n } }, false] },
    });
    const ctx = makeCtx(PRINCIPAL_A);

    const result = await ApiClient.openVaultAndBorrowBound(ctx, COLLATERAL_RAW, ICUSD_RAW, CKDOGE_LEDGER_ID);

    expect(result.kind).toBe('dispatched_err');
    expect(result.errorMessage).toContain('Transfer error:');
    expect(backendActor.open_vault_and_borrow).toHaveBeenCalledTimes(1);
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

describe('redemption submission is paused before wallet approval', () => {
  const request = {
    amount_e8s: 12_345_678n,
    expected_collateral_type: Principal.fromText(CKDOGE_LEDGER_ID),
    min_net_collateral_raw: 100_000n,
  };

  it('refuses a quoted redemption before ledger reads, signer access, approval, or backend dispatch', async () => {
    localStorage.setItem('rumi_last_wallet', 'oisy');
    currentWalletType.set(WALLET_TYPES.OISY);
    const result = await ApiClient.redeemQuoted(request, undefined, acceptedOfferFor(request));

    expect(result).toMatchObject({
      success: false,
      error: expect.stringContaining('paused until transfer recovery'),
    });
    expect(mocks.getSignerAgent).not.toHaveBeenCalled();
    expect(mocks.anonAllowance).not.toHaveBeenCalled();
    expect(mocks.anonBalance).not.toHaveBeenCalled();
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.redeem_quoted).not.toHaveBeenCalled();
  });

  it('refuses reserve redemption before signer or approval actions', async () => {
    const result = await ApiClient.redeemReserves(1);

    expect(result.success).toBe(false);
    expect(result.error).toContain('paused until transfer recovery');
    expect(mocks.getSignerAgent).not.toHaveBeenCalled();
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
  });

  it('preflight warms the exact icUSD fee cache before an Oisy click path', async () => {
    currentWalletType.set(WALLET_TYPES.OISY);
    const preflight = await ApiClient.getRedemptionPreflight();

    expect(preflight.feeRaw).toBe(100_000n);
    expect(getFreshCachedLedgerFee({
      ledgerId: CONFIG.currentIcusdLedgerId,
      decimals: 8,
      symbol: 'icUSD',
    })).toBe(100_000n);
  });

  it('recognizes only exact vault-missing close errors', () => {
    const isVaultNotFoundError = (ApiClient as any).isVaultNotFoundError as (message: string, vaultId: number) => boolean;
    expect(isVaultNotFoundError('Vault #42 not found', 42)).toBe(true);
    expect(isVaultNotFoundError('Vault #43 not found', 42)).toBe(false);
    expect(isVaultNotFoundError('Vault not found', 42)).toBe(false);
    expect(isVaultNotFoundError('Price #42 not found', 42)).toBe(false);
    expect(isVaultNotFoundError('unknown vault #42 while fetching price', 42)).toBe(false);
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

describe('ApiClient.addMarginV2Bound — journal before approval', () => {
  const ledger = CKDOGE_LEDGER_ID;
  const amount = 123_456_789n;
  const vaultId = 44n;
  const requestId = 7n;
  const marginStatus = (phase: object, result: object[] = []) => ({
    request_id: requestId,
    owner: Principal.fromText(PRINCIPAL_A),
    ledger: Principal.fromText(ledger),
    operation: { AddMargin: { vault_id: vaultId } },
    phase,
    amount_raw: amount,
    fee_raw: 10_000n,
    memo: [],
    created_at_time_ns: 1n,
    candidate_block_index: [],
    result,
    had_ambiguous_attempt: false,
    last_error: [],
  });

  it('returns an exact completed result without another approval or update', async () => {
    const completed = marginStatus({ Complete: null }, [{ AddMargin: { block_index: 99n } }]);
    backendActor.get_my_collateral_ingress_state.mockResolvedValue({ Ok: {
      next_request_id: 8n, active_request: [], latest_result: [completed],
    } });

    const result = await ApiClient.addMarginV2Bound(makeCtx(PRINCIPAL_A), requestId, Number(vaultId), amount, ledger);

    expect(result).toMatchObject({ kind: 'dispatched_ok', status: completed });
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.add_margin_v2).not.toHaveBeenCalled();
  });

  it('replays exact Pending/Held status without approving again', async () => {
    const held = marginStatus({ Held: null });
    backendActor.get_my_collateral_ingress_state.mockResolvedValue({ Ok: {
      next_request_id: 8n, active_request: [held], latest_result: [],
    } });
    backendActor.add_margin_v2.mockResolvedValue({ Ok: held });

    const result = await ApiClient.addMarginV2Bound(makeCtx(PRINCIPAL_A), requestId, Number(vaultId), amount, ledger);

    expect(result).toMatchObject({ kind: 'dispatched_ok', status: held });
    expect(backendActor.add_margin_v2).toHaveBeenCalledWith(requestId, { vault_id: vaultId, amount });
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
  });

  it('fails closed when the request ID resolves to a different amount', async () => {
    const mismatched = { ...marginStatus({ Held: null }), amount_raw: amount + 1n };
    backendActor.get_my_collateral_ingress_state.mockResolvedValue({ Ok: {
      next_request_id: 8n, active_request: [mismatched], latest_result: [],
    } });

    const result = await ApiClient.addMarginV2Bound(makeCtx(PRINCIPAL_A), requestId, Number(vaultId), amount, ledger);

    expect(result.kind).toBe('predispatch_aborted');
    expect(result.errorMessage).toMatch(/different collateral arguments/i);
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.add_margin_v2).not.toHaveBeenCalled();
  });

  it('persists approval ambiguity before dispatch and sends one exact V2 update', async () => {
    const completed = marginStatus({ Complete: null }, [{ AddMargin: { block_index: 101n } }]);
    backendActor.get_my_collateral_ingress_state.mockResolvedValue({ Ok: {
      next_request_id: requestId, active_request: [], latest_result: [],
    } });
    backendActor.add_margin_v2.mockResolvedValue({ Ok: completed });
    mocks.anonAllowance.mockResolvedValueOnce({ allowance: 0n }).mockResolvedValueOnce({ allowance: amount + 10_000n });
    const order: string[] = [];
    ledgerActor.icrc2_approve.mockImplementation(async () => { order.push('approve'); return { Ok: 1n }; });
    backendActor.add_margin_v2.mockImplementation(async (...args: unknown[]) => {
      order.push('add_margin_v2');
      expect(args).toEqual([requestId, { vault_id: vaultId, amount }]);
      return { Ok: completed };
    });

    const result = await ApiClient.addMarginV2Bound(
      makeCtx(PRINCIPAL_A), requestId, Number(vaultId), amount, ledger, false,
      () => order.push('persist-approval-marker'),
    );

    expect(result).toMatchObject({ kind: 'dispatched_ok', status: completed, approvalMayHaveMutated: true });
    expect(order).toEqual(['persist-approval-marker', 'approve', 'add_margin_v2']);
    expect(ledgerActor.icrc2_approve).toHaveBeenCalledTimes(1);
    expect(ledgerActor.icrc2_approve.mock.calls[0][0].amount).toBe(amount + 20_000n);
  });

  it('does not repeat an ambiguous approval when allowance is still low', async () => {
    backendActor.get_my_collateral_ingress_state.mockResolvedValue({ Ok: {
      next_request_id: requestId, active_request: [], latest_result: [],
    } });
    mocks.anonAllowance.mockResolvedValue({ allowance: 0n });

    const result = await ApiClient.addMarginV2Bound(makeCtx(PRINCIPAL_A), requestId, Number(vaultId), amount, ledger, true);

    expect(result.kind).toBe('predispatch_aborted');
    expect(result.errorMessage).toMatch(/confirm an approval retry to continue/i);
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.add_margin_v2).not.toHaveBeenCalled();
  });

  it('retries an ambiguous approval only after explicit confirmation, reusing the same request ID', async () => {
    const completed = marginStatus({ Complete: null }, [{ AddMargin: { block_index: 103n } }]);
    backendActor.get_my_collateral_ingress_state.mockResolvedValue({ Ok: {
      next_request_id: requestId, active_request: [], latest_result: [],
    } });
    mocks.anonAllowance.mockResolvedValueOnce({ allowance: 0n })
      .mockResolvedValueOnce({ allowance: 0n })
      .mockResolvedValueOnce({ allowance: amount + 10_000n });
    backendActor.add_margin_v2.mockResolvedValue({ Ok: completed });
    const confirmRetry = vi.fn(() => true);
    const order: string[] = [];
    ledgerActor.icrc2_approve.mockImplementation(async () => { order.push('approve'); return { Ok: 2n }; });

    const result = await ApiClient.addMarginV2Bound(
      makeCtx(PRINCIPAL_A), requestId, Number(vaultId), amount, ledger, true,
      () => order.push('persist-marker'), confirmRetry,
    );

    expect(confirmRetry).toHaveBeenCalledTimes(1);
    expect(order).toEqual(['persist-marker', 'approve']);
    expect(ledgerActor.icrc2_approve).toHaveBeenCalledTimes(1);
    expect(backendActor.add_margin_v2).toHaveBeenCalledWith(requestId, { vault_id: vaultId, amount });
    expect(result).toMatchObject({ kind: 'dispatched_ok', status: completed, approvalMayHaveMutated: true });
  });

  it('replays a matching row that appears during the retry confirmation without a second approval', async () => {
    const held = marginStatus({ Held: null });
    backendActor.get_my_collateral_ingress_state
      .mockResolvedValueOnce({ Ok: { next_request_id: requestId, active_request: [], latest_result: [] } })
      .mockResolvedValueOnce({ Ok: { next_request_id: requestId + 1n, active_request: [held], latest_result: [] } });
    mocks.anonAllowance.mockResolvedValue({ allowance: 0n });
    backendActor.add_margin_v2.mockResolvedValue({ Ok: held });

    const result = await ApiClient.addMarginV2Bound(
      makeCtx(PRINCIPAL_A), requestId, Number(vaultId), amount, ledger, true, undefined, () => true,
    );

    expect(result).toMatchObject({ kind: 'dispatched_ok', status: held });
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.add_margin_v2).toHaveBeenCalledWith(requestId, { vault_id: vaultId, amount });
  });

  it('reuses a sufficient reconciled allowance after a prior approval attempt without approving again', async () => {
    const completed = marginStatus({ Complete: null }, [{ AddMargin: { block_index: 102n } }]);
    backendActor.get_my_collateral_ingress_state.mockResolvedValue({ Ok: {
      next_request_id: requestId, active_request: [], latest_result: [],
    } });
    mocks.anonAllowance.mockResolvedValue({ allowance: amount + 10_000n });
    backendActor.add_margin_v2.mockResolvedValue({ Ok: completed });

    const result = await ApiClient.addMarginV2Bound(makeCtx(PRINCIPAL_A), requestId, Number(vaultId), amount, ledger, true);

    expect(result).toMatchObject({ kind: 'dispatched_ok', status: completed, approvalMayHaveMutated: true });
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.add_margin_v2).toHaveBeenCalledWith(requestId, { vault_id: vaultId, amount });
  });

  it('rechecks the bound session after allowance read and stops before approval on account switch', async () => {
    backendActor.get_my_collateral_ingress_state.mockResolvedValue({ Ok: {
      next_request_id: requestId, active_request: [], latest_result: [],
    } });
    mocks.anonAllowance.mockImplementationOnce(async () => {
      setLivePrincipal(PRINCIPAL_B);
      return { allowance: 0n };
    });

    const result = await ApiClient.addMarginV2Bound(makeCtx(PRINCIPAL_A), requestId, Number(vaultId), amount, ledger);

    expect(result.kind).toBe('predispatch_aborted');
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(backendActor.add_margin_v2).not.toHaveBeenCalled();
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
