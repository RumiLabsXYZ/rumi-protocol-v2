import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const servicePath = resolve(__dirname, 'stabilityPoolService.ts');

describe('StabilityPoolService native XRP actor routing', () => {
  it('does not open the Oisy signer for passive pending XRP payout refreshes', () => {
    const source = readFileSync(servicePath, 'utf8');

    expect(source).toContain('async getMyNativeXrpPayouts(options: { allowSigner?: boolean } = {})');
    expect(source).toContain('async ackNativeXrpPayoutSettled');

    const readMethod = source.slice(
      source.indexOf('async getMyNativeXrpPayouts(options: { allowSigner?: boolean } = {})'),
      source.indexOf('async ackNativeXrpPayoutSettled')
    );
    const ackMethod = source.slice(source.indexOf('async ackNativeXrpPayoutSettled'));

    expect(readMethod).toContain('if (isOisyWallet() && !options.allowSigner)');
    expect(readMethod).toContain('return [];');
    expect(readMethod).toContain('this.getMutationActor()');
    expect(ackMethod).toContain('this.getMutationActor()');
  });
});
