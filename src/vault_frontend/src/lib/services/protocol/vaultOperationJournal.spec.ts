import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
	runVaultPullOperation,
	type VaultPullAdapter,
	type VaultPullRequest,
	type VaultPullStatus,
	type VaultPullView
} from './vaultOperationJournal';

const owner = '2vxsx-fae';
const openRequest: VaultPullRequest = {
	OpenVault: { amount_e8s: 125_000_000n, collateral_type: 'ryjl3-tyaaa-aaaaa-aaaba-cai' }
};

function lockManager() {
	let held = false;
	return {
		request: vi.fn(
			async (
				_name: string,
				_options: unknown,
				callback: (lock: object | null) => Promise<unknown>
			) => {
				if (held) return callback(null);
				held = true;
				try {
					return await callback({});
				} finally {
					held = false;
				}
			}
		)
	};
}

function view(
	operationId: bigint,
	request: VaultPullRequest,
	phase: VaultPullView['phase']
): VaultPullView {
	return { operation_id: operationId, request, vault_id: 42n, phase };
}

function adapter(initial: VaultPullStatus, afterDispatch?: VaultPullStatus) {
	let status = initial;
	const calls: string[] = [];
	const instance: VaultPullAdapter<{ vaultId: number }> = {
		currentOwner: () => owner,
		getStatus: vi.fn(async () => status),
		approve: vi.fn(async (request) => {
			calls.push('approve');
			expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toBeTruthy();
			return { success: true };
		}),
		dispatch: vi.fn(async (operationId, request) => {
			calls.push('dispatch');
			expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toBeTruthy();
			if (afterDispatch) status = afterDispatch;
			return { operationId: operationId.toString(), request } as never;
		}),
		completed: vi.fn((current) => ({
			vaultId: Number((current.phase as any).Completed.result.OpenVault.vault_id)
		})),
		acknowledge: vi.fn(async (operationId) => {
			calls.push('ack');
			const active = status.active[0];
			expect(active?.operation_id).toBe(operationId);
			status = { acknowledged_through: operationId, active: [] };
		})
	};
	return { adapter: instance, calls, currentStatus: () => status };
}

describe('runVaultPullOperation', () => {
	beforeEach(() => {
		localStorage.clear();
		Object.defineProperty(navigator, 'locks', { configurable: true, value: lockManager() });
	});

	it('persists the exact next ID and arguments before approval, then ACKs only after terminal completion', async () => {
		const completed = view(5n, openRequest, {
			Completed: { result: { OpenVault: { vault_id: 42n } } }
		});
		const setup = adapter(
			{ acknowledged_through: 4n, active: [] },
			{ acknowledged_through: 4n, active: [completed] }
		);

		const result = await runVaultPullOperation({
			owner,
			request: openRequest,
			adapter: setup.adapter
		});

		expect(result).toEqual({ vaultId: 42 });
		expect(setup.adapter.approve).toHaveBeenCalledWith(openRequest);
		expect(setup.adapter.dispatch).toHaveBeenCalledWith(5n, openRequest);
		expect(setup.calls).toEqual(['approve', 'dispatch', 'ack']);
		expect(setup.currentStatus().acknowledged_through).toBe(5n);
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toBeNull();
	});

	it('reconstructs the exact active request and ID from authenticated status after localStorage loss', async () => {
		const recoveredRequest: VaultPullRequest = {
			OpenVaultAndBorrow: {
				amount_e8s: 88_000_000n,
				borrow_amount_e8s: 1_250_000_000n,
				collateral_type: 'ryjl3-tyaaa-aaaaa-aaaba-cai'
			}
		};
		const completed = view(19n, recoveredRequest, {
			Completed: { result: { OpenVault: { vault_id: 77n } } }
		});
		const setup = adapter(
			{ acknowledged_through: 18n, active: [view(19n, recoveredRequest, { Prepared: null })] },
			{ acknowledged_through: 18n, active: [completed] }
		);

		await runVaultPullOperation({ owner, request: recoveredRequest, adapter: setup.adapter });

		expect(setup.adapter.dispatch).toHaveBeenCalledWith(19n, recoveredRequest);
		expect(setup.adapter.approve).toHaveBeenCalledWith(recoveredRequest);
		expect(setup.currentStatus().acknowledged_through).toBe(19n);
	});

	it('adopts a backend-active request but refuses to run it from a different user action', async () => {
		const activeRequest: VaultPullRequest = {
			OpenVaultAndBorrow: {
				amount_e8s: 88_000_000n,
				borrow_amount_e8s: 1_250_000_000n,
				collateral_type: 'ryjl3-tyaaa-aaaaa-aaaba-cai'
			}
		};
		const setup = adapter({
			acknowledged_through: 18n,
			active: [view(19n, activeRequest, { Prepared: null })]
		});

		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('still bound to different arguments');

		expect(setup.adapter.approve).not.toHaveBeenCalled();
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
		expect(setup.adapter.acknowledge).not.toHaveBeenCalled();
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toContain('1250000000n');
	});

	it('keeps a saved exact intent held when the incoming form request has changed', async () => {
		const savedRequest: VaultPullRequest = {
			OpenVaultAndBorrow: {
				amount_e8s: 88_000_000n,
				borrow_amount_e8s: 1_250_000_000n,
				collateral_type: 'ryjl3-tyaaa-aaaaa-aaaba-cai'
			}
		};
		const savedIntent = {
			version: 1,
			owner,
			operationId: '19',
			request: {
				OpenVaultAndBorrow: {
					amount_e8s: '88000000n',
					borrow_amount_e8s: '1250000000n',
					collateral_type: 'ryjl3-tyaaa-aaaaa-aaaba-cai'
				}
			},
			savedAt: Date.now()
		};
		localStorage.setItem(`rumi:vault-pull-operation:v1:${owner}`, JSON.stringify(savedIntent));
		const setup = adapter({
			acknowledged_through: 18n,
			active: [view(19n, savedRequest, { Prepared: null })]
		});

		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('different arguments');

		expect(setup.adapter.approve).not.toHaveBeenCalled();
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toContain('1250000000n');
	});

	it('keeps Submitted operations held with exact ID and never approves, redispatches, or ACKs', async () => {
		const pending = view(9n, openRequest, { Submitted: null });
		const setup = adapter({ acknowledged_through: 8n, active: [pending] });

		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('Vault operation 9 is submitted');

		expect(setup.adapter.approve).not.toHaveBeenCalled();
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
		expect(setup.adapter.acknowledge).not.toHaveBeenCalled();
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toContain('9');
	});

	it('fails closed when another tab already holds the wallet lock', async () => {
		Object.defineProperty(navigator, 'locks', {
			configurable: true,
			value: {
				request: async (
					_name: string,
					_options: unknown,
					callback: (lock: object | null) => Promise<unknown>
				) => callback(null)
			}
		});
		const setup = adapter({ acknowledged_through: 0n, active: [] });

		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('Another tab is already handling');
		expect(setup.adapter.getStatus).not.toHaveBeenCalled();
	});

	it('fails closed when Web Locks are unavailable', async () => {
		Object.defineProperty(navigator, 'locks', { configurable: true, value: undefined });
		const setup = adapter({ acknowledged_through: 0n, active: [] });

		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('cannot safely lock vault operations');
		expect(setup.adapter.getStatus).not.toHaveBeenCalled();
	});
});
