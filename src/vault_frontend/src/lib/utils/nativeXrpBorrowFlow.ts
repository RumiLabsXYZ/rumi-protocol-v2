import type { CollateralInfo } from '$lib/services/types';

export const NATIVE_XRP_ACCOUNT_RESERVE = 1;

export interface NativeXrpDepositIntent {
  collateralAmount: number;
  icusdAmount: number;
  collateralInfo?: Pick<CollateralInfo, 'symbol' | 'custodyKind'>;
}

export interface NativeXrpDepositCopy {
  assetName: string;
  sendAmount: number;
  reserveAmount: number;
  sendAmountLabel: string;
  collateralAmountLabel: string;
  reserveAmountLabel: string;
  borrowAmountLabel: string;
  reserveExplanation: string;
}

export type NativeXrpBorrowPhase =
  | 'opening'
  | 'awaiting'
  | 'confirming'
  | 'borrowing'
  | 'borrow_failed'
  | 'error';

export function isNativeXrpCollateral(
  collateralInfo: Pick<CollateralInfo, 'custodyKind'> | undefined
): boolean {
  return collateralInfo?.custodyKind === 'NativeXrp';
}

export function formatXrpAmount(amount: number): string {
  const rounded = amount.toFixed(6).replace(/\.?0+$/, '');
  return `${rounded} XRP`;
}

export function buildXrpPaymentUri(address: string, amount: number): string {
  const amountParam = formatXrpAmount(amount).replace(/ XRP$/, '');
  return `ripple:${encodeURIComponent(address)}?amount=${encodeURIComponent(amountParam)}`;
}

export function nativeXrpDepositCopy(intent: NativeXrpDepositIntent): NativeXrpDepositCopy {
  const assetName = intent.collateralInfo?.symbol || 'XRP';
  const reserveAmount = NATIVE_XRP_ACCOUNT_RESERVE;
  const sendAmount = intent.collateralAmount + reserveAmount;
  const collateralAmountLabel = formatXrpAmount(intent.collateralAmount);
  const reserveAmountLabel = formatXrpAmount(reserveAmount);
  return {
    assetName,
    sendAmount,
    reserveAmount,
    sendAmountLabel: formatXrpAmount(sendAmount),
    collateralAmountLabel,
    reserveAmountLabel,
    borrowAmountLabel: `${intent.icusdAmount.toFixed(2)} icUSD`,
    reserveExplanation: `${collateralAmountLabel} collateral + ${reserveAmountLabel} XRPL account reserve. The reserve activates this XRP address and stays locked there; your vault stays open so you do not pay it again.`,
  };
}

export function nativeXrpModalTitle(phase: NativeXrpBorrowPhase, hasDepositAddress: boolean): string {
  if (phase === 'error' && !hasDepositAddress) {
    return 'Could not prepare XRP address';
  }
  if (phase === 'opening' || !hasDepositAddress) {
    return 'Approve in OISY to generate your XRP address';
  }
  return 'Send XRP to open your vault';
}

export function nativeXrpModalStatusLabel(phase: NativeXrpBorrowPhase): string {
  switch (phase) {
    case 'opening':
      return 'Approve in OISY';
    case 'awaiting':
      return 'Awaiting deposit';
    case 'confirming':
      return 'Checking XRPL';
    case 'borrowing':
      return 'Minting icUSD';
    case 'borrow_failed':
      return 'Borrow paused';
    case 'error':
      return 'Needs attention';
  }
}

export function nativeXrpModalOpeningCopy(): string {
  return 'Approve open_xrp_vault in OISY. We will show your XRP deposit address and QR code after approval.';
}

export function nativeXrpModalShouldRender(
  phase: NativeXrpBorrowPhase,
  hasDepositAddress: boolean
): boolean {
  return phase !== 'opening' || hasDepositAddress;
}

export function nativeXrpModalPrimaryActionLabel(
  phase: NativeXrpBorrowPhase,
  hasDepositAddress: boolean
): string | null {
  if (!hasDepositAddress) return null;

  switch (phase) {
    case 'awaiting':
      return "I've sent the XRP";
    case 'confirming':
      return 'Checking deposit...';
    case 'borrowing':
      return 'Minting icUSD...';
    default:
      return null;
  }
}

export function nativeXrpKeepOpenCloseCopy(): string {
  return 'The XRP account reserve stays locked on XRPL, so the vault stays open. You do not need to pay that reserve again, and the vault will be ready when you want to use this XRP address later.';
}
