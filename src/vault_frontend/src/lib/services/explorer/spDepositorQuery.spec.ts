import { Principal } from '@dfinity/principal';
import { describe, expect, it, vi } from 'vitest';
import { fetchCompleteSpPositions, fetchLegacySpPositions } from './spDepositorQuery';

const depositor = Principal.fromText('aaaaa-aa');

describe('stability-pool depositor query completeness', () => {
	it('rejects when depositor enumeration fails', async () => {
		const listDepositors = vi.fn().mockRejectedValue(new Error('list unavailable'));

		await expect(fetchCompleteSpPositions(listDepositors, vi.fn())).rejects.toThrow(
			'list unavailable'
		);
	});

	it('rejects when any enumerated position query fails', async () => {
		const listDepositors = vi.fn().mockResolvedValue([depositor]);
		const getPosition = vi.fn().mockRejectedValue(new Error('position unavailable'));

		await expect(fetchCompleteSpPositions(listDepositors, getPosition)).rejects.toThrow(
			'position unavailable'
		);
	});

	it('skips a position that disappeared after enumeration without hiding peers', async () => {
		const peer = Principal.fromText('2vxsx-fae');
		const position = { total_usd_value_e8s: 1n };
		const listDepositors = vi.fn().mockResolvedValue([depositor, peer]);
		const getPosition = vi.fn().mockResolvedValueOnce([]).mockResolvedValueOnce([position]);

		await expect(fetchCompleteSpPositions(listDepositors, getPosition)).resolves.toEqual([
			[peer, position]
		]);
	});
});

describe('legacy stability-pool depositor queries', () => {
	it('keeps successful peers when one position query fails', async () => {
		const peer = Principal.fromText('2vxsx-fae');
		const position = { total_usd_value_e8s: 1n };
		const listDepositors = vi.fn().mockResolvedValue([depositor, peer]);
		const getPosition = vi.fn()
			.mockRejectedValueOnce(new Error('position unavailable'))
			.mockResolvedValueOnce([position]);

		await expect(fetchLegacySpPositions(listDepositors, getPosition)).resolves.toEqual([
			[peer, position],
		]);
	});
});
