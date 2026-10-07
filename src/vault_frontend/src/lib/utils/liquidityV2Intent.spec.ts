import { describe, expect, it } from 'vitest';
import type { LiquidityV2StatusView } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
import { formatLiquidityAmountRaw, liquidityV2ActionLockName, liquidityV2ClaimIdentityMatches, liquidityV2Disposition, liquidityV2IntentKey, liquidityV2MayAdoptClaimAmount, liquidityV2StatusHasOwner, liquidityV2StatusMatchesIntent, parseLiquidityAmountRaw, parseLiquidityV2Intent, type LiquidityV2Intent } from './liquidityV2Intent';

const intent: LiquidityV2Intent = { version: 1, owner: 'owner-a', network: 'host|backend', ledgerPrincipal: 'icusd-ledger', requestId: '9', operation: 'Provide', amountRaw: '125000000', approvalAttempted: false, backendDispatchAttempted: false };
function status(phase: LiquidityV2StatusView['phase'], receipt: [] | [bigint] = []): LiquidityV2StatusView {
  return { owner: { toText: () => 'owner-a' } as LiquidityV2StatusView['owner'], request_id: 9n,
    kind: { Provide: null }, amount_raw: 125_000_000n,
    ledger: { toText: () => 'icusd-ledger' } as LiquidityV2StatusView['ledger'], phase,
    result_block_index: receipt, candidate_block_index: [], had_ambiguous_attempt: false,
    last_error: [], created_at_ns: 1n };
}

describe('owner-journaled liquidity V2 intents', () => {
  it('isolates each owner/network and serializes every liquidity action under one owner lock', () => {
    expect(liquidityV2IntentKey('owner-a', 'host')).not.toBe(liquidityV2IntentKey('owner-b', 'host'));
    expect(liquidityV2IntentKey('owner-a', 'host')).not.toBe(liquidityV2IntentKey('owner-a', 'other-host'));
    const sharedLock = liquidityV2ActionLockName('owner-a', 'mainnet');
    expect(sharedLock).toBe('rumi_liquidity_action_mainnet_owner-a');
    // Operation is deliberately absent from the lock name, so Provide, Withdraw,
    // and ClaimReturns for this owner cannot race each other across tabs.
  });

  it('accepts only exact owner, request, kind, amount, and ledger matches', () => {
    const row = status({ Pending: null });
    expect(liquidityV2StatusMatchesIntent(row, intent)).toBe(true);
    expect(liquidityV2StatusMatchesIntent({ ...row, ledger: { toText: () => 'new-ledger' } as LiquidityV2StatusView['ledger'] }, intent)).toBe(false);
    expect(liquidityV2StatusMatchesIntent({ ...row, request_id: 10n }, intent)).toBe(false);
    expect(liquidityV2StatusMatchesIntent({ ...row, amount_raw: 1n }, intent)).toBe(false);
    expect(liquidityV2StatusMatchesIntent({ ...row, owner: { toText: () => 'owner-b' } as LiquidityV2StatusView['owner'] }, intent)).toBe(false);
    expect(liquidityV2StatusMatchesIntent({ ...row, kind: { Withdraw: null } }, intent)).toBe(false);
  });

  it('allows an unrelated latest result on its historical ledger while retaining owner binding', () => {
    const historicalLatest = { ...status({ Complete: null }, [6n]), request_id: 8n,
      ledger: { toText: () => 'retired-icusd-ledger' } as LiquidityV2StatusView['ledger'] };
    expect(liquidityV2StatusHasOwner(historicalLatest, 'owner-a')).toBe(true);
    expect(liquidityV2StatusMatchesIntent(historicalLatest, intent)).toBe(false);
    expect(liquidityV2StatusHasOwner(historicalLatest, 'owner-b')).toBe(false);
  });

  it('uses claim request identity without inventing an amount argument the endpoint does not accept', () => {
    const claim = { ...intent, operation: 'ClaimReturns' as const, ledgerPrincipal: 'icp-ledger', amountRaw: '200000000' };
    const claimRow = { ...status({ Pending: null }), kind: { ClaimReturns: null }, amount_raw: 175_000_000n,
      ledger: { toText: () => 'icp-ledger' } as LiquidityV2StatusView['ledger'] };
    expect(liquidityV2StatusMatchesIntent(claimRow, claim)).toBe(false);
    expect(liquidityV2ClaimIdentityMatches(claimRow, claim)).toBe(true);
    expect(liquidityV2MayAdoptClaimAmount(claim, false, false, 9n)).toBe(true);
    expect(liquidityV2MayAdoptClaimAmount({ ...claim, backendDispatchAttempted: true }, false, false, 9n)).toBe(false);
    expect(liquidityV2MayAdoptClaimAmount(claim, false, true, 9n)).toBe(false);
  });

  it('retains exact intent arguments and classifies receipt, rejection, pending, and held precisely', () => {
    expect(parseLiquidityV2Intent(JSON.stringify(intent))).toEqual(intent);
    const oldIntent = { ...intent } as Partial<LiquidityV2Intent>;
    delete oldIntent.ledgerPrincipal;
    expect(parseLiquidityV2Intent(JSON.stringify(oldIntent))?.ledgerPrincipal).toBeNull();
    expect(parseLiquidityV2Intent(JSON.stringify({ ...intent, requestId: '0' }))).toBeNull();
    expect(liquidityV2Disposition(status({ Complete: null }, [7n]))).toBe('complete');
    expect(liquidityV2Disposition(status({ Complete: null }))).toBe('pending');
    expect(liquidityV2Disposition(status({ Rejected: null }))).toBe('rejected');
    expect(liquidityV2Disposition(status({ Pending: null }))).toBe('pending');
    expect(liquidityV2Disposition(status({ Held: null }))).toBe('pending');
  });

  it('parses and restores 8-decimal wire amounts without floating point', () => {
    expect(parseLiquidityAmountRaw('1.23456789')).toBe(123_456_789n);
    expect(formatLiquidityAmountRaw(123_456_789n)).toBe('1.23456789');
    expect(parseLiquidityAmountRaw('1.000000001')).toBeNull();
    expect(parseLiquidityAmountRaw('184467440737.09551616')).toBeNull();
  });
});
