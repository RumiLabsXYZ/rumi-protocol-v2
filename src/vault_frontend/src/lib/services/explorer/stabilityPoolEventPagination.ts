export const STABILITY_POOL_EVENTS_PAGE_SIZE = 500n;

type SequentialEvent = {
	id?: bigint | number;
};

type FetchEventsPage<T> = (start: bigint, length: bigint) => Promise<T[]>;

export async function fetchSequentialEventById<T extends SequentialEvent>(
	id: number,
	fetchPage: FetchEventsPage<T>,
): Promise<T | null> {
	if (!Number.isSafeInteger(id) || id < 0) return null;

	const firstPage = await fetchPage(0n, 1n);
	const first = firstPage[0];
	if (!first) return null;

	const firstId = Number(first.id ?? 0);
	if (!Number.isSafeInteger(firstId) || id < firstId) return null;
	if (id === firstId) return first;

	const page = await fetchPage(BigInt(id - firstId), 1n);
	return page.find((event) => Number(event.id ?? -1) === id) ?? null;
}

export async function fetchAllSequentialEvents<T>(
	count: bigint,
	fetchPage: FetchEventsPage<T>,
	pageSize = STABILITY_POOL_EVENTS_PAGE_SIZE,
): Promise<T[]> {
	if (count <= 0n) return [];
	if (pageSize <= 0n) throw new Error('pageSize must be positive');

	const pages: T[][] = [];
	for (let start = 0n; start < count; start += pageSize) {
		const remaining = count - start;
		const length = remaining < pageSize ? remaining : pageSize;
		pages.push(await fetchPage(start, length));
	}
	return pages.flat();
}
