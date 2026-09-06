import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const servicePath = resolve(__dirname, 'stabilityPoolService.ts');

describe('StabilityPoolService native SOL actor routing', () => {
  it('does not open the Oisy signer for passive pending SOL payout refreshes', () => {
    const source = readFileSync(servicePath, 'utf8');

    expect(source).toContain('async getMyNativeSolPayouts(options: { allowSigner?: boolean } = {})');
    expect(source).toContain('async ackNativeSolPayoutSettled');
    expect(source).toContain('async optInNativeSolCollateral');

    const readMethod = source.slice(
      source.indexOf('async getMyNativeSolPayouts(options: { allowSigner?: boolean } = {})'),
      source.indexOf('async ackNativeSolPayoutSettled')
    );
    const ackMethod = source.slice(source.indexOf('async ackNativeSolPayoutSettled'));

    expect(readMethod).toContain('if (isOisyWallet() && !options.allowSigner)');
    expect(readMethod).toContain('return [];');
    expect(readMethod).toContain('this.getMutationActor()');
    expect(ackMethod).toContain('this.getMutationActor()');
  });

  it('opts in through the tagless optInNativeSolCollateralUsingActor helper (no _with_tag variant)', () => {
    const source = readFileSync(servicePath, 'utf8');
    const optInMethod = source.slice(
      source.indexOf('async optInNativeSolCollateral(collateralType: Principal, payoutAddress: string)'),
      source.indexOf('async getMyNativeSolPayouts')
    );

    expect(optInMethod).toContain('optInNativeSolCollateralUsingActor(actor, collateralType, payoutAddress');
  });
});
