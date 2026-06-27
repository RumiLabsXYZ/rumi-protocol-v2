import { describe, expect, it, vi } from 'vitest';
import {
	fetchAllSequentialEvents,
	fetchSequentialEventById,
	STABILITY_POOL_EVENTS_PAGE_SIZE,
} from './stabilityPoolEventPagination';

describe('stability pool event pagination', () => {
	it('fetches a single sequential event directly by id', async () => {
		const fetchPage = vi.fn(async (start: bigint, length: bigint) => [
			{ id: Number(start), length },
		]);

		const event = await fetchSequentialEventById(580, fetchPage);

		expect(fetchPage).toHaveBeenNthCalledWith(1, 0n, 1n);
		expect(fetchPage).toHaveBeenNthCalledWith(2, 580n, 1n);
		expect(event).toEqual({ id: 580, length: 1n });
	});

	it('accounts for retained logs whose first event id is no longer zero', async () => {
		const fetchPage = vi.fn(async (start: bigint) => {
			const firstRetainedId = 10_000;
			return [{ id: firstRetainedId + Number(start) }];
		});

		const event = await fetchSequentialEventById(10_580, fetchPage);

		expect(fetchPage).toHaveBeenNthCalledWith(1, 0n, 1n);
		expect(fetchPage).toHaveBeenNthCalledWith(2, 580n, 1n);
		expect(event).toEqual({ id: 10_580 });
	});

	it('returns null when the direct page does not contain the requested id', async () => {
		const fetchPage = vi.fn(async (start: bigint) => {
			if (start === 0n) return [{ id: 0 }];
			return [{ id: 579 }];
		});

		await expect(fetchSequentialEventById(580, fetchPage)).resolves.toBeNull();
	});

	it('walks full event logs in pages that respect the canister cap', async () => {
		const fetchPage = vi.fn(async (start: bigint, length: bigint) => {
			return Array.from({ length: Number(length) }, (_, offset) => ({
				id: Number(start) + offset,
			}));
		});

		const events = await fetchAllSequentialEvents(582n, fetchPage);

		expect(fetchPage).toHaveBeenCalledTimes(2);
		expect(fetchPage).toHaveBeenNthCalledWith(1, 0n, STABILITY_POOL_EVENTS_PAGE_SIZE);
		expect(fetchPage).toHaveBeenNthCalledWith(2, 500n, 82n);
		expect(events).toHaveLength(582);
		expect(events.at(-1)).toEqual({ id: 581 });
	});
});
