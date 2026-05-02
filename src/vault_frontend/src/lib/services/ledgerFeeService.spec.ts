import { describe, it, expect, vi, beforeEach } from 'vitest';

// ──────────────────────────────────────────────────────────────
// Hoisted mocks for @dfinity/agent so we can swap icrc1_fee()
// behaviour per-test without rebuilding the module under test.
// ──────────────────────────────────────────────────────────────

const mocks = vi.hoisted(() => ({
  icrc1Fee: vi.fn(),
  fetchRootKey: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('@dfinity/agent', async () => {
  const actual = await vi.importActual<typeof import('@dfinity/agent')>('@dfinity/agent');
  return {
    ...actual,
    Actor: {
      ...actual.Actor,
      createActor: vi.fn(() => ({ icrc1_fee: mocks.icrc1Fee })),
    },
    HttpAgent: vi.fn(() => ({
      fetchRootKey: mocks.fetchRootKey,
    })),
    AnonymousIdentity: vi.fn(() => ({})),
  };
});

vi.mock('../config', () => ({
  CONFIG: { host: 'https://icp0.io', isLocal: false },
}));

vi.mock('../idls/ledger.idl.js', () => ({
  ICRC1_IDL: {},
}));

import { fetchLedgerFee, getCachedLedgerFee, _clearLedgerFeeCache } from './ledgerFeeService';

describe('ledgerFeeService', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    _clearLedgerFeeCache();
  });

  it('returns the live icrc1_fee on first call', async () => {
    mocks.icrc1Fee.mockResolvedValueOnce(20_000n);
    const fee = await fetchLedgerFee({ ledgerId: 'aaaaa-aa', decimals: 8 });
    expect(fee).toBe(20_000n);
    expect(mocks.icrc1Fee).toHaveBeenCalledTimes(1);
  });

  it('caches the fee — second call within TTL does not re-query', async () => {
    mocks.icrc1Fee.mockResolvedValue(15_000n);
    await fetchLedgerFee({ ledgerId: 'bbbbb-bb', decimals: 6 });
    await fetchLedgerFee({ ledgerId: 'bbbbb-bb', decimals: 6 });
    expect(mocks.icrc1Fee).toHaveBeenCalledTimes(1);
  });

  it('falls back on query error, warns to console, and returns the 8-decimal fallback', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    mocks.icrc1Fee.mockRejectedValueOnce(new Error('network'));
    const fee = await fetchLedgerFee({ ledgerId: 'ccccc-cc', decimals: 8 });
    expect(fee).toBe(100_000n);
    expect(warnSpy).toHaveBeenCalled();
    warnSpy.mockRestore();
  });

  it('falls back to ICP fee (10_000) when symbol is ICP and the query fails', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    mocks.icrc1Fee.mockRejectedValueOnce(new Error('network'));
    const fee = await fetchLedgerFee({ ledgerId: 'icp-led', symbol: 'ICP', decimals: 8 });
    expect(fee).toBe(10_000n);
    warnSpy.mockRestore();
  });

  it('falls back to the 6-decimal value (10_000) when query fails for a stablecoin', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    mocks.icrc1Fee.mockRejectedValueOnce(new Error('offline'));
    const fee = await fetchLedgerFee({ ledgerId: 'ckusdt-led', decimals: 6 });
    expect(fee).toBe(10_000n);
    warnSpy.mockRestore();
  });

  it('does NOT cache fallback results so a later success can overwrite', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    mocks.icrc1Fee.mockRejectedValueOnce(new Error('first call fails'));
    mocks.icrc1Fee.mockResolvedValueOnce(42_000n);

    const first = await fetchLedgerFee({ ledgerId: 'eeeee-ee', decimals: 8 });
    expect(first).toBe(100_000n); // fallback

    const second = await fetchLedgerFee({ ledgerId: 'eeeee-ee', decimals: 8 });
    expect(second).toBe(42_000n); // live value, NOT the fallback
    expect(mocks.icrc1Fee).toHaveBeenCalledTimes(2);
    warnSpy.mockRestore();
  });

  it('getCachedLedgerFee returns cached value if present', async () => {
    mocks.icrc1Fee.mockResolvedValueOnce(25_000n);
    await fetchLedgerFee({ ledgerId: 'ddddd-dd', decimals: 6 });
    expect(getCachedLedgerFee({ ledgerId: 'ddddd-dd', decimals: 6 })).toBe(25_000n);
  });

  it('getCachedLedgerFee returns the 8-decimal fallback when nothing is cached', () => {
    expect(getCachedLedgerFee({ ledgerId: 'never-seen', decimals: 8 })).toBe(100_000n);
  });

  it('getCachedLedgerFee returns the ICP fallback when symbol is ICP and nothing is cached', () => {
    expect(getCachedLedgerFee({ ledgerId: 'icp-cold', symbol: 'ICP', decimals: 8 })).toBe(10_000n);
  });
});
