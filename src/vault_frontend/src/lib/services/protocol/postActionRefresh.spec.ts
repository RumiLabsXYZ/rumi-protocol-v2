import { describe, expect, it, vi } from 'vitest';
import { runConfirmedActionRefresh } from './postActionRefresh';

describe('confirmed financial action refresh handling', () => {
  it('reports a refresh failure separately without converting confirmed success into failure', async () => {
    const report = vi.fn();
    const refresh = vi.fn().mockRejectedValue(new Error('query unavailable'));
    const ctx = { expectedPrincipalText: 'owner-a', assertCurrent: () => true };

    await expect(runConfirmedActionRefresh('Borrow', ctx, refresh, report)).resolves.toBe(false);
    expect(refresh).toHaveBeenCalledOnce();
    expect(report).toHaveBeenCalledWith(expect.stringContaining('Borrow is confirmed'));
    expect(report).toHaveBeenCalledWith(expect.stringContaining('Do not submit the financial action again'));
  });

  it('asks for the original wallet when it changed before the status refresh', async () => {
    const report = vi.fn();
    const refresh = vi.fn();
    const ctx = { expectedPrincipalText: 'owner-a', assertCurrent: () => false };

    await expect(runConfirmedActionRefresh('Vault creation', ctx, refresh, report)).resolves.toBe(false);
    expect(refresh).not.toHaveBeenCalled();
    expect(report).toHaveBeenCalledWith(expect.stringContaining('Reconnect the wallet that submitted it'));
  });

  it('consumes a rejected post-success balance refresh and keeps the confirmed action successful', async () => {
    const report = vi.fn();
    const balanceRefresh = vi.fn().mockRejectedValue(new Error('ledger query unavailable'));
    const ctx = { expectedPrincipalText: 'owner-a', assertCurrent: () => true };
    const confirmedAction = { success: true as const };

    await expect(runConfirmedActionRefresh('Collateral deposit', ctx, async () => {
      await balanceRefresh();
    }, report)).resolves.toBe(false);

    expect(confirmedAction.success).toBe(true);
    expect(report).toHaveBeenCalledOnce();
    expect(report).toHaveBeenCalledWith(expect.stringContaining('Collateral deposit is confirmed'));
    expect(report).toHaveBeenCalledWith(expect.stringContaining('Do not submit the financial action again'));
  });
});
