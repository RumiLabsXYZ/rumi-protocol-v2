import { describe, it, expect, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

// walletOperations pulls the wallet store + permission manager at import
// time; neutralise them so the spec only exercises the expiry helper.
vi.mock('../../stores/wallet', () => ({ walletStore: {} }));
vi.mock('../PermissionManager', () => ({ permissionManager: {} }));

import { largeApprovalExpiry } from './walletOperations';

const here = path.dirname(fileURLToPath(import.meta.url));

const NS_PER_MS = 1_000_000n;
const THIRTY_DAYS_MS = 30n * 24n * 60n * 60n * 1_000n;

describe('largeApprovalExpiry (FE-001)', () => {
  it('FE-001: returns a single nanosecond timestamp 30 days from now', () => {
    const before = BigInt(Date.now());
    const result = largeApprovalExpiry();
    const after = BigInt(Date.now());

    expect(result).toHaveLength(1);
    const expiry = result[0];
    expect(expiry).toBeGreaterThanOrEqual((before + THIRTY_DAYS_MS) * NS_PER_MS);
    expect(expiry).toBeLessThanOrEqual((after + THIRTY_DAYS_MS) * NS_PER_MS);
  });

  it('FE-001: every inline LARGE_APPROVAL approve in apiClient.ts carries the bounded expiry', () => {
    const src = readFileSync(path.join(here, 'apiClient.ts'), 'utf8');

    // The 7 Oisy vault flows share this single-line option block. The
    // unbounded form must not reappear.
    expect(src).not.toContain('expires_at: [], expected_allowance: [], memo: [], fee: [],');
    const bounded = src.match(
      /expires_at: largeApprovalExpiry\(\), expected_allowance: \[\], memo: \[\], fee: \[\],/g,
    );
    expect(bounded).toHaveLength(7);
  });

  it('FE-001: the LARGE_APPROVAL helper paths in walletOperations.ts carry the bounded expiry', () => {
    const src = readFileSync(path.join(here, 'walletOperations.ts'), 'utf8');

    const icusd = src.slice(src.indexOf('approveIcusdTransfer'), src.indexOf('approveStableTransfer'));
    expect(icusd).toContain('expires_at: largeApprovalExpiry()');

    const stable = src.slice(src.indexOf('approveStableTransfer'));
    expect(stable).toContain('expires_at: largeApprovalExpiry()');
  });
});
