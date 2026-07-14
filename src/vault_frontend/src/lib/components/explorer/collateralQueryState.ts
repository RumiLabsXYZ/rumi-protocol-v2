export function collateralQueryIssue(
	configs: PromiseSettledResult<unknown[]>,
	totals: PromiseSettledResult<unknown[]>,
	vaults: PromiseSettledResult<unknown[]>
): string | null {
	const unavailable: string[] = [];
	if (configs.status === 'rejected') unavailable.push('collateral configuration');
	if (totals.status === 'rejected') unavailable.push('collateral totals');
	if (vaults.status === 'rejected') unavailable.push('vault enumeration');
	if (unavailable.length === 0) return null;

	const last = unavailable.pop();
	const subject = unavailable.length > 0 ? `${unavailable.join(', ')} and ${last}` : last;
	return `${subject} ${unavailable.length > 0 ? 'are' : 'is'} temporarily unavailable.`;
}
