import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, 'explorerService.ts'), 'utf8');

function functionBody(name: string): string {
	const start = source.indexOf(`export async function ${name}(`);
	expect(start, `${name} must exist`).toBeGreaterThanOrEqual(0);
	const next = source.indexOf('\nexport async function ', start + 1);
	return source.slice(start, next === -1 ? source.length : next);
}

describe('strict explorer reads do not change legacy fail-soft consumers', () => {
	it('keeps strict and legacy caches isolated', () => {
		expect(functionBody('fetchCollateralConfigsStrict')).toContain("'collateral:configs:strict'");
		expect(functionBody('fetchCollateralConfigs')).toContain("'collateral:configs'");
		expect(functionBody('fetchAllVaultsStrict')).toContain("'vaults:all:strict'");
		expect(functionBody('fetchAllVaults')).toContain("'vaults:all'");
		expect(functionBody('fetchCurrentSpDepositorsStrict')).toContain("'pool:stability:current_depositors:strict'");
		expect(functionBody('fetchCurrentSpDepositors')).toContain("'pool:stability:current_depositors'");
	});

	it('retains per-item tolerance for legacy collateral config reads', () => {
		const body = functionBody('fetchCollateralConfigs');
		expect(body).toContain('fetchLegacyCollateralConfigs');
		expect(body).not.toContain('fetchCollateralConfigsStrict');
	});

	it('retains per-position tolerance for legacy SP depositor reads', () => {
		const body = functionBody('fetchCurrentSpDepositors');
		expect(body).toContain('fetchLegacySpPositions');
		expect(body).not.toContain('fetchCurrentSpDepositorsStrict');
	});

	it('keeps the known legacy consumers on legacy APIs', () => {
		const portfolio = readFileSync(
			resolve(__dirname, '../../components/explorer/PortfolioValueChart.svelte'),
			'utf8'
		);
		const depositors = readFileSync(
			resolve(__dirname, '../../components/explorer/SpCurrentDepositorsCard.svelte'),
			'utf8'
		);
		expect(portfolio).toContain('fetchCollateralConfigs()');
		expect(portfolio).not.toContain('fetchCollateralConfigsStrict');
		expect(depositors).toContain('fetchCurrentSpDepositors');
		expect(depositors).not.toContain('fetchCurrentSpDepositorsStrict');
	});
});
