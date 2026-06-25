import { describe, expect, it } from 'vitest';
import {
  buildXrpPaymentUri,
  formatXrpAmount,
  isNativeXrpCollateral,
  nativeXrpDepositCopy,
  nativeXrpKeepOpenCloseCopy,
  nativeXrpModalOpeningCopy,
  nativeXrpModalPrimaryActionLabel,
  nativeXrpModalShouldRender,
  nativeXrpModalStatusLabel,
  nativeXrpModalTitle,
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

    expect(copy.sendAmountLabel).toBe('13.595678 XRP');
    expect(copy.collateralAmountLabel).toBe('12.345678 XRP');
    expect(copy.reserveAmountLabel).toBe('1.25 XRP');
    expect(copy.sendAmount).toBe(13.595678);
    expect(copy.reserveAmount).toBe(1.25);
    expect(copy.borrowAmountLabel).toBe('4.50 icUSD');
    expect(copy.assetName).toBe('XRP');
  });

  it('explains the split between credited XRP collateral and the XRPL reserve', () => {
    const copy = nativeXrpDepositCopy({
      collateralAmount: 2,
      icusdAmount: 0.5,
      reserveBaseDrops: 1_000_000n,
      collateralInfo: xrpCollateral,
    });

    expect(copy.sendAmountLabel).toBe('3 XRP');
    expect(copy.reserveExplanation).toContain('2 XRP collateral');
    expect(copy.reserveExplanation).toContain('1 XRP XRPL account reserve');
    expect(buildXrpPaymentUri('rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh', copy.sendAmount)).toBe(
      'ripple:rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh?amount=3'
    );
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
});
