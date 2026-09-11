import { describe, it, expect, vi, beforeEach } from 'vitest';
import { Principal } from '@dfinity/principal';

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
      createActor: vi.fn(() => ({ icrc2_allowance: mocks.anonAllowance })),
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
import {
  ApiClient,
  type BoundOpenVaultAndBorrowResult,
  type BoundBorrowFromVaultResult,
} from './apiClient';
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

/** A context that is current until `live` is flipped false — models both an
 * account switch (principal text mismatch, checked separately by
 * assertActionBoundContextCurrent) and a same-principal disconnect/reconnect
 * (generation bump — assertCurrent alone flips false, principal text unchanged). */
function makeCtx(expectedPrincipalText: string, assertCurrent: () => boolean = () => true): ActionBoundContext {
  return { expectedPrincipalText, assertCurrent };
}

let backendActor: { open_vault_and_borrow: ReturnType<typeof vi.fn>; borrow_from_vault: ReturnType<typeof vi.fn> };
let ledgerActor: { icrc2_approve: ReturnType<typeof vi.fn> };

beforeEach(() => {
  vi.clearAllMocks();
  vi.spyOn(console, 'log').mockImplementation(() => {});
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  vi.spyOn(console, 'error').mockImplementation(() => {});
  localStorage.clear();

  backendActor = {
    open_vault_and_borrow: vi.fn().mockResolvedValue({ Ok: { vault_id: 7n, block_index: 99n } }),
    borrow_from_vault: vi.fn().mockResolvedValue({ Ok: { block_index: 55n, fee_amount_paid: 1_000n } }),
  };
  ledgerActor = {
    icrc2_approve: vi.fn().mockResolvedValue({ Ok: 1n }),
  };

  mocks.getActor.mockImplementation(async (canisterId: string) => {
    if (canisterId === BACKEND_ID) return backendActor;
    if (canisterId === CKDOGE_LEDGER_ID) return ledgerActor;
    throw new Error(`unexpected getActor(${canisterId})`);
  });
  mocks.anonAllowance.mockResolvedValue({ allowance: 0n });
  mocks.getSignerAgent.mockResolvedValue(null);

  setLivePrincipal(PRINCIPAL_A);
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
