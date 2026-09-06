import { describe, it, expect } from 'vitest';
import { getAllDelegationTargets } from './pnp';
import { CANISTER_IDS, CONFIG } from '../config';

// getAllDelegationTargets() feeds the scoped PNP delegation `targets` and
// the Plug `whitelist` in initializePNP(). A
// canister missing from this list cannot be called by a delegation-based
// wallet, so every swap venue (including the ICPswap pools) must appear
// here.
describe('getAllDelegationTargets', () => {
  it('includes the core protocol canisters', () => {
    const targets = getAllDelegationTargets();
    expect(targets).toContain(CONFIG.currentCanisterId);
    expect(targets).toContain(CONFIG.currentIcpLedgerId);
    expect(targets).toContain(CONFIG.currentIcusdLedgerId);
    expect(targets).toContain(CANISTER_IDS.CKUSDT_LEDGER);
    expect(targets).toContain(CANISTER_IDS.CKUSDC_LEDGER);
    expect(targets).toContain(CANISTER_IDS.STABILITY_POOL);
    expect(targets).toContain(CANISTER_IDS.THREEPOOL);
    expect(targets).toContain(CANISTER_IDS.RUMI_AMM);
  });

  it('includes all five ICPswap pool canisters used as swap venues', () => {
    const targets = getAllDelegationTargets();
    expect(targets).toContain(CANISTER_IDS.ICPSWAP_3USD_ICP_POOL);
    expect(targets).toContain(CANISTER_IDS.ICPSWAP_ICUSD_ICP_POOL);
    expect(targets).toContain(CANISTER_IDS.ICPSWAP_CKUSDT_ICUSD_POOL);
    expect(targets).toContain(CANISTER_IDS.ICPSWAP_ICUSD_CKUSDC_POOL);
    expect(targets).toContain(CANISTER_IDS.ICPSWAP_CKUSDT_CKUSDC_POOL);
  });

  it('has no duplicate or empty entries', () => {
    const targets = getAllDelegationTargets();
    expect(targets.every((id) => typeof id === 'string' && id.length > 0)).toBe(true);
    expect(new Set(targets).size).toBe(targets.length);
  });
});
