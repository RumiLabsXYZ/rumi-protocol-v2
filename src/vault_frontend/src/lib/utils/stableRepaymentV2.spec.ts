import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import type { StableRepaymentV2StatusView } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
import { stableRepaymentV2Outcome, stableRepaymentV2RequiredAllowance, stableRepaymentV2TransportOutcome, unwrapStableRepaymentV2Status } from './stableRepaymentV2';

function status(phase: StableRepaymentV2StatusView['phase'], hasReceipt = false): StableRepaymentV2StatusView {
  return {
    request_id: 3n, owner: {} as StableRepaymentV2StatusView['owner'], ledger: {} as StableRepaymentV2StatusView['ledger'],
    vault_id: 8n, requested_amount_e8: 120_000_000n, principal_pull_e6: 1_200_000n,
    token_type: { CKUSDC: null }, phase, result: hasReceipt ? [{} as NonNullable<StableRepaymentV2StatusView['result'][0]>] : [],
    last_error: [], tuple: [], had_ambiguous_attempt: false, effective_debt_reduction_e8: 0n,
    candidate_block_index: [], surcharge_e6: 0n,
  };
}

describe('stable repayment V2 status contract', () => {
  it('unwraps the generated direct Candid option', () => {
    expect(unwrapStableRepaymentV2Status([])).toBeNull();
    const row = status({ PendingPull: null });
    expect(unwrapStableRepaymentV2Status([row])).toBe(row);
  });

  it('settles only receipt-bearing Complete and classifies Rejected separately', () => {
    expect(stableRepaymentV2Outcome(status({ Complete: null }, true))).toBe('complete');
    expect(stableRepaymentV2Outcome(status({ Complete: null }, false))).toBe('pending');
    expect(stableRepaymentV2Outcome(status({ Rejected: null }))).toBe('rejected');
    expect(stableRepaymentV2Outcome(status({ PendingPull: null }))).toBe('pending');
    expect(stableRepaymentV2Outcome(status({ HeldPull: null }))).toBe('pending');
  });

  it('does not treat an Ok(HeldPull) response as settlement', () => {
    expect(stableRepaymentV2TransportOutcome(status({ HeldPull: null }))).toBe('ambiguous_transport');
    expect(stableRepaymentV2TransportOutcome(status({ PendingPull: null }))).toBe('ambiguous_transport');
    expect(stableRepaymentV2TransportOutcome(status({ Complete: null }, true))).toBe('dispatched_ok');
    expect(stableRepaymentV2TransportOutcome(status({ Rejected: null }))).toBe('dispatched_err');
  });

  it('keeps approval bounded to the requested principal plus surcharge and ledger fees', () => {
    expect(stableRepaymentV2RequiredAllowance(1_000_000n, 0.01)).toBe(1_030_000n);
    expect(stableRepaymentV2RequiredAllowance(1_000_000n, 0.01)).toBeLessThan(1_000_000_000_000_000n);
    const api = readFileSync(path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../services/protocol/apiClient.ts'), 'utf8');
    const start = api.indexOf('static async repayStableV2Bound(');
    const end = api.indexOf('static async repayToVaultWithStable(', start);
    const implementation = api.slice(start, end);
    expect(implementation).toContain('amount: requiredAllowance');
    expect(implementation).not.toContain('1_000_000_000_000_000n');
  });
});
