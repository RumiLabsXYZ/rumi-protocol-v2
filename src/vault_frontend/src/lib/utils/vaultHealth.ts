export const VAULT_CR_DANGER = '#e06b9f';
export const VAULT_CR_SAFE = '#e2e8f0';

/**
 * Health headroom represented by the vault card's CR text tint.
 * Zero is at the liquidation boundary; one is at the white end of the tint.
 * Deliberately unbounded so sorting can distinguish both red and white values.
 */
export function getVaultHealthHeadroom(cr: number, minimumCr: number, liquidationCr: number): number {
	const whiteCr = minimumCr * 1.351;
	const span = whiteCr - liquidationCr;
	if (!Number.isFinite(cr) || !Number.isFinite(whiteCr) || !Number.isFinite(liquidationCr) || span <= 0) {
		return Number.POSITIVE_INFINITY;
	}
	return (cr - liquidationCr) / span;
}

function interpolateHex(from: string, to: string, amount: number): string {
	const a = from.match(/[\da-f]{2}/gi)?.map((v) => parseInt(v, 16)) ?? [226, 232, 240];
	const b = to.match(/[\da-f]{2}/gi)?.map((v) => parseInt(v, 16)) ?? [224, 107, 159];
	const channels = a.map((start, i) => Math.round(start + ((b[i] ?? start) - start) * amount));
	return `#${channels.map((channel) => channel.toString(16).padStart(2, '0')).join('')}`;
}

/** Matches the existing white-to-pink CR label fade, including its clamps. */
export function getVaultCrTextColor(cr: number, minimumCr: number, liquidationCr: number): string {
	const headroom = getVaultHealthHeadroom(cr, minimumCr, liquidationCr);
	if (!Number.isFinite(headroom) || headroom >= 1) return VAULT_CR_SAFE;
	if (headroom <= 0) return VAULT_CR_DANGER;
	return interpolateHex(VAULT_CR_SAFE, VAULT_CR_DANGER, 1 - headroom);
}
