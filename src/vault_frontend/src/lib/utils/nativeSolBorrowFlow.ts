import type { CollateralInfo } from '$lib/services/types';

export const SOL_LAMPORTS_PER_SOL = 1_000_000_000;

export interface NativeSolDepositIntent {
  collateralAmount: number;
  icusdAmount: number;
  rentExemptLamports: bigint | number;
  collateralInfo?: Pick<CollateralInfo, 'symbol' | 'custodyKind'>;
}

export interface NativeSolDepositCopy {
  assetName: string;
  sendAmount: number;
  reserveAmount: number;
  sendAmountLabel: string;
  collateralAmountLabel: string;
  reserveAmountLabel: string;
  borrowAmountLabel: string;
  reserveExplanation: string;
}

export type NativeSolBorrowPhase =
  | 'opening'
  | 'awaiting'
  | 'confirming'
  | 'ready_to_borrow'
  | 'borrowing'
  | 'borrow_failed'
  | 'error';

export function isNativeSolCollateral(
  collateralInfo: Pick<CollateralInfo, 'custodyKind'> | undefined
): boolean {
  return collateralInfo?.custodyKind === 'NativeSol';
}

export function formatSolAmount(amount: number): string {
  const rounded = amount.toFixed(9).replace(/\.?0+$/, '');
  return `${rounded} SOL`;
}

export function solAmountFromLamports(lamports: bigint | number): number {
  return Number(lamports) / SOL_LAMPORTS_PER_SOL;
}

/**
 * Solana Pay transfer request URI. Note the amount parameter is in SOL, not
 * lamports (unlike XRP's `ripple:` scheme, which also takes the human unit,
 * called out here because it is easy to assume lamports by analogy to the rest
 * of this module, which otherwise deals exclusively in lamports).
 */
export function buildSolPaymentUri(address: string, amount: number): string {
  const amountParam = formatSolAmount(amount).replace(/ SOL$/, '');
  return `solana:${encodeURIComponent(address)}?amount=${encodeURIComponent(amountParam)}`;
}

export function nativeSolDepositCopy(intent: NativeSolDepositIntent): NativeSolDepositCopy {
  const assetName = intent.collateralInfo?.symbol || 'SOL';
  const reserveAmount = solAmountFromLamports(intent.rentExemptLamports);
  const sendAmount = intent.collateralAmount + reserveAmount;
  const collateralAmountLabel = formatSolAmount(intent.collateralAmount);
  const reserveAmountLabel = formatSolAmount(reserveAmount);
  return {
    assetName,
    sendAmount,
    reserveAmount,
    sendAmountLabel: formatSolAmount(sendAmount),
    collateralAmountLabel,
    reserveAmountLabel,
    borrowAmountLabel: `${intent.icusdAmount.toFixed(2)} icUSD`,
    reserveExplanation: `${collateralAmountLabel} collateral + ${reserveAmountLabel} rent-exempt reserve. The reserve keeps this Solana address alive and stays locked there; your vault stays open so you do not pay it again.`,
  };
}

export function nativeSolModalTitle(phase: NativeSolBorrowPhase, hasDepositAddress: boolean): string {
  if (phase === 'error' && !hasDepositAddress) {
    return 'Could not prepare SOL address';
  }
  if (phase === 'opening' || !hasDepositAddress) {
    return 'Approve in OISY to generate your SOL address';
  }
  switch (phase) {
    case 'ready_to_borrow':
      return 'SOL received, approve your borrow';
    case 'borrowing':
      return 'Minting your icUSD';
    case 'borrow_failed':
      return 'SOL received, borrow not finished';
    default:
      return 'Send SOL to open your vault';
  }
}

export function nativeSolModalStatusLabel(phase: NativeSolBorrowPhase): string {
  switch (phase) {
    case 'opening':
      return 'Approve in OISY';
    case 'awaiting':
      return 'Awaiting deposit';
    case 'confirming':
      return 'Checking Solana';
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

export function nativeSolModalOpeningCopy(): string {
  return 'Approve open_sol_vault in OISY. We will show your SOL deposit address and QR code after approval.';
}

export function nativeSolModalShouldRender(
  phase: NativeSolBorrowPhase,
  hasDepositAddress: boolean
): boolean {
  return phase !== 'opening' || hasDepositAddress;
}

export function nativeSolModalPrimaryActionLabel(
  phase: NativeSolBorrowPhase,
  hasDepositAddress: boolean,
  borrowAmountLabel?: string
): string | null {
  if (!hasDepositAddress) return null;

  switch (phase) {
    case 'awaiting':
      return "I've sent the SOL";
    case 'confirming':
      return 'Checking deposit...';
    // The borrow is a SECOND wallet approval and must come from its own click;
    // see nativeSolBorrowSeparateApprovalCopy(). Both the ready and failed
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
 * inside a browser user-gesture, and the confirm call's Solana verification
 * round-trip burns that gesture window, so an auto-borrow chained onto the
 * confirm always dies with "Signer window should not be opened outside of click
 * handler". Mirrors the native-XRP flow exactly (nativeXrpBorrowFlow.ts).
 */
export function nativeSolBorrowSeparateApprovalCopy(borrowAmountLabel: string): string {
  return `Your SOL is in the vault. Borrowing ${borrowAmountLabel} is a separate wallet approval, so approve it to finish.`;
}

/** Reassurance that closing now is safe, and where to pick the borrow back up. */
export function nativeSolBorrowLaterCopy(vaultId: number): string {
  return `Nothing is at risk if you stop here, your SOL is already collateral in vault #${vaultId}. You can borrow against it any time from the Vaults page.`;
}

const SIGNER_GESTURE_HINT = 'signer window';

/**
 * Turn a raw wallet/signer error into copy a user can act on. The signer-window
 * error in particular is meaningless to a user and, in this flow, always means
 * "the borrow needs its own click" rather than a real failure.
 */
export function nativeSolBorrowErrorCopy(rawError: string | undefined, borrowAmountLabel: string): string {
  if (rawError && rawError.toLowerCase().includes(SIGNER_GESTURE_HINT)) {
    return `Your wallet needs a fresh approval for the borrow. Tap Borrow ${borrowAmountLabel} to open it.`;
  }
  return rawError ?? 'Deposit confirmed, but borrowing failed.';
}

export function nativeSolKeepOpenCloseCopy(): string {
  return 'The SOL rent-exempt reserve stays locked on Solana, so the vault stays open. You do not need to pay that reserve again, and the vault will be ready when you want to use this SOL address later.';
}
