import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { repaymentV2Disposition, repaymentV2IntentKey, parseRepaymentV2Intent, repaymentV2StatusMatchesIntent, repaymentV2TransportOutcome } from './repaymentV2Intent';
import type { RepaymentV2StatusView } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
import { Principal } from '@dfinity/principal';

const apiClientPath = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../services/protocol/apiClient.ts');
const protocolManagerPath = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../services/ProtocolManager.ts');
const protocolServicePath = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../services/protocol.ts');
const vaultCardPath = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../components/vault/VaultCard.svelte');

const intent = {
  version: 1 as const, owner: '2vxsx-fae', network: 'mainnet|protocol', requestId: '5',
  vaultId: '12', requestedAmountRaw: '12345678', closeAfterRepay: true, approvalAttempted: true, backendDispatchAttempted: true,
};

function status(phase: RepaymentV2StatusView['phase']): RepaymentV2StatusView {
  return {
    owner: Principal.fromText(intent.owner), request_id: 5n, vault_id: 12n,
    requested_amount_raw: 12345678n, effective_amount_raw: 12345678n, close_after_repay: true,
    ledger: Principal.fromText('2vxsx-fae'), tuple: [], candidate_block_index: [],
    result: [{ repay_block_index: 20n, collateral_return_block_index: [21n] }],
    had_ambiguous_attempt: false, last_error: [], phase,
  };
}

describe('owner-global repayment V2 intent', () => {
  it('persists decimal Nat IDs and strictly validates saved intent', () => {
    expect(repaymentV2IntentKey(intent.owner, intent.network)).toContain(encodeURIComponent(intent.owner));
    expect(parseRepaymentV2Intent(JSON.stringify(intent))?.requestId).toBe('5');
    expect(parseRepaymentV2Intent(JSON.stringify({ ...intent, requestId: '-1' }))).toBeNull();
  });

  it('binds backend status to the exact owner, ledger, ID, vault, amount, and close mode', () => {
    const row = status({ Complete: null });
    expect(repaymentV2StatusMatchesIntent(row, intent, '2vxsx-fae')).toBe(true);
    expect(repaymentV2StatusMatchesIntent({ ...row, requested_amount_raw: 12345679n }, intent, '2vxsx-fae')).toBe(false);
    expect(repaymentV2StatusMatchesIntent(row, { ...intent, closeAfterRepay: false }, '2vxsx-fae')).toBe(false);
  });

  it('never classifies CloseNeedsAdditionalRepayment as close success', () => {
    expect(repaymentV2Disposition(status({ Complete: null }))).toBe('complete');
    expect(repaymentV2Disposition(status({ CloseNeedsAdditionalRepayment: null }))).toBe('needs_additional_repayment');
    expect(repaymentV2Disposition(status({ HeldPull: null }))).toBe('pending');
    expect(repaymentV2Disposition({ ...status({ CloseNeedsAdditionalRepayment: null }), result: [] })).toBe('pending');
  });

  it('keeps matching pending/held rows ambiguous after a lost dispatch reply', () => {
    expect(repaymentV2TransportOutcome(status({ PendingPull: null }))).toBe('ambiguous_transport');
    expect(repaymentV2TransportOutcome(status({ HeldPull: null }))).toBe('ambiguous_transport');
    expect(repaymentV2TransportOutcome(status({ Complete: null }))).toBe('dispatched_ok');
    expect(repaymentV2TransportOutcome(status({ CloseNeedsAdditionalRepayment: null }))).toBe('dispatched_ok');
    expect(repaymentV2TransportOutcome(status({ Rejected: null }))).toBe('dispatched_err');
  });

  it('does not label a matching pending/held status as a successful API outcome', () => {
    const source = readFileSync(apiClientPath, 'utf8');
    const start = source.indexOf('static async repayV2Bound(');
    const end = source.indexOf('static async repayToVault(', start);
    const method = source.slice(start, end);
    const catchBlock = method.slice(method.lastIndexOf('} catch (error) {'));
    expect(catchBlock).toContain('repaymentV2TransportOutcome(exact)');
    expect(catchBlock).toContain("if (outcome === 'dispatched_ok')");
    expect(catchBlock).toContain("return result(outcome, exact, error instanceof Error ? error.message");
    expect(catchBlock).not.toContain("matches(exact) ? 'dispatched_ok'");
  });

  it('fails legacy no-ID API and manager wrappers before any approval work', () => {
    const api = readFileSync(apiClientPath, 'utf8');
    const manager = readFileSync(protocolManagerPath, 'utf8');
    const service = readFileSync(protocolServicePath, 'utf8');
    for (const methodName of ['repayToVault', 'repayAndCloseVault']) {
      const apiStart = api.indexOf(`static async ${methodName}(`);
      const apiEnd = api.indexOf('\n}', apiStart) + 2;
      const apiMethod = api.slice(apiStart, apiEnd);
      expect(apiMethod).toContain('legacyIcusdRepaymentDisabled()');
      expect(apiMethod).not.toContain('icrc2_approve');
      const managerStart = manager.indexOf(`async ${methodName}(`);
      const managerEnd = manager.indexOf('\n  }', managerStart) + 4;
      const managerMethod = manager.slice(managerStart, managerEnd);
      expect(managerMethod).toContain(`ApiClient.${methodName}(`);
      expect(managerMethod).not.toContain('executeOperation(');
      expect(managerMethod).not.toContain('approveIcusdTransfer(');
    }
    expect(service).toContain('static partialRepayToVault = ApiClient.repayToVault;');
  });

  it('preflights exact owner status before any approval and only then dispatches V2', () => {
    const source = readFileSync(apiClientPath, 'utf8');
    const start = source.indexOf('static async repayV2Bound(');
    const end = source.indexOf('static async repayToVault(', start);
    const method = source.slice(start, end);
    expect(start).toBeGreaterThanOrEqual(0);
    expect(method.indexOf('getRepaymentV2StatusBound')).toBeLessThan(method.indexOf('beforeApprovalDispatch();'));
    expect(method.indexOf('getRepaymentV2RequestStateBound')).toBeLessThan(method.indexOf('beforeApprovalDispatch();'));
    expect(method.indexOf('if (!existingJournal)')).toBeLessThan(method.indexOf('beforeApprovalDispatch();'));
    expect(method.indexOf('beforeBackendDispatch();')).toBeLessThan(method.indexOf('await actor.repay_to_vault_v2'));
    expect(method).toContain('await actor.repay_and_close_vault_v2');
    expect(method).not.toContain('verifyRepayLanded');
    expect(method).toContain('repaymentV2TransportOutcome(response.Ok)');
    expect(method).toContain('repaymentV2TransportOutcome(exact)');
  });

  it('routes active icUSD repayment through V2 and leaves no legacy icUSD caller in VaultCard', () => {
    const source = readFileSync(vaultCardPath, 'utf8');
    expect(source).toContain('ApiClient.repayV2Bound(');
    expect(source).not.toContain('protocolManager.repayToVault(');
    expect(source).not.toContain('protocolManager.repayAndCloseVault(');
  });
});
