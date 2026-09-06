import { describe, it, expect } from 'vitest';
import {
  collateralGainDisplayAmount,
  sumPendingXrpDrops,
  XRP_NATIVE_PRINCIPAL_TEXT,
} from './xrpPayoutHelpers';
import { SOL_NATIVE_PRINCIPAL_TEXT } from './solPayoutHelpers';

const CKBTC = 'mxzaz-hqaaa-aaaar-qaada-cai';

describe('collateralGainDisplayAmount', () => {
  it('shows pending XRP claim drops for native XRP, not the always-zero ICRC gain', () => {
    // The live shape after the 2026-08-15 absorb of vault #195: the depositor
    // is owed 1,185,343 drops via an XRPL claim, while `collateral_gains` for
    // XRP is (and always will be) 0 because XRP never lands in the pool.
    const out = collateralGainDisplayAmount(XRP_NATIVE_PRINCIPAL_TEXT, 0n, 1_185_343n);

    expect(out.amount).toBe(1_185_343n);
    expect(out.viaXrpClaims).toBe(true);
    expect(out.viaSolClaims).toBe(false);
  });

  it('leaves ICRC collateral on its real gains value', () => {
    const out = collateralGainDisplayAmount(CKBTC, 5_020_000n, 1_185_343n);

    expect(out.amount).toBe(5_020_000n);
    expect(out.viaXrpClaims).toBe(false);
    expect(out.viaSolClaims).toBe(false);
  });

  it('reports zero for native XRP when nothing is actually owed', () => {
    const out = collateralGainDisplayAmount(XRP_NATIVE_PRINCIPAL_TEXT, 0n, 0n);

    expect(out.amount).toBe(0n);
    // Still flagged as the claim rail, so the UI labels it consistently rather
    // than flipping presentation based on whether a balance happens to exist.
    expect(out.viaXrpClaims).toBe(true);
  });

  it('shows pending SOL claim lamports for native SOL, not the always-zero ICRC gain', () => {
    // Same latent bug as XRP's, fixed the same way: native SOL pays out via
    // SolClaim, never through the pool, so `collateral_gains` for SOL is
    // permanently 0.
    const out = collateralGainDisplayAmount(SOL_NATIVE_PRINCIPAL_TEXT, 0n, 0n, 2_500_000_000n);

    expect(out.amount).toBe(2_500_000_000n);
    expect(out.viaSolClaims).toBe(true);
    expect(out.viaXrpClaims).toBe(false);
  });

  it('reports zero for native SOL when nothing is actually owed', () => {
    const out = collateralGainDisplayAmount(SOL_NATIVE_PRINCIPAL_TEXT, 0n, 0n, 0n);

    expect(out.amount).toBe(0n);
    expect(out.viaSolClaims).toBe(true);
  });

  it('defaults pendingSolLamports to zero when the caller omits it', () => {
    // EarnInfoCard.svelte always passes it, but the parameter is optional so
    // existing call sites (and this test suite's XRP-only cases above) keep
    // compiling without a SOL argument.
    const out = collateralGainDisplayAmount(SOL_NATIVE_PRINCIPAL_TEXT, 0n, 0n);

    expect(out.amount).toBe(0n);
    expect(out.viaSolClaims).toBe(true);
  });
});

describe('sumPendingXrpDrops', () => {
  it('totals drops across payouts', () => {
    expect(sumPendingXrpDrops([{ drops: 1_185_343n }, { drops: 590_828n }])).toBe(1_776_171n);
  });

  it('accepts number drops without losing precision on the bigint total', () => {
    expect(sumPendingXrpDrops([{ drops: 5_092 }, { drops: 3_760 }])).toBe(8_852n);
  });

  it('is zero for empty, null, and undefined', () => {
    expect(sumPendingXrpDrops([])).toBe(0n);
    expect(sumPendingXrpDrops(null)).toBe(0n);
    expect(sumPendingXrpDrops(undefined)).toBe(0n);
  });
});
