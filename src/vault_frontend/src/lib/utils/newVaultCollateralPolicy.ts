/**
 * Collateral types hidden from the NEW-VAULT selector on the borrow page only.
 *
 * These collaterals stay fully active everywhere else (existing vaults,
 * explorer, stability pool, redemptions). This is a display-only gate so the
 * protocol keeps servicing them while no new vaults are opened against them.
 *
 * To revert: remove the principal from this set (empty the set to show
 * everything again). No call sites need to change.
 */
const HIDDEN_NEW_VAULT_COLLATERAL_PRINCIPALS = new Set([
	'buwm7-7yaaa-aaaar-qagva-cai' // nICP (WaterNeuron Staked ICP)
]);

export function isHiddenForNewVaults(principalText: string): boolean {
	return HIDDEN_NEW_VAULT_COLLATERAL_PRINCIPALS.has(principalText);
}

export function newVaultCollateralTypes<T extends { principal: string }>(collaterals: T[]): T[] {
	return collaterals.filter((collateral) => !isHiddenForNewVaults(collateral.principal));
}
