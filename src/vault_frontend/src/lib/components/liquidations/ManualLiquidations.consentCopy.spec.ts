import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const here = path.dirname(fileURLToPath(import.meta.url));

describe('manual liquidation approval status copy', () => {
  it('states approval succeeded without claiming liquidation was submitted', () => {
    const source = readFileSync(path.resolve(here, 'ManualLiquidations.svelte'), 'utf8');
    expect(source).toContain('icUSD approval succeeded; liquidation was not submitted. Click Liquidate to review and submit it.');
    expect(source).not.toContain('Approved! Click Liquidate again to complete.');
  });
});
