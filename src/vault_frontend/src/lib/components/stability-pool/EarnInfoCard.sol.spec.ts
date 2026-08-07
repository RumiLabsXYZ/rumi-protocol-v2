import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const componentPath = resolve(__dirname, 'EarnInfoCard.svelte');

describe('EarnInfoCard native SOL payout routing', () => {
  it('mounts SolPayoutRouting alongside XrpPayoutRouting', () => {
    const source = readFileSync(componentPath, 'utf8');

    expect(source).toContain("import SolPayoutRouting from './SolPayoutRouting.svelte'");
    expect(source).toContain('<SolPayoutRouting');
    expect(source).toContain('<XrpPayoutRouting');
  });

  it('lists SOL in the collateral display-order map', () => {
    const source = readFileSync(componentPath, 'utf8');
    const orderLine = source.slice(
      source.indexOf('const COLLATERAL_ORDER'),
      source.indexOf('\n', source.indexOf('const COLLATERAL_ORDER'))
    );

    expect(orderLine).toContain('SOL:');
    expect(orderLine.indexOf('XRP:')).toBeLessThan(orderLine.indexOf('SOL:'));
  });
});
