import { describe, expect, it } from 'vitest';
import {
  buildSolPaymentUri,
  formatSolAmount,
  isNativeSolCollateral,
  nativeSolBorrowErrorCopy,
  nativeSolBorrowLaterCopy,
  nativeSolBorrowSeparateApprovalCopy,
  nativeSolDepositCopy,
  nativeSolKeepOpenCloseCopy,
  nativeSolModalOpeningCopy,
  nativeSolModalPrimaryActionLabel,
  nativeSolModalShouldRender,
  nativeSolModalStatusLabel,
  nativeSolModalTitle,
  solCreditedCollateral,
} from './nativeSolBorrowFlow';
import type { CollateralInfo } from '$lib/services/types';

const solCollateral = {
  symbol: 'SOL',
  decimals: 9,
  custodyKind: 'NativeSol',
} as CollateralInfo;

describe('native SOL borrow flow helpers', () => {
  it('routes only NativeSol collateral into the native deposit flow', () => {
    expect(isNativeSolCollateral(solCollateral)).toBe(true);
    expect(isNativeSolCollateral({ ...solCollateral, custodyKind: 'IcrcLedger' })).toBe(false);
    expect(isNativeSolCollateral({ ...solCollateral, custodyKind: 'NativeXrp' })).toBe(false);
    expect(isNativeSolCollateral(undefined)).toBe(false);
  });

  it('formats the exact SOL amount using nine decimal places without noisy trailing zeroes', () => {
    expect(formatSolAmount(1)).toBe('1 SOL');
    expect(formatSolAmount(1.25)).toBe('1.25 SOL');
    expect(formatSolAmount(1.234567891)).toBe('1.234567891 SOL');
  });

  it('builds a Solana Pay URI with the address and amount in SOL, not lamports', () => {
    expect(buildSolPaymentUri('7ZWZzVxYw2z8b3z1JYQq6nT9hK1p4c6dR3qU9mN2sT8p', 2.5)).toBe(
      'solana:7ZWZzVxYw2z8b3z1JYQq6nT9hK1p4c6dR3qU9mN2sT8p?amount=2.5'
    );
  });

  it('summarizes the deposit and borrow intent for modal copy', () => {
    const copy = nativeSolDepositCopy({
      collateralAmount: 12.345,
      icusdAmount: 4.5,
      rentExemptLamports: 890_880n,
      collateralInfo: solCollateral,
    });

    // The user sends EXACTLY what they typed; the reserve comes out of it.
    expect(copy.sendAmountLabel).toBe('12.345 SOL');
    expect(copy.collateralAmountLabel).toBe('12.34410912 SOL');
    expect(copy.reserveAmountLabel).toBe('0.00089088 SOL');
    expect(copy.sendAmount).toBe(12.345);
    expect(copy.creditedAmount).toBeCloseTo(12.34410912, 8);
    expect(copy.borrowAmountLabel).toBe('4.50 icUSD');
    expect(copy.assetName).toBe('SOL');
  });

  it('never asks the user to send more than the amount they entered', () => {
    const entered = 5000;
    const copy = nativeSolDepositCopy({
      collateralAmount: entered,
      icusdAmount: 100,
      rentExemptLamports: 1_000_000_000n,
      collateralInfo: solCollateral,
    });

    expect(copy.sendAmount).toBe(entered);
    expect(copy.sendAmountLabel).toBe('5000 SOL');
    expect(copy.creditedAmount).toBe(4999);
    expect(buildSolPaymentUri('address', copy.sendAmount)).toBe('solana:address?amount=5000');
  });

  it('explains the split between credited SOL collateral and the rent-exempt reserve', () => {
    const copy = nativeSolDepositCopy({
      collateralAmount: 3,
      icusdAmount: 0.5,
      rentExemptLamports: 1_000_000_000n,
      collateralInfo: solCollateral,
    });

    expect(copy.sendAmountLabel).toBe('3 SOL');
    expect(copy.reserveExplanation).toContain('Of the 3 SOL you send');
    expect(copy.reserveExplanation).toContain('1 SOL is the Solana rent-exempt reserve');
    expect(copy.reserveExplanation).toContain('2 SOL becomes your collateral');
    expect(buildSolPaymentUri('address', copy.sendAmount)).toBe('solana:address?amount=3');
  });

  it('never credits negative collateral when the send amount is at or below the reserve', () => {
    expect(solCreditedCollateral(5000, 1)).toBe(4999);
    expect(solCreditedCollateral(1, 1)).toBe(0);
    expect(solCreditedCollateral(0.5, 1)).toBe(0);

    const copy = nativeSolDepositCopy({
      collateralAmount: 0.5,
      icusdAmount: 0,
      rentExemptLamports: 1_000_000_000n,
      collateralInfo: solCollateral,
    });
    expect(copy.creditedAmount).toBe(0);
  });

  it('explains that the SOL rent-exempt reserve stays locked and the vault stays open', () => {
    expect(nativeSolKeepOpenCloseCopy()).toContain('SOL rent-exempt reserve');
    expect(nativeSolKeepOpenCloseCopy()).toContain('vault stays open');
  });

  it('keeps the opening state focused on wallet approval before a SOL address exists', () => {
    expect(nativeSolModalTitle('opening', false)).toBe('Approve in OISY to generate your SOL address');
    expect(nativeSolModalStatusLabel('opening')).toBe('Approve in OISY');
    expect(nativeSolModalOpeningCopy()).toContain('show your SOL deposit address');
    expect(nativeSolModalShouldRender('opening', false)).toBe(false);
    expect(nativeSolModalPrimaryActionLabel('opening', false)).toBeNull();
  });

  it('shows the sent-deposit action only after the SOL custody address is ready', () => {
    expect(nativeSolModalTitle('awaiting', true)).toBe('Send SOL to open your vault');
    expect(nativeSolModalShouldRender('awaiting', true)).toBe(true);
    expect(nativeSolModalShouldRender('error', false)).toBe(true);
    expect(nativeSolModalPrimaryActionLabel('awaiting', true)).toBe("I've sent the SOL");
    expect(nativeSolModalPrimaryActionLabel('confirming', true)).toBe('Checking deposit...');
    expect(nativeSolModalPrimaryActionLabel('borrowing', true)).toBe('Minting icUSD...');
    expect(nativeSolModalPrimaryActionLabel('error', true)).toBeNull();
  });

  // Confirm-deposit and borrow are two canister calls, so two wallet approvals.
  // Oisy only opens its signer from a user-gesture, and the confirm round-trip
  // burns it, so the borrow must come from its own click. Mirrors XRP exactly.
  it('offers the borrow as its own action once the deposit is credited', () => {
    expect(nativeSolModalTitle('ready_to_borrow', true)).toBe('SOL received, approve your borrow');
    expect(nativeSolModalStatusLabel('ready_to_borrow')).toBe('Ready to borrow');
    expect(nativeSolModalPrimaryActionLabel('ready_to_borrow', true, '4.50 icUSD')).toBe('Borrow 4.50 icUSD');
    expect(nativeSolModalShouldRender('ready_to_borrow', true)).toBe(true);
  });

  it('keeps the same borrow action available after a failed borrow so the user is never stranded', () => {
    expect(nativeSolModalTitle('borrow_failed', true)).toBe('SOL received, borrow not finished');
    expect(nativeSolModalPrimaryActionLabel('borrow_failed', true, '4.50 icUSD')).toBe('Borrow 4.50 icUSD');
    expect(nativeSolModalPrimaryActionLabel('ready_to_borrow', true)).toBe('Borrow icUSD');
  });

  it('never shows a borrow action before the custody address exists', () => {
    expect(nativeSolModalPrimaryActionLabel('ready_to_borrow', false, '4.50 icUSD')).toBeNull();
    expect(nativeSolModalPrimaryActionLabel('borrow_failed', false, '4.50 icUSD')).toBeNull();
  });

  it('translates the raw signer-window error into an actionable instruction', () => {
    expect(
      nativeSolBorrowErrorCopy(
        'Signer window should not be opened outside of click handler',
        '4.50 icUSD'
      )
    ).toBe('Your wallet needs a fresh approval for the borrow. Tap Borrow 4.50 icUSD to open it.');

    // Case-insensitive, so wording variants from the signer still map across.
    expect(nativeSolBorrowErrorCopy('The Signer Window is already open', '4.50 icUSD')).toContain(
      'fresh approval'
    );
  });

  it('passes through a genuine borrow failure instead of blaming the wallet', () => {
    expect(nativeSolBorrowErrorCopy('Debt ceiling reached for SOL', '4.50 icUSD')).toBe(
      'Debt ceiling reached for SOL'
    );
    expect(nativeSolBorrowErrorCopy(undefined, '4.50 icUSD')).toBe(
      'Deposit confirmed, but borrowing failed.'
    );
  });

  it('tells the user their collateral is safe and where to finish the borrow later', () => {
    expect(nativeSolBorrowSeparateApprovalCopy('4.50 icUSD')).toContain('separate wallet approval');
    const later = nativeSolBorrowLaterCopy(198);
    expect(later).toContain('vault #198');
    expect(later).toContain('Vaults page');
  });
});
