import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const componentPath = resolve(__dirname, 'XrpPayoutRouting.svelte');

describe('XrpPayoutRouting pending payouts', () => {
  it('renders pending payout rows and settles before acknowledgement', () => {
    const source = readFileSync(componentPath, 'utf8');

    expect(source).toContain('pendingPayouts = await stabilityPoolService.getMyNativeXrpPayouts()');
    expect(source).toContain('Claim #{claimId}');
    expect(source).toContain('payout.payout_address');
    expect(source).toContain('tag {payout.destination_tag[0]}');
    expect(source).toContain('hasPendingPayouts = pendingPayouts.length > 0');
    expect(source).toContain('userHasIcusd || isEnabled || loadingPayouts || hasPendingPayouts');
    expect(source).toContain('const claimOutstanding = await XrpVaultService.hasOutstandingClaim(claimId)');
    expect(source).toContain('Retry once it validates to clear this reminder');
    expect(source).toContain('if (!message.includes(\'not available\'))');

    const settleCall = source.indexOf('XrpVaultService.settleXrpClaim');
    const outstandingCheck = source.indexOf('XrpVaultService.hasOutstandingClaim');
    const ackCall = source.indexOf('stabilityPoolService.ackNativeXrpPayoutSettled');
    expect(settleCall).toBeGreaterThan(-1);
    expect(outstandingCheck).toBeGreaterThan(settleCall);
    expect(ackCall).toBeGreaterThan(outstandingCheck);

    const loadPendingPayouts = source.slice(
      source.indexOf('async function loadPendingPayouts()'),
      source.indexOf('$: if (!isConnected)')
    );
    const catchBlock = loadPendingPayouts.slice(
      loadPendingPayouts.indexOf('} catch (err: unknown) {'),
      loadPendingPayouts.indexOf('} finally {')
    );
    expect(catchBlock).not.toContain('pendingPayouts = []');
  });
});
