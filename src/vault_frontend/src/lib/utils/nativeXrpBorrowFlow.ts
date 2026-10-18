import type { CollateralInfo } from '$lib/services/types';

export const XRP_DROPS_PER_XRP = 1_000_000;

/**
 * Pre-open estimate of the XRPL base reserve, in XRP.
 *
 * The AUTHORITATIVE value is whatever `open_xrp_vault` returns in
 * `reserveBaseDrops` (the backend reads it live from rippled `server_state`), but
 * the borrow form has to size the collateral/CR/max-borrow math BEFORE the vault
 * is opened, when no reserve is known yet. The XRPL base reserve is a
 * network-wide parameter currently set to 1 XRP by validator vote.
 *
 * If the network ever raises it, this under-deducts on the form only: the modal
 * always shows the real reserve, and the backend independently re-checks the
 * collateral ratio, so a stale value here fails safe (a borrow gets rejected)
 * rather than over-crediting anyone.
 */
export const XRPL_BASE_RESERVE_XRP = 1;

/**
 * Collateral actually credited to the vault when the user sends `sendAmount`.
 *
 * The user names the amount they will SEND; the XRPL base reserve comes out of
 * that amount to activate the custody account, and `confirm_xrp_deposit` credits
 * the remainder (`balance - reserve_base`). Never negative.
 */
export function xrpCreditedCollateral(sendAmount: number, reserveAmount: number): number {
  return Math.max(0, sendAmount - reserveAmount);
}

export interface NativeXrpDepositIntent {
  /** What the user said they would SEND (the reserve comes OUT of this). */
  collateralAmount: number;
  icusdAmount: number;
  reserveBaseDrops: bigint | number;
  collateralInfo?: Pick<CollateralInfo, 'symbol' | 'custodyKind'>;
}

export interface NativeXrpDepositCopy {
  assetName: string;
  /** Exactly what the user asked to send — never their amount plus a surprise. */
  sendAmount: number;
  reserveAmount: number;
  /** What actually lands as collateral: sendAmount minus the XRPL base reserve. */
  creditedAmount: number;
  /** Exact integer drops (null if the typed amount is not exactly representable). */
  sendDrops: bigint | null;
  reserveDrops: bigint;
  creditedDrops: bigint | null;
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

/** XRPL maximum supply: 100 billion XRP. */
const MAX_XRP_DROPS = 100_000_000_000n * BigInt(XRP_DROPS_PER_XRP);
const XRP_DECIMAL_TEXT = /^(\d+)(?:\.(\d{1,6}))?$/;

/**
 * Parse decimal XRP text to integer drops using string/BigInt math only.
 * Accepts `digits[.digits{1,6}]`; rejects seven or more decimals, signs,
 * exponents, empty and garbage input. Never goes through Number.
 */
export function tryParseXrpToDrops(text: string): bigint | null {
  if (typeof text !== 'string') return null;
  const m = XRP_DECIMAL_TEXT.exec(text.trim());
  if (!m) return null;
  const whole = BigInt(m[1]);
  const frac = BigInt((m[2] ?? '').padEnd(6, '0') || '0');
  const drops = whole * BigInt(XRP_DROPS_PER_XRP) + frac;
  return drops > MAX_XRP_DROPS ? null : drops;
}

export function parseXrpToDrops(text: string): bigint {
  const drops = tryParseXrpToDrops(text);
  if (drops === null) {
    throw new Error('Enter an XRP amount with at most 6 decimal places.');
  }
  return drops;
}

/** Exact decimal text for integer drops (no trailing zeros, no float math). */
export function formatDropsAsXrp(drops: bigint): string {
  const negative = drops < 0n;
  const abs = negative ? -drops : drops;
  const whole = abs / BigInt(XRP_DROPS_PER_XRP);
  const frac = (abs % BigInt(XRP_DROPS_PER_XRP)).toString().padStart(6, '0').replace(/0+$/, '');
  return `${negative ? '-' : ''}${whole}${frac ? `.${frac}` : ''}`;
}

/**
 * Convert a JS number that originated from user-typed decimal text into exact
 * drops. The shortest round-trip repr of such a number is the typed text, so
 * this only succeeds when that text has at most 6 decimals; any other float
 * (for example 0.1 + 0.2) is refused rather than rounded.
 */
export function xrpNumberToDrops(amount: number): bigint | null {
  if (!Number.isFinite(amount) || amount < 0) return null;
  const text = String(amount);
  if (/e/i.test(text)) return null;
  return tryParseXrpToDrops(text);
}

export function formatXrpAmount(amount: number): string {
  const rounded = amount.toFixed(6).replace(/\.?0+$/, '');
  return `${rounded} XRP`;
}

export function xrpAmountFromDrops(drops: bigint | number): number {
  return Number(drops) / XRP_DROPS_PER_XRP;
}

/**
 * `ripple:` payment URI. The amount is formatted from integer drops, exactly.
 * A number is accepted only when it is exactly representable in drops;
 * otherwise this returns '' so no inexact QR/URI is ever produced.
 */
export function buildXrpPaymentUri(address: string, amount: number | bigint): string {
  const drops = typeof amount === 'bigint' ? amount : xrpNumberToDrops(amount);
  if (drops === null || drops < 0n) return '';
  return `ripple:${encodeURIComponent(address)}?amount=${encodeURIComponent(formatDropsAsXrp(drops))}`;
}

export function nativeXrpDepositCopy(intent: NativeXrpDepositIntent): NativeXrpDepositCopy {
  const assetName = intent.collateralInfo?.symbol || 'XRP';
  const reserveAmount = xrpAmountFromDrops(intent.reserveBaseDrops);
  // The user sends EXACTLY what they asked to send. The XRPL base reserve is
  // taken out of that amount rather than added on top, so the deposit
  // instruction never differs from the number they typed.
  const sendAmount = intent.collateralAmount;
  const creditedAmount = xrpCreditedCollateral(sendAmount, reserveAmount);
  const sendDrops = xrpNumberToDrops(sendAmount);
  const reserveDrops = BigInt(intent.reserveBaseDrops);
  const creditedDrops = sendDrops === null ? null : sendDrops > reserveDrops ? sendDrops - reserveDrops : 0n;
  const exact = (drops: bigint | null, fallback: number) =>
    drops === null ? formatXrpAmount(fallback) : `${formatDropsAsXrp(drops)} XRP`;
  const collateralAmountLabel = exact(creditedDrops, creditedAmount);
  const reserveAmountLabel = exact(reserveDrops, reserveAmount);
  const sendAmountLabel = exact(sendDrops, sendAmount);
  return {
    sendDrops,
    reserveDrops,
    creditedDrops,
    assetName,
    sendAmount,
    reserveAmount,
    creditedAmount,
    sendAmountLabel,
    collateralAmountLabel,
    reserveAmountLabel,
    borrowAmountLabel: `${intent.icusdAmount.toFixed(2)} icUSD`,
    reserveExplanation: `Of the ${sendAmountLabel} you send, ${reserveAmountLabel} is the XRPL account reserve and ${collateralAmountLabel} becomes your collateral. The reserve activates this XRP address and stays locked there; your vault stays open so you do not pay it again.`,
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
