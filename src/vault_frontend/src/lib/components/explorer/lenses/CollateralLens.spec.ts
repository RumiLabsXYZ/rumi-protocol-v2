import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const source = readFileSync(resolve(__dirname, 'CollateralLens.svelte'), 'utf8');

describe('CollateralLens strict query degradation', () => {
	it('uses strict reads only on this truth-sensitive surface', () => {
		expect(source).toContain('fetchCollateralConfigsStrict');
		expect(source).toContain('fetchCollateralTotalsStrict');
		expect(source).toContain('fetchAllVaultsStrict');
	});

	it('suppresses health and CR empty claims when collateral data is degraded', () => {
		const issueBranch = source.indexOf('{#if collateralDataIssue}');
		const healthStrip = source.indexOf('<LensHealthStrip');
		const crPanel = source.indexOf('>CR distribution<');
		const crIssueBranch = source.indexOf('{:else if collateralDataIssue}', crPanel);
		const noVaults = source.indexOf('No active vaults.');

		expect(issueBranch).toBeGreaterThan(-1);
		expect(healthStrip).toBeGreaterThan(issueBranch);
		expect(crIssueBranch).toBeGreaterThan(crPanel);
		expect(noVaults).toBeGreaterThan(crIssueBranch);
	});

	it('suppresses the activity panel when collateral data is degraded', () => {
		const activityPanel = source.lastIndexOf('<LensActivityPanel');
		const activityIssueBranch = source.lastIndexOf('{#if collateralDataIssue}', activityPanel);
		const unavailable = source.indexOf('Vault activity is temporarily unavailable.', activityIssueBranch);

		expect(activityIssueBranch).toBeGreaterThan(-1);
		expect(unavailable).toBeGreaterThan(activityIssueBranch);
		expect(activityPanel).toBeGreaterThan(unavailable);
	});
});
