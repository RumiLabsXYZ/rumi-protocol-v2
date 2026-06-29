import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const servicePath = resolve(__dirname, 'stabilityPoolService.ts');

describe('StabilityPoolService native XRP actor routing', () => {
  it('uses the anonymous query actor for passive pending XRP payout reads', () => {
    const source = readFileSync(servicePath, 'utf8');

    expect(source).toContain('async getMyNativeXrpPayouts(): Promise<NativeXrpPendingPayout[]>');
    expect(source).toContain('async ackNativeXrpPayoutSettled');

    const readMethod = source.slice(
      source.indexOf('async getMyNativeXrpPayouts(): Promise<NativeXrpPendingPayout[]>'),
      source.indexOf('async ackNativeXrpPayoutSettled')
    );
    const ackMethod = source.slice(source.indexOf('async ackNativeXrpPayoutSettled'));

    expect(readMethod).toContain('this.getQueryActor()');
    expect(readMethod).not.toContain('this.getMutationActor()');
    expect(ackMethod).toContain('this.getMutationActor()');
  });
});
