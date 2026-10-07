/**
 * Pure client-side guard for the 3pool's icUSD concentration rule.
 *
 * This is a UX pre-check only. The canister re-checks the same policy while
 * holding its pool lock before it pulls any ledger funds.
 */

export const THREE_POOL_CAP_NUMERATOR = 666n;
export const THREE_POOL_CAP_DENOMINATOR = 1000n;
export const THREE_POOL_DEFAULT_DECIMALS = [8, 6, 6] as const;

export type DepositPolicyReason =
  | 'allowed'
  | 'no_amount'
  | 'cap_exceeded'
  | 'overcap_icusd_requires_stables';

export interface DepositPolicyResult {
  allowed: boolean;
  reason: DepositPolicyReason;
  /** Amount of additional stablecoin value needed, normalized to `normalizationDecimals`. */
  missingStableNormalized: bigint;
  normalizationDecimals: number;
  oldIcusdNormalized: bigint;
  oldTotalNormalized: bigint;
  newIcusdNormalized: bigint;
  newTotalNormalized: bigint;
}

function powerOfTen(exponent: number): bigint {
  if (!Number.isInteger(exponent) || exponent < 0) {
    throw new Error('Token decimals must be a non-negative integer');
  }
  return 10n ** BigInt(exponent);
}

function normalizeAmount(amount: bigint, decimals: number, targetDecimals: number): bigint {
  if (amount < 0n) throw new Error('Token amounts cannot be negative');
  if (!Number.isInteger(decimals) || decimals < 0 || decimals > targetDecimals) {
    throw new Error('Token decimals must not exceed the normalization precision');
  }
  return amount * powerOfTen(targetDecimals - decimals);
}

function ceilDiv(numerator: bigint, denominator: bigint): bigint {
  if (numerator <= 0n) return 0n;
  return (numerator + denominator - 1n) / denominator;
}

/**
 * Evaluate a proposed deposit against the nominal 666/1000 icUSD cap.
 *
 * Balances and amounts use each token's native decimals. They are normalized
 * exactly to the largest supplied decimal precision before any comparisons.
 */
export function evaluateThreePoolDepositPolicy(
  currentBalances: readonly [bigint, bigint, bigint],
  depositAmounts: readonly [bigint, bigint, bigint],
  tokenDecimals: readonly [number, number, number] = THREE_POOL_DEFAULT_DECIMALS,
): DepositPolicyResult {
  const normalizationDecimals = Math.max(...tokenDecimals);
  const old = currentBalances.map((amount, index) =>
    normalizeAmount(amount, tokenDecimals[index], normalizationDecimals),
  ) as [bigint, bigint, bigint];
  const deposit = depositAmounts.map((amount, index) =>
    normalizeAmount(amount, tokenDecimals[index], normalizationDecimals),
  ) as [bigint, bigint, bigint];

  const oldIcusdNormalized = old[0];
  const oldTotalNormalized = old[0] + old[1] + old[2];
  const newIcusdNormalized = old[0] + deposit[0];
  const newTotalNormalized = oldTotalNormalized + deposit[0] + deposit[1] + deposit[2];
  const missingStableForCap = (): bigint => {
    const numerator =
      THREE_POOL_CAP_DENOMINATOR * newIcusdNormalized -
      THREE_POOL_CAP_NUMERATOR * newTotalNormalized;
    return ceilDiv(numerator, THREE_POOL_CAP_NUMERATOR);
  };
  const base = {
    normalizationDecimals,
    oldIcusdNormalized,
    oldTotalNormalized,
    newIcusdNormalized,
    newTotalNormalized,
  };

  if (deposit[0] === 0n && deposit[1] === 0n && deposit[2] === 0n) {
    return { ...base, allowed: false, reason: 'no_amount', missingStableNormalized: 0n };
  }

  const oldAtOrBelowCap =
    oldTotalNormalized === 0n ||
    oldIcusdNormalized * THREE_POOL_CAP_DENOMINATOR <=
      oldTotalNormalized * THREE_POOL_CAP_NUMERATOR;

  if (!oldAtOrBelowCap && deposit[0] > 0n) {
    const stableDeposit = deposit[1] + deposit[2];
    // A corrective deposit must itself be no more than 66.6% icUSD. The
    // resulting stable requirement is just over 0.5015 stable per icUSD
    // (for example, 200 icUSD needs 100.300301 stable value at e8).
    const requiredStableDeposit = ceilDiv(
      (THREE_POOL_CAP_DENOMINATOR - THREE_POOL_CAP_NUMERATOR) * deposit[0],
      THREE_POOL_CAP_NUMERATOR,
    );
    if (stableDeposit < requiredStableDeposit) {
      return {
        ...base,
        allowed: false,
        reason: 'overcap_icusd_requires_stables',
        missingStableNormalized: requiredStableDeposit - stableDeposit,
      };
    }
    return { ...base, allowed: true, reason: 'allowed', missingStableNormalized: 0n };
  }

  const resultingShareWithinCap =
    newTotalNormalized > 0n &&
    newIcusdNormalized * THREE_POOL_CAP_DENOMINATOR <=
      newTotalNormalized * THREE_POOL_CAP_NUMERATOR;
  if (!oldAtOrBelowCap && deposit[0] === 0n) {
    // Stable-only deposits are always valid corrective deposits, even when
    // they do not restore the whole pool below the cap in one operation.
    return { ...base, allowed: true, reason: 'allowed', missingStableNormalized: 0n };
  }

  if (!resultingShareWithinCap) {
    return {
      ...base,
      allowed: false,
      reason: 'cap_exceeded',
      missingStableNormalized: missingStableForCap(),
    };
  }

  return { ...base, allowed: true, reason: 'allowed', missingStableNormalized: 0n };
}
