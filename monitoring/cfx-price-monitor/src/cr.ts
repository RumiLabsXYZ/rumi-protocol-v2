// Port of the backend's `collateral_ratio_e4` (chains/vault.rs). Faithful to the
// on-chain integer (truncating) math + the u64::MAX cap, so the monitor computes
// the SAME true CR the canister would, at the live market price. The Rust path
// uses `saturating_mul` on u128; JS BigInt has no overflow, so the two diverge
// only above ~2^128 — magnitudes unreachable for any real collateral/price/debt
// (and the result is capped at u64::MAX either way). Within real ranges they are
// identical, which is what the test vectors pin.

/** u64::MAX — the CR returned for a debt-free vault (infinite ratio). */
export const U64_MAX = (1n << 64n) - 1n;

/**
 * collateral_ratio_e4 — returns the collateral ratio in e4 fixed point
 * (13000 == 130.00%).
 *
 * cr_e4 = (collateral_native * price_e8 / 10^native_decimals) * 10_000 / debt_e8s
 *
 * All integer division (truncating), matching the Rust u128 path, capped at
 * u64::MAX. A zero-debt vault returns U64_MAX.
 */
export function collateralRatioE4(
  collateralNative: bigint,
  nativeDecimals: number,
  priceE8: bigint,
  debtE8s: bigint,
): bigint {
  if (debtE8s === 0n) return U64_MAX;
  const nativeScale = 10n ** BigInt(nativeDecimals);
  const collateralUsdE8 = (collateralNative * priceE8) / nativeScale;
  const cr = (collateralUsdE8 * 10_000n) / debtE8s;
  return cr < U64_MAX ? cr : U64_MAX;
}
