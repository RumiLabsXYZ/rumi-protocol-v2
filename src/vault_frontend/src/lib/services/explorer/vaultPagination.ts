export interface VaultPage<T> {
	vaults: T[];
	next_start_id: [] | [bigint];
}

export async function fetchCompleteVaultPages<T>(
	fetchPage: (startId: bigint, pageSize: bigint) => Promise<VaultPage<T>>,
	pageSize: bigint,
	maxPages: number
): Promise<T[]> {
	const all: T[] = [];
	let startId = 0n;

	for (let page = 0; page < maxPages; page += 1) {
		const response = await fetchPage(startId, pageSize);
		all.push(...response.vaults);
		if (response.next_start_id.length === 0) return all;
		startId = response.next_start_id[0];
	}

	throw new Error(`Vault pagination limit (${maxPages} pages) exhausted`);
}
