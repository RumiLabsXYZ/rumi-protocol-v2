import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const componentPath = resolve(__dirname, 'EarnInfoCard.svelte');

describe('EarnInfoCard native XRP wiring', () => {
  it('renders the live XRP opt-in and pending payout child from EarnInfoCard', () => {
    const source = readFileSync(componentPath, 'utf8');

    expect(source).toContain("XrpPayoutRouting.svelte");
    expect(source).toContain('<XrpPayoutRouting');
  });

  it('does not present sunset BOB as a gain or liquidation preference', () => {
    const source = readFileSync(componentPath, 'utf8');

    expect(source).toContain("new Set(['PHASMA', 'BOB'])");
  });
});
