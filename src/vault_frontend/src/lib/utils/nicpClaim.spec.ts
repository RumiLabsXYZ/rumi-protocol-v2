import { describe, expect, it } from 'vitest';
import { decodeNicpClaim, nicpClaimValueUsd } from './nicpClaim';

describe('protocol-controlled nICP redemption display', () => {
  it('values the fixed ICP receivable with the existing haircut', () => {
    const claim = decodeNicpClaim([{ stage: { ClaimConfirmed: null }, icp_due_e8s: 190_000_000_000n, received_icp_e8s: [], withdrawal_id: [5n] }])!;
    expect(claim).toEqual({ stage: 'ClaimConfirmed', icpAmount: 1900, withdrawalId: '5' });
    expect(nicpClaimValueUsd(claim, 2, 0.1)).toBe(3420);
    expect(nicpClaimValueUsd(claim, 1, 0.1)).toBe(1710);
  });
  it('settled collateral uses the actual receipt and missing terms fail closed', () => {
    const claim = decodeNicpClaim([{ stage: { Settled: null }, icp_due_e8s: 190_000_000_000n, received_icp_e8s: [180_000_000_000n], withdrawal_id: [5n] }])!;
    expect(nicpClaimValueUsd(claim, 2, 0.1)).toBe(3240);
    expect(nicpClaimValueUsd(claim, 2, undefined)).toBe(0);
    expect(nicpClaimValueUsd(claim, 2, -0.1)).toBe(0);
    expect(decodeNicpClaim([])).toBeUndefined();
  });
});
