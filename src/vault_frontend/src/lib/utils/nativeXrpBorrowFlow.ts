import type { CollateralInfo } from '$lib/services/types';

export const XRP_DROPS_PER_XRP = 1_000_000;

export interface NativeXrpDepositIntent {
  collateralAmount: number;
  icusdAmount: number;
  reserveBaseDrops: bigint | number;
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
  | 'ready_to_borrow'
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

export function xrpAmountFromDrops(drops: bigint | number): number {
  return Number(drops) / XRP_DROPS_PER_XRP;
}

export function buildXrpPaymentUri(address: string, amount: number): string {
  const amountParam = formatXrpAmount(amount).replace(/ XRP$/, '');
  return `ripple:${encodeURIComponent(address)}?amount=${encodeURIComponent(amountParam)}`;
}

export function nativeXrpDepositCopy(intent: NativeXrpDepositIntent): NativeXrpDepositCopy {
  const assetName = intent.collateralInfo?.symbol || 'XRP';
  const reserveAmount = xrpAmountFromDrops(intent.reserveBaseDrops);
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
  switch (phase) {
    case 'ready_to_borrow':
      return 'XRP received — approve your borrow';
    case 'borrowing':
      return 'Minting your icUSD';
    case 'borrow_failed':
      return 'XRP received — borrow not finished';
    default:
      return 'Send XRP to open your vault';
  }
}

export function nativeXrpModalStatusLabel(phase: NativeXrpBorrowPhase): string {
  switch (phase) {
    case 'opening':
      return 'Approve in OISY';
    case 'awaiting':
      return 'Awaiting deposit';
    case 'confirming':
      return 'Checking XRPL';
    case 'ready_to_borrow':
      return 'Ready to borrow';
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
  hasDepositAddress: boolean,
  borrowAmountLabel?: string
): string | null {
  if (!hasDepositAddress) return null;

  switch (phase) {
    case 'awaiting':
      return "I've sent the XRP";
    case 'confirming':
      return 'Checking deposit...';
    // The borrow is a SECOND wallet approval and must come from its own click —
    // see nativeXrpBorrowSeparateApprovalCopy(). Both the ready and failed
    // states offer the same action so a user is never stranded mid-flow.
    case 'ready_to_borrow':
    case 'borrow_failed':
      return borrowAmountLabel ? `Borrow ${borrowAmountLabel}` : 'Borrow icUSD';
    case 'borrowing':
      return 'Minting icUSD...';
    default:
      return null;
  }
}

/**
 * Why the borrow needs its own click.
 *
 * Confirming the deposit and borrowing are two separate canister calls, so they
 * are two separate wallet approvals. Oisy will only open its signer popup from
 * inside a browser user-gesture, and the confirm call's XRPL verification
 * round-trip burns that gesture window — so an auto-borrow chained onto the
 * confirm always dies with "Signer window should not be opened outside of click
 * handler". ICRC-112 batching would have allowed one approval for both calls,
 * but no wallet ever adopted it (see services/pnp.ts). The honest fix is to ask
 * for a second, deliberate click.
 */
export function nativeXrpBorrowSeparateApprovalCopy(borrowAmountLabel: string): string {
  return `Your XRP is in the vault. Borrowing ${borrowAmountLabel} is a separate wallet approval, so approve it to finish.`;
}

/** Reassurance that closing now is safe, and where to pick the borrow back up. */
export function nativeXrpBorrowLaterCopy(vaultId: number): string {
  return `Nothing is at risk if you stop here — your XRP is already collateral in vault #${vaultId}. You can borrow against it any time from the Vaults page.`;
}

const SIGNER_GESTURE_HINT = 'signer window';

/**
 * Turn a raw wallet/signer error into copy a user can act on. The signer-window
 * error in particular is meaningless to a user and, in this flow, always means
 * "the borrow needs its own click" rather than a real failure.
 */
export function nativeXrpBorrowErrorCopy(rawError: string | undefined, borrowAmountLabel: string): string {
  if (rawError && rawError.toLowerCase().includes(SIGNER_GESTURE_HINT)) {
    return `Your wallet needs a fresh approval for the borrow. Tap Borrow ${borrowAmountLabel} to open it.`;
  }
  return rawError ?? 'Deposit confirmed, but borrowing failed.';
}

export function nativeXrpKeepOpenCloseCopy(): string {
  return 'The XRP account reserve stays locked on XRPL, so the vault stays open. You do not need to pay that reserve again, and the vault will be ready when you want to use this XRP address later.';
}
