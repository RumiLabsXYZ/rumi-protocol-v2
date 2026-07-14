import type { CollateralInfo } from '../../services/stabilityPoolService';

const BOB_COLLATERAL_PRINCIPAL = '7pail-xaaaa-aaaas-aabmq-cai';
const HIDDEN_GAIN_SYMBOLS = new Set(['PHASMA']);

export function isSunsetBobCollateral(collateral: CollateralInfo): boolean {
	// BOB's one-way wind-down policy is principal-bound in the canister. Do
	// not advertise re-entry if the independently managed SP registry still
	// carries its historical Active status during activation.
	return collateral.ledger_id.toText() === BOB_COLLATERAL_PRINCIPAL;
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
			!isSunsetBobCollateral(collateral) || !optedOut.has(collateral.ledger_id.toText())
	);
}
