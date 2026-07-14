import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const source = readFileSync(resolve(__dirname, 'SpCoverageCard.svelte'), 'utf8');

describe('SpCoverageCard query degradation', () => {
	it('renders query failure before the genuine-empty state', () => {
		expect(source).toContain('fetchCurrentSpDepositorsStrict');
		expect(source).toContain('fetchCollateralConfigsStrict');
		expect(source).toContain("loadIssue = 'Collateral coverage is temporarily unavailable.'");
		expect(source.indexOf('{:else if loadIssue}')).toBeGreaterThan(-1);
		expect(source.indexOf('{:else if loadIssue}')).toBeLessThan(
			source.indexOf('{:else if rows.length === 0}')
		);
	});
});
