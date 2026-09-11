import { Principal } from '@dfinity/principal';
import { flushSync, tick } from 'svelte';
import { DOGE_BORROW_INTENT_VERSION, icusdAmountToRawE8s, type DogeBorrowIntentRecord } from '../../src/lib/utils/dogeBorrowWizard';

/**
 * Test-only fixture data for dogeBorrowPage.fixture.spec.ts. Deterministic,
 * synthetic values only — no real principals, wallets, or mainnet reads.
 * Safe to import normally (no vi.mock involvement, no side effects).
 */

export const PRINCIPAL_A = Principal.fromUint8Array(Uint8Array.from([1, 1, 1, 1]));
export const PRINCIPAL_B = Principal.fromUint8Array(Uint8Array.from([2, 2, 2, 2]));
export const PRINCIPAL_A_TEXT = PRINCIPAL_A.toText();
export const PRINCIPAL_B_TEXT = PRINCIPAL_B.toText();

export const CKDOGE_LEDGER_TEXT = 'efmc5-wyaaa-aaaar-qb3wa-cai';

export function fakeCollateralInfo(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    principal: CKDOGE_LEDGER_TEXT,
    symbol: 'ckDOGE',
    price: 0.08,
    liquidationCr: 1.2,
    minimumCr: 1.35,
    borrowingFee: 0.005,
    debtCeiling: 1_000_000,
    status: 'Active',
    ...overrides,
  };
}

/** Shape returned by the backend for get_vaults, as consumed by fetchVaultLites in +page.svelte. */
export function fakeRawVault(params: {
  vaultId: number;
  collateralPrincipalText?: string;
  collateralAmount: bigint;
  borrowedIcusd: bigint;
}) {
  return {
    vault_id: BigInt(params.vaultId),
    collateral_type: { toText: () => params.collateralPrincipalText ?? CKDOGE_LEDGER_TEXT },
    collateral_amount: params.collateralAmount,
    borrowed_icusd_amount: params.borrowedIcusd,
  };
}

export function fakeMinterInfo(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    min_confirmations: 60,
    deposit_doge_min_amount: 100_000_000n,
    retrieve_doge_min_amount: 500_000_000n,
    ...overrides,
  };
}

export function fakeMintedUtxoResult(blockIndex: bigint, koinuAmount: bigint) {
  return { Ok: [{ Minted: { block_index: blockIndex, minted_amount: koinuAmount } }] };
}

export function fakeCheckedUtxoResult() {
  return { Ok: [{ Checked: null }] };
}

export function fakeNoNewUtxosError(currentConfirmations: number, requiredConfirmations: number) {
  return {
    Err: {
      NoNewUtxos: {
        current_confirmations: [currentConfirmations],
        required_confirmations: requiredConfirmations,
        pending_utxos: [[]],
      },
    },
  };
}

/**
 * Matches DogeBorrowIntentRecord in dogeBorrowWizard.ts (version 2, as of this
 * fixture's authoring). The version tag is imported from the real module
 * (not hand-copied) because it is expected to keep moving as the repair
 * worker iterates; every OTHER field is still hand-listed so a genuinely new
 * required field shows up as a real spec/type failure here, not a silently
 * self-updating fixture.
 */
export function fakeIntentRecord(params: {
  principal: string;
  step: 'choose' | 'signin' | 'send' | 'confirm' | 'done';
  collateralAmountDoge: number;
  icusdAmount: number;
  vaultId: number | null;
  borrowConfirmed: boolean;
  mintedBlockIndices?: string[];
  sessionMintedKoinu?: string;
  pendingAction?: 'open_and_borrow' | 'finish_borrow' | null;
  preActionVaultIds?: number[];
  submittedCollateralKoinu?: string | null;
  submittedIcusdAmount?: number | null;
  submittedIcusdAmountRaw?: string | null;
  partialBorrowAcknowledged?: boolean;
  now: number;
}): DogeBorrowIntentRecord {
  return {
    version: DOGE_BORROW_INTENT_VERSION,
    principal: params.principal,
    createdAt: params.now,
    updatedAt: params.now,
    step: params.step,
    collateralAmountDoge: params.collateralAmountDoge,
    icusdAmount: params.icusdAmount,
    mintedBlockIndices: params.mintedBlockIndices ?? [],
    sessionMintedKoinu: params.sessionMintedKoinu ?? '0',
    vaultId: params.vaultId,
    borrowConfirmed: params.borrowConfirmed,
    pendingAction: params.pendingAction ?? null,
    preActionVaultIds: params.preActionVaultIds ?? [],
    submittedCollateralKoinu: params.submittedCollateralKoinu ?? null,
    submittedIcusdAmount: params.submittedIcusdAmount ?? null,
    submittedIcusdAmountRaw:
      params.submittedIcusdAmountRaw ?? (params.submittedIcusdAmount != null ? icusdAmountToRawE8s(params.submittedIcusdAmount).toString() : null),
    partialBorrowAcknowledged: params.partialBorrowAcknowledged ?? false,
  };
}

/** A VaultDTO-shaped record (src/lib/services/types.ts) as returned by appDataStore's userVaults — the frontend-normalized shape (human-readable numbers), distinct from the raw candid get_vaults() shape used by fakeRawVault. */
export function fakeVaultDto(params: {
  vaultId: number;
  owner: string;
  collateralType?: string;
  collateralAmount: number;
  borrowedIcusd: number;
  collateralSymbol?: string;
  collateralDecimals?: number;
}) {
  return {
    vaultId: params.vaultId,
    owner: params.owner,
    icpMargin: 0,
    borrowedIcusd: params.borrowedIcusd,
    collateralType: params.collateralType ?? CKDOGE_LEDGER_TEXT,
    collateralAmount: params.collateralAmount,
    collateralSymbol: params.collateralSymbol ?? 'ckDOGE',
    collateralDecimals: params.collateralDecimals ?? 8,
    accruedInterest: 0,
  };
}

export function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Flushes microtasks + Svelte's reactive scheduler a few times, for settling chained awaits inside component effects. */
export async function settle(rounds = 12) {
  for (let i = 0; i < rounds; i++) {
    await Promise.resolve();
    await tick();
    flushSync();
  }
}

export function setInputValue(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!;
  setter.call(input, value);
  input.dispatchEvent(new Event('input', { bubbles: true }));
}
