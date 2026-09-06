import { describe, it, expect } from 'vitest';
import { formatStabilityPoolEvent } from './explorerFormatters';
import { XRP_NATIVE_PRINCIPAL } from './explorerHelpers';

/**
 * `LiquidationExecuted.collateral_gained` is denominated in the collateral's
 * OWN native units, not e8s. The formatter used to hardcode 8 decimals, which
 * is correct for icUSD/BOB/EXE but renders native XRP (6-decimal drops) 100x
 * too small.
 *
 * The amounts below are the real mainnet absorb of vault #195 on 2026-08-15:
 * 1,796,552 drops seized against 1.67588756 icUSD burned.
 */
const liquidationExecuted = (collateralType: string, collateralGained: bigint) => ({
  event_type: {
    LiquidationExecuted: {
      vault_id: 195n,
      stables_consumed_e8s: 167_588_756n,
      collateral_gained: collateralGained,
      collateral_type: { toText: () => collateralType },
      success: true,
    },
  },
});

describe('formatStabilityPoolEvent — LiquidationExecuted', () => {
  it('formats native-XRP collateral with 6 decimals (drops), not 8', () => {
    const out = formatStabilityPoolEvent(liquidationExecuted(XRP_NATIVE_PRINCIPAL, 1_796_552n));

    // 1_796_552 drops = 1.796552 XRP. At a hardcoded 8 decimals this would
    // read 0.01796552 XRP.
    expect(out.summary).toContain('1.796552 XRP');
    expect(out.summary).not.toContain('0.01796552');
  });

  it('still formats 8-decimal collateral correctly', () => {
    // ckBTC ledger: 8 decimals. 1_796_552 => 0.01796552.
    const out = formatStabilityPoolEvent(
      liquidationExecuted('mxzaz-hqaaa-aaaar-qaada-cai', 1_796_552n),
    );

    expect(out.summary).toContain('0.01796552');
  });

  it('keeps stables_consumed_e8s at 8 decimals (icUSD is e8s regardless of collateral)', () => {
    const out = formatStabilityPoolEvent(liquidationExecuted(XRP_NATIVE_PRINCIPAL, 1_796_552n));

    expect(out.summary).toContain('1.67588756');
  });

  it('names the vault so an absorb is not just an anonymous notification', () => {
    const out = formatStabilityPoolEvent(liquidationExecuted(XRP_NATIVE_PRINCIPAL, 1_796_552n));

    expect(out.typeName).toBe('Liquidation Executed');
    expect(out.summary).toContain('#195');
    expect(out.fields.some((f) => f.label === 'Vault' && f.value === '#195')).toBe(true);
  });
});
