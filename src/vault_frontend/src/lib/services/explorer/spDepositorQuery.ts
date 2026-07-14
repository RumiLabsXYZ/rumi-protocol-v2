export async function fetchCompleteSpPositions<P, T>(
	listDepositors: () => Promise<P[]>,
	getPosition: (principal: P) => Promise<[] | [T]>
): Promise<Array<[P, T]>> {
	const principals = await listDepositors();
	const positions = await Promise.all(principals.map((principal) => getPosition(principal)));
	const complete: Array<[P, T]> = [];

	for (let index = 0; index < principals.length; index += 1) {
		const position = positions[index][0];
		if (position !== undefined) complete.push([principals[index], position]);
	}

	return complete;
}

export async function fetchLegacySpPositions<P, T>(
	listDepositors: () => Promise<P[]>,
	getPosition: (principal: P) => Promise<[] | [T]>
): Promise<Array<[P, T]>> {
	const principals = await listDepositors();
	const positions = await Promise.all(
		principals.map(async (principal) => {
			try {
				return await getPosition(principal);
			} catch {
				return [];
			}
		})
	);
	const available: Array<[P, T]> = [];

	for (let index = 0; index < principals.length; index += 1) {
		const position = positions[index][0];
		if (position !== undefined) available.push([principals[index], position]);
	}

	return available;
}
