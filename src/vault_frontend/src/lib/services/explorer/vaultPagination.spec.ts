import { describe, expect, it, vi } from 'vitest';
import { fetchCompleteVaultPages } from './vaultPagination';

describe('vault enumeration completeness', () => {
	it('rejects a first-page query failure', async () => {
		const fetchPage = vi.fn().mockRejectedValue(new Error('vault page unavailable'));

		await expect(fetchCompleteVaultPages(fetchPage, 500n, 100)).rejects.toThrow(
			'vault page unavailable'
		);
	});

	it('rejects a later-page query failure instead of returning a partial list', async () => {
		const fetchPage = vi
			.fn()
			.mockResolvedValueOnce({ vaults: [{ vault_id: 1n }], next_start_id: [2n] })
			.mockRejectedValueOnce(new Error('second page unavailable'));

		await expect(fetchCompleteVaultPages(fetchPage, 500n, 100)).rejects.toThrow(
			'second page unavailable'
		);
	});

	it('returns every page after complete pagination', async () => {
		const fetchPage = vi
			.fn()
			.mockResolvedValueOnce({ vaults: [{ vault_id: 1n }], next_start_id: [2n] })
			.mockResolvedValueOnce({ vaults: [{ vault_id: 2n }], next_start_id: [] });

		await expect(fetchCompleteVaultPages(fetchPage, 500n, 100)).resolves.toEqual([
			{ vault_id: 1n },
			{ vault_id: 2n }
		]);
	});

	it('rejects instead of returning partial data when the page cap is exhausted', async () => {
		const fetchPage = vi.fn().mockResolvedValue({
			vaults: [{ vault_id: 1n }],
			next_start_id: [2n]
		});

		await expect(fetchCompleteVaultPages(fetchPage, 500n, 1)).rejects.toThrow('pagination limit');
	});
});
