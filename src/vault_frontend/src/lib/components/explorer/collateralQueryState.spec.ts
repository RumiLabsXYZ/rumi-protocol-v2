import { describe, expect, it } from 'vitest';
import { collateralQueryIssue } from './collateralQueryState';

describe('collateral explorer query degradation', () => {
	it('reports config failure without calling it an empty protocol', () => {
		expect(
			collateralQueryIssue(
				{ status: 'rejected', reason: new Error('config unavailable') },
				{ status: 'fulfilled', value: [{ collateral_type: 'ICP' }] },
				{ status: 'fulfilled', value: [] }
			)
		).toContain('configuration');
	});

	it('reports totals failure without calling it an empty protocol', () => {
		expect(
			collateralQueryIssue(
				{ status: 'fulfilled', value: [{ ledger_canister_id: 'ICP' }] },
				{ status: 'rejected', reason: new Error('totals unavailable') },
				{ status: 'fulfilled', value: [] }
			)
		).toContain('totals');
	});

	it('reports when both collateral queries fail', () => {
		expect(
			collateralQueryIssue(
				{ status: 'rejected', reason: new Error('config unavailable') },
				{ status: 'rejected', reason: new Error('totals unavailable') },
				{ status: 'fulfilled', value: [] }
			)
		).toContain('configuration');
		expect(
			collateralQueryIssue(
				{ status: 'rejected', reason: new Error('config unavailable') },
				{ status: 'rejected', reason: new Error('totals unavailable') },
				{ status: 'fulfilled', value: [] }
			)
		).toContain('totals');
	});

	it('reports vault enumeration failure instead of treating risk data as empty', () => {
		expect(
			collateralQueryIssue(
				{ status: 'fulfilled', value: [] },
				{ status: 'fulfilled', value: [] },
				{ status: 'rejected', reason: new Error('vaults unavailable') }
			)
		).toContain('vault enumeration');
	});

	it('accepts a genuinely empty successful response', () => {
		expect(
			collateralQueryIssue(
				{ status: 'fulfilled', value: [] },
				{ status: 'fulfilled', value: [] },
				{ status: 'fulfilled', value: [] }
			)
		).toBeNull();
	});
});
