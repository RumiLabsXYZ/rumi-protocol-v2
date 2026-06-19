// Port of the backend's `collateral_ratio_e4` (chains/vault.rs). Kept
// byte-for-byte faithful to the on-chain integer math so the monitor computes
// the SAME true CR the canister would, at the live market price.

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
