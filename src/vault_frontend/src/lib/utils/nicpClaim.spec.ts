import { describe, expect, it } from 'vitest';
import { decodeNicpClaim, nicpClaimValueUsd } from './nicpClaim';

describe('protocol-controlled nICP redemption display', () => {
  it('values the fixed ICP receivable with the existing haircut', () => {
    const claim = decodeNicpClaim([{ stage: { ClaimConfirmed: null }, icp_due_e8s: 190_000_000_000n, received_icp_e8s: [], withdrawal_id: [5n] }])!;
    expect(claim).toEqual({ stage: 'ClaimConfirmed', icpAmount: 1900, withdrawalId: '5' });
    expect(nicpClaimValueUsd(claim, 2, 0.1)).toBe(3420);
    expect(nicpClaimValueUsd(claim, 1, 0.1)).toBe(1710);
  });
  it('shows the durable payout phase and receipt', () => {
    const base = { stage: { Settled: null }, icp_due_e8s: 100n, received_icp_e8s: [100n], withdrawal_id: [5n] };
    const pending = decodeNicpClaim([{ ...base, release: [{ block: [] }] }]);
    expect(pending?.releasePending).toBe(true);
    expect(nicpClaimValueUsd(pending!, 2, 0.1)).toBe(0);
    const done = decodeNicpClaim([{ ...base, release: [{ block: [123n] }] }]);
    expect(done?.releasePending).toBe(false);
    expect(done?.releaseBlock).toBe('123');
    expect(nicpClaimValueUsd(done!, 2, 0.1)).toBe(0);
  });
  it('settled collateral uses the actual receipt and missing terms fail closed', () => {
    const claim = decodeNicpClaim([{ stage: { Settled: null }, icp_due_e8s: 190_000_000_000n, received_icp_e8s: [180_000_000_000n], withdrawal_id: [5n] }])!;
    expect(nicpClaimValueUsd(claim, 2, 0.1)).toBe(3240);
    expect(nicpClaimValueUsd(claim, 2, undefined)).toBe(0);
    expect(nicpClaimValueUsd(claim, 2, -0.1)).toBe(0);
    expect(decodeNicpClaim([])).toBeUndefined();
  });
});


describe('automatic vault 51 collateral adoption', () => {
  it('does not value an adopted claim a second time', () => {
    const claim = decodeNicpClaim([{ stage: { Settled: null }, icp_due_e8s: 190_000_000_000n,
      received_icp_e8s: [180_000_000_000n], withdrawal_id: [1696n],
      adoption: [{ amount_e8s: 179_999_990_000n, block: [123n], completed: true }] }])!;
    expect(claim.stage).toBe('Adopted');
    expect(claim.icpAmount).toBe(1799.9999);
    expect(nicpClaimValueUsd(claim, 2, 0.1)).toBe(0);
  });
});


describe('pending adoption receipt', () => {
  it('does not retire claim backing from a marker without a receipt', () => {
    const claim = decodeNicpClaim([{ stage: { Settled: null }, icp_due_e8s: 190_000_000_000n,
      received_icp_e8s: [180_000_000_000n], adoption: [{ amount_e8s: 179_999_990_000n, block: [], completed: true }] }])!;
    expect(claim.stage).toBe('Adopting');
    expect(nicpClaimValueUsd(claim, 2, 0.1)).toBeCloseTo(3239.99982);
  });
});
