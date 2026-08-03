import type { CollateralInfo } from '../../services/stabilityPoolService';

const BOB_COLLATERAL_PRINCIPAL = '7pail-xaaaa-aaaas-aabmq-cai';
const EXE_COLLATERAL_PRINCIPAL = 'rh2pm-ryaaa-aaaan-qeniq-cai';
// Sunset collaterals in one-way wind-down. Add to this set (not the call
// sites) when another collateral is sunset.
const SUNSET_COLLATERAL_PRINCIPALS = new Set([BOB_COLLATERAL_PRINCIPAL, EXE_COLLATERAL_PRINCIPAL]);
const HIDDEN_GAIN_SYMBOLS = new Set(['PHASMA']);

export function isSunsetCollateral(collateral: CollateralInfo): boolean {
	// Sunset wind-down policy is principal-bound here. Do not advertise
	// re-entry if the independently managed SP registry still carries its
	// historical Active status during activation.
	return SUNSET_COLLATERAL_PRINCIPALS.has(collateral.ledger_id.toText());
}

export function gainCollaterals(collaterals: CollateralInfo[]): CollateralInfo[] {
	return collaterals.filter((collateral) => !HIDDEN_GAIN_SYMBOLS.has(collateral.symbol));
}

export function liquidationPreferenceCollaterals(
	collaterals: CollateralInfo[],
	optedOut: Set<string>
): CollateralInfo[] {
	return gainCollaterals(collaterals).filter(
		(collateral) =>
			!isSunsetCollateral(collateral) || !optedOut.has(collateral.ledger_id.toText())
	);
}
