/**
 * Borrow-capacity math for the vault borrow panel.
 *
 * A vault's borrowable icUSD is the MINIMUM of three independent limits:
 *   1. collateral capacity  - how much more this vault can borrow before its
 *      collateralization ratio drops to the per-collateral minimum,
 *   2. per-collateral debt ceiling headroom  - the shared cap on aggregate debt
 *      for this collateral type, minus what every vault of that type already owes,
 *   3. global icUSD mint-cap headroom  - the protocol-wide cap on total icUSD
 *      borrowed, minus what is already borrowed everywhere.
 *
 * The backend enforces (2) and (3) atomically in `BorrowReservationGuard`
 * (src/rumi_protocol_backend/src/guard.rs) and rejects a borrow that exceeds
 * either. The panel must mirror those limits so "Max" never offers an amount the
 * backend will refuse. All amounts here are in human icUSD (already divided by
 * E8S) except where noted.
 */

export type BorrowConstraint = 'collateral' | 'debtCeiling' | 'globalCap';

export interface BorrowMaxInput {
  /** USD value of this vault's collateral (human units). */
  collateralValueUsd: number;
  /** Minimum collateralization ratio for this collateral (e.g. 1.333). */
  minCr: number;
  /** This vault's current (interest-inclusive) debt, human icUSD. */
  currentDebt: number;
  /** Per-collateral debt ceiling, human icUSD. Non-positive means "no ceiling". */
  debtCeiling: number;
  /** Aggregate debt across all vaults of this collateral type, human icUSD. */
  aggregateDebt: number;
  /** Global icUSD mint cap, human icUSD. Non-positive means "no global cap". */
  globalCap: number;
  /** Total icUSD borrowed protocol-wide, human icUSD. */
  globalBorrowed: number;
  /**
   * Safety haircut applied to the collateral capacity only, so the button never
   * overshoots when the backend oracle price differs slightly. Defaults to 0.995.
   * The ceiling / global headrooms are exact integers on the backend, so no
   * haircut is applied to them.
   */
  haircut?: number;
}

export interface BorrowMaxResult {
  /** The borrowable amount: min of the applicable limits, floored at 0. */
  maxBorrow: number;
  /** Which limit produced maxBorrow. 'collateral' unless a protocol cap is strictly tighter. */
  binding: BorrowConstraint;
  /** Collateral-capacity headroom (with haircut), human icUSD. */
  collateralHeadroom: number;
  /** Per-collateral ceiling headroom, human icUSD; null when no ceiling applies. */
  ceilingHeadroom: number | null;
  /** Global mint-cap headroom, human icUSD; null when no global cap applies. */
  globalHeadroom: number | null;
}

const DEFAULT_HAIRCUT = 0.995;

const E8S = 100_000_000;

/** Format a human icUSD amount with up to 4 decimals, no trailing zero noise. */
function formatIcusd(amount: number): string {
  return amount.toLocaleString('en-US', { maximumFractionDigits: 4 });
}

/**
 * Translate the raw `BorrowReservationGuard` rejection strings from the backend
 * (src/rumi_protocol_backend/src/guard.rs) into a friendly, human-readable
 * message. Returns null when `raw` is not one of those two messages, so callers
 * can fall through to their existing handling.
 *
 * Raw formats (A, B, C, D are e8s integers):
 *   "Borrow would exceed debt ceiling incl in-flight (A + B + C > D)"
 *   "Borrow would exceed global icUSD mint cap incl in-flight (A + B + C > D)"
 * where A = committed debt, B = in-flight reservations, C = attempted borrow,
 * D = the cap. Remaining headroom = D - A - B (clamped at 0).
 */
export function friendlyBorrowCapError(raw: string): string | null {
  if (typeof raw !== 'string') return null;

  const isCeiling = raw.includes('Borrow would exceed debt ceiling incl in-flight');
  const isGlobal = raw.includes('Borrow would exceed global icUSD mint cap incl in-flight');
  if (!isCeiling && !isGlobal) return null;

  const nums = raw.match(/\((\d+) \+ (\d+) \+ (\d+) > (\d+)\)/);
  const remaining = nums
    ? Math.max(0, (Number(nums[4]) - Number(nums[1]) - Number(nums[2])) / E8S)
    : null;

  if (isGlobal) {
    return remaining !== null
      ? `This borrow would exceed the global icUSD mint cap. Only ${formatIcusd(remaining)} icUSD can still be minted protocol-wide right now.`
      : 'This borrow would exceed the global icUSD mint cap. Try a smaller amount.';
  }
  return remaining !== null
    ? `This borrow would exceed this collateral's debt ceiling. Only ${formatIcusd(remaining)} icUSD can still be borrowed against it protocol-wide right now.`
    : "This borrow would exceed this collateral's debt ceiling. Try a smaller amount.";
}

export function computeBorrowMax(input: BorrowMaxInput): BorrowMaxResult {
  const {
    collateralValueUsd,
    minCr,
    currentDebt,
    debtCeiling,
    aggregateDebt,
    globalCap,
    globalBorrowed,
    haircut = DEFAULT_HAIRCUT,
  } = input;

  const collateralHeadroom = minCr > 0 && collateralValueUsd > 0
    ? Math.max(0, (collateralValueUsd / minCr - currentDebt) * haircut)
    : 0;

  // A non-positive ceiling / cap means the constraint is not configured (or not
  // yet loaded), so it does not bound the amount.
  const ceilingHeadroom = debtCeiling > 0
    ? Math.max(0, debtCeiling - aggregateDebt)
    : null;
  const globalHeadroom = globalCap > 0
    ? Math.max(0, globalCap - globalBorrowed)
    : null;

  // Start from collateral; only let a protocol cap take over when it is STRICTLY
  // tighter, so ties keep the neutral 'collateral' label (no scary message).
  let binding: BorrowConstraint = 'collateral';
  let maxBorrow = collateralHeadroom;

  if (ceilingHeadroom !== null && ceilingHeadroom < maxBorrow) {
    binding = 'debtCeiling';
    maxBorrow = ceilingHeadroom;
  }
  if (globalHeadroom !== null && globalHeadroom < maxBorrow) {
    binding = 'globalCap';
    maxBorrow = globalHeadroom;
  }

  return { maxBorrow, binding, collateralHeadroom, ceilingHeadroom, globalHeadroom };
}
