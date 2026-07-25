import { describe, expect, it } from 'vitest';
import {
  buildXrpPaymentUri,
  formatXrpAmount,
  isNativeXrpCollateral,
  nativeXrpBorrowErrorCopy,
  nativeXrpBorrowLaterCopy,
  nativeXrpBorrowSeparateApprovalCopy,
  nativeXrpDepositCopy,
  nativeXrpKeepOpenCloseCopy,
  nativeXrpModalOpeningCopy,
  nativeXrpModalPrimaryActionLabel,
  nativeXrpModalShouldRender,
  nativeXrpModalStatusLabel,
  nativeXrpModalTitle,
  xrpCreditedCollateral,
} from './nativeXrpBorrowFlow';
import type { CollateralInfo } from '$lib/services/types';

const xrpCollateral = {
  symbol: 'XRP',
  decimals: 6,
  custodyKind: 'NativeXrp',
} as CollateralInfo;

describe('native XRP borrow flow helpers', () => {
  it('routes only NativeXrp collateral into the native deposit flow', () => {
    expect(isNativeXrpCollateral(xrpCollateral)).toBe(true);
    expect(isNativeXrpCollateral({ ...xrpCollateral, custodyKind: 'IcrcLedger' })).toBe(false);
    expect(isNativeXrpCollateral(undefined)).toBe(false);
  });

  it('formats the exact XRP amount using six decimal places without noisy trailing zeroes', () => {
    expect(formatXrpAmount(1)).toBe('1 XRP');
    expect(formatXrpAmount(1.25)).toBe('1.25 XRP');
    expect(formatXrpAmount(1.2345678)).toBe('1.234568 XRP');
  });

  it('builds a scan-friendly XRP payment URI with the address and amount', () => {
    expect(buildXrpPaymentUri('rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh', 2.5)).toBe(
      'ripple:rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh?amount=2.5'
    );
  });

  it('summarizes the deposit and borrow intent for modal copy', () => {
    const copy = nativeXrpDepositCopy({
      collateralAmount: 12.345678,
      icusdAmount: 4.5,
      reserveBaseDrops: 1_250_000n,
      collateralInfo: xrpCollateral,
    });

    // The user sends EXACTLY what they typed; the reserve comes out of it.
    expect(copy.sendAmountLabel).toBe('12.345678 XRP');
    expect(copy.collateralAmountLabel).toBe('11.095678 XRP');
    expect(copy.reserveAmountLabel).toBe('1.25 XRP');
    expect(copy.sendAmount).toBe(12.345678);
    expect(copy.creditedAmount).toBeCloseTo(11.095678, 6);
    expect(copy.reserveAmount).toBe(1.25);
    expect(copy.borrowAmountLabel).toBe('4.50 icUSD');
    expect(copy.assetName).toBe('XRP');
  });

  it('never asks the user to send more than the amount they entered', () => {
    const entered = 5000;
    const copy = nativeXrpDepositCopy({
      collateralAmount: entered,
      icusdAmount: 100,
      reserveBaseDrops: 1_000_000n,
      collateralInfo: xrpCollateral,
    });

    expect(copy.sendAmount).toBe(entered);
    expect(copy.sendAmountLabel).toBe('5000 XRP');
    expect(copy.creditedAmount).toBe(4999);
    expect(buildXrpPaymentUri('rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh', copy.sendAmount)).toBe(
      'ripple:rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh?amount=5000'
    );
  });

  it('explains the split between credited XRP collateral and the XRPL reserve', () => {
    const copy = nativeXrpDepositCopy({
      collateralAmount: 3,
      icusdAmount: 0.5,
      reserveBaseDrops: 1_000_000n,
      collateralInfo: xrpCollateral,
    });

    expect(copy.sendAmountLabel).toBe('3 XRP');
    expect(copy.reserveExplanation).toContain('Of the 3 XRP you send');
    expect(copy.reserveExplanation).toContain('1 XRP is the XRPL account reserve');
    expect(copy.reserveExplanation).toContain('2 XRP becomes your collateral');
    expect(buildXrpPaymentUri('rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh', copy.sendAmount)).toBe(
      'ripple:rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh?amount=3'
    );
  });

  it('never credits negative collateral when the send amount is at or below the reserve', () => {
    expect(xrpCreditedCollateral(5000, 1)).toBe(4999);
    expect(xrpCreditedCollateral(1, 1)).toBe(0);
    expect(xrpCreditedCollateral(0.5, 1)).toBe(0);

    const copy = nativeXrpDepositCopy({
      collateralAmount: 0.5,
      icusdAmount: 0,
      reserveBaseDrops: 1_000_000n,
      collateralInfo: xrpCollateral,
    });
    expect(copy.creditedAmount).toBe(0);
  });

  it('explains that native XRP reserve stays locked and the vault stays open', () => {
    expect(nativeXrpKeepOpenCloseCopy()).toContain('XRP account reserve');
    expect(nativeXrpKeepOpenCloseCopy()).toContain('vault stays open');
  });

  it('keeps the opening state focused on wallet approval before an XRP address exists', () => {
    expect(nativeXrpModalTitle('opening', false)).toBe('Approve in OISY to generate your XRP address');
    expect(nativeXrpModalStatusLabel('opening')).toBe('Approve in OISY');
    expect(nativeXrpModalOpeningCopy()).toContain('show your XRP deposit address');
    expect(nativeXrpModalShouldRender('opening', false)).toBe(false);
    expect(nativeXrpModalPrimaryActionLabel('opening', false)).toBeNull();
  });

  it('shows the sent-deposit action only after the XRP custody address is ready', () => {
    expect(nativeXrpModalTitle('awaiting', true)).toBe('Send XRP to open your vault');
    expect(nativeXrpModalShouldRender('awaiting', true)).toBe(true);
    expect(nativeXrpModalShouldRender('error', false)).toBe(true);
    expect(nativeXrpModalPrimaryActionLabel('awaiting', true)).toBe("I've sent the XRP");
    expect(nativeXrpModalPrimaryActionLabel('confirming', true)).toBe('Checking deposit...');
    expect(nativeXrpModalPrimaryActionLabel('borrowing', true)).toBe('Minting icUSD...');
    expect(nativeXrpModalPrimaryActionLabel('error', true)).toBeNull();
  });

  // Confirm-deposit and borrow are two canister calls, so two wallet approvals.
  // Oisy only opens its signer from a user-gesture, and the confirm round-trip
  // burns it, so the borrow must come from its own click.
  it('offers the borrow as its own action once the deposit is credited', () => {
    expect(nativeXrpModalTitle('ready_to_borrow', true)).toBe('XRP received — approve your borrow');
    expect(nativeXrpModalStatusLabel('ready_to_borrow')).toBe('Ready to borrow');
    expect(nativeXrpModalPrimaryActionLabel('ready_to_borrow', true, '4.50 icUSD')).toBe('Borrow 4.50 icUSD');
    expect(nativeXrpModalShouldRender('ready_to_borrow', true)).toBe(true);
  });

  it('keeps the same borrow action available after a failed borrow so the user is never stranded', () => {
    expect(nativeXrpModalTitle('borrow_failed', true)).toBe('XRP received — borrow not finished');
    expect(nativeXrpModalPrimaryActionLabel('borrow_failed', true, '4.50 icUSD')).toBe('Borrow 4.50 icUSD');
    expect(nativeXrpModalPrimaryActionLabel('ready_to_borrow', true)).toBe('Borrow icUSD');
  });

  it('never shows a borrow action before the custody address exists', () => {
    expect(nativeXrpModalPrimaryActionLabel('ready_to_borrow', false, '4.50 icUSD')).toBeNull();
    expect(nativeXrpModalPrimaryActionLabel('borrow_failed', false, '4.50 icUSD')).toBeNull();
  });

  it('translates the raw signer-window error into an actionable instruction', () => {
    expect(
      nativeXrpBorrowErrorCopy(
        'Signer window should not be opened outside of click handler',
        '4.50 icUSD'
      )
    ).toBe('Your wallet needs a fresh approval for the borrow. Tap Borrow 4.50 icUSD to open it.');

    // Case-insensitive, so wording variants from the signer still map across.
    expect(nativeXrpBorrowErrorCopy('The Signer Window is already open', '4.50 icUSD')).toContain(
      'fresh approval'
    );
  });

  it('passes through a genuine borrow failure instead of blaming the wallet', () => {
    expect(nativeXrpBorrowErrorCopy('Debt ceiling reached for XRP', '4.50 icUSD')).toBe(
      'Debt ceiling reached for XRP'
    );
    expect(nativeXrpBorrowErrorCopy(undefined, '4.50 icUSD')).toBe(
      'Deposit confirmed, but borrowing failed.'
    );
  });

  it('tells the user their collateral is safe and where to finish the borrow later', () => {
    expect(nativeXrpBorrowSeparateApprovalCopy('4.50 icUSD')).toContain('separate wallet approval');
    const later = nativeXrpBorrowLaterCopy(198);
    expect(later).toContain('vault #198');
    expect(later).toContain('Vaults page');
  });
});
