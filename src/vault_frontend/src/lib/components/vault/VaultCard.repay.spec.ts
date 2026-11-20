import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const componentPath = resolve(__dirname, './VaultCard.svelte');

describe('VaultCard repayment input contract', () => {
  it('keeps the displayed max, HTML constraints, and token precision aligned', () => {
    const source = readFileSync(componentPath, 'utf8');

    // icUSD uses e8s raw units, so a value such as 1.995 must be valid in the
    // browser input and must remain the same value selected by Max.
    expect(source).toContain('repayAmount = repayQuote.amount');
    expect(source).toContain('$: repayInputMax = repayQuote?.amount;');
    expect(source).toContain('Max: {repayInputMax}');
    expect(source).toContain('min="0" max={repayInputMax} step={repayInputStep}');
  });

  it('keeps a wallet-limited icUSD max on the ordinary repay path', () => {
    const source = readFileSync(componentPath, 'utf8');

    expect(source).toContain('computeSafeIcusdRepayMax');
    expect(source).toContain("repayTokenType === 'icUSD' && spendableE8s >= debtE8s");
  });
});
