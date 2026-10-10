import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
	runVaultPullOperation,
	recoverSubmittedVaultPullFromBlock,
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

function adapter(initial: VaultPullStatus, afterDispatch?: VaultPullStatus, afterReconcile?: VaultPullStatus) {
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
		}),
		reconcileFromBlock: vi.fn(async (operationId, blockIndex) => {
			calls.push(`reconcile:${operationId}:${blockIndex}`);
			if (afterReconcile) status = afterReconcile;
		})
	};
	return { adapter: instance, calls, currentStatus: () => status, setStatus: (next: VaultPullStatus) => { status = next; } };
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

	it('notifies the same tab when dispatch status first becomes Submitted', async () => {
		const submitted = view(1n, openRequest, { Submitted: null });
		const setup = adapter(
			{ acknowledged_through: 0n, active: [] },
			{ acknowledged_through: 0n, active: [submitted] }
		);
		const listener = vi.fn();
		window.addEventListener('rumi:vault-pull-status-change', listener);

		await expect(runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })).rejects.toThrow('submitted');

		expect(listener).toHaveBeenCalledOnce();
		expect((listener.mock.calls[0][0] as CustomEvent).detail).toEqual({ owner });
		window.removeEventListener('rumi:vault-pull-status-change', listener);
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

	it('reconciles the exact block, verifies progress, resumes the same operation, and ACKs', async () => {
		const submitted = view(9n, openRequest, { Submitted: null });
		const pulled = view(9n, openRequest, { PullConfirmed: { block_index: 77n } });
		const completed = view(9n, openRequest, { Completed: { result: { OpenVault: { vault_id: 42n } } } });
		const setup = adapter(
			{ acknowledged_through: 8n, active: [submitted] },
			{ acknowledged_through: 8n, active: [completed] },
			{ acknowledged_through: 8n, active: [pulled] }
		);
		await expect(runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })).rejects.toThrow('submitted');
		const result = await recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter });
		expect(result).toEqual({ vaultId: 42 });
		expect(setup.adapter.reconcileFromBlock).toHaveBeenCalledWith(9n, 77n);
		expect(setup.adapter.dispatch).toHaveBeenCalledWith(9n, openRequest);
		expect(setup.calls).toEqual(['reconcile:9:77', 'dispatch', 'ack']);
		expect(setup.adapter.approve).not.toHaveBeenCalled();
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toBeNull();
	});

	it('uses authenticated status after a lost reconcile reply before resuming', async () => {
		const submitted = view(9n, openRequest, { Submitted: null });
		const pulled = view(9n, openRequest, { PullConfirmed: { block_index: 77n } });
		const completed = view(9n, openRequest, { Completed: { result: { OpenVault: { vault_id: 42n } } } });
		const setup = adapter(
			{ acknowledged_through: 8n, active: [submitted] },
			{ acknowledged_through: 8n, active: [completed] },
			{ acknowledged_through: 8n, active: [pulled] }
		);
		await expect(runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })).rejects.toThrow('submitted');
		vi.mocked(setup.adapter.reconcileFromBlock!).mockImplementationOnce(async () => {
			setup.setStatus({ acknowledged_through: 8n, active: [pulled] });
			throw new Error('reconcile reply lost');
		});

		const result = await recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter });

		expect(result).toEqual({ vaultId: 42 });
		expect(setup.adapter.getStatus).toHaveBeenCalledTimes(6);
		expect(setup.adapter.dispatch).toHaveBeenCalledWith(9n, openRequest);
		expect(setup.adapter.acknowledge).toHaveBeenCalledWith(9n);
	});

	it('keeps the saved intent when the backend rejects a false candidate', async () => {
		const setup = adapter({ acknowledged_through: 8n, active: [view(9n, openRequest, { Submitted: null })] });
		await expect(runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })).rejects.toThrow('submitted');
		vi.mocked(setup.adapter.reconcileFromBlock!).mockRejectedValueOnce(new Error('false candidate'));
		await expect(recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 999n, adapter: setup.adapter })).rejects.toThrow('false candidate');
		await expect(recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 999n, adapter: setup.adapter })).rejects.toThrow('did not move');
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
		expect(setup.adapter.acknowledge).not.toHaveBeenCalled();
		expect(setup.adapter.reconcileFromBlock).toHaveBeenCalledTimes(2);
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toContain('9');
	});

	it('stops if wallet identity changes during the recovery await', async () => {
		const setup = adapter({ acknowledged_through: 8n, active: [view(9n, openRequest, { Submitted: null })] });
		await expect(runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })).rejects.toThrow('submitted');
		let identity = owner;
		setup.adapter.currentOwner = () => identity;
		vi.mocked(setup.adapter.reconcileFromBlock!).mockImplementationOnce(async () => { identity = 'another-wallet'; });
		await expect(recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter })).rejects.toThrow('identity changed');
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
		expect(setup.adapter.acknowledge).not.toHaveBeenCalled();
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toContain('9');
	});

	it('stops if the wallet session generation changes while the principal text stays the same', async () => {
		const submitted = view(9n, openRequest, { Submitted: null });
		const setup = adapter({ acknowledged_through: 8n, active: [submitted] });
		let generation = 1;
		setup.adapter.assertCurrent = () => {
			if (generation !== 1) throw new Error('stale wallet session');
		};
		vi.mocked(setup.adapter.reconcileFromBlock!).mockImplementationOnce(async () => { generation = 2; });
		await expect(recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter })).rejects.toThrow('stale wallet session');
		expect(setup.adapter.currentOwner()).toBe(owner);
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toBeTruthy();
	});

	it('rebuilds a missing local intent from the authenticated exact Submitted status', async () => {
		const submitted = view(12n, openRequest, { Submitted: null });
		const pulled = view(12n, openRequest, { PullConfirmed: { block_index: 88n } });
		const completed = view(12n, openRequest, { Completed: { result: { OpenVault: { vault_id: 42n } } } });
		const setup = adapter(
			{ acknowledged_through: 11n, active: [submitted] },
			{ acknowledged_through: 11n, active: [completed] },
			{ acknowledged_through: 11n, active: [pulled] }
		);

		const result = await recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 88n, adapter: setup.adapter });

		expect(result).toEqual({ vaultId: 42 });
		expect(setup.adapter.reconcileFromBlock).toHaveBeenCalledWith(12n, 88n);
		expect(setup.adapter.dispatch).toHaveBeenCalledWith(12n, openRequest);
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toBeNull();
	});

	it('does not permit a repeated recovery after the saved intent is acknowledged', async () => {
		const submitted = view(9n, openRequest, { Submitted: null });
		const pulled = view(9n, openRequest, { PullConfirmed: { block_index: 77n } });
		const completed = view(9n, openRequest, { Completed: { result: { OpenVault: { vault_id: 42n } } } });
		const setup = adapter(
			{ acknowledged_through: 8n, active: [submitted] },
			{ acknowledged_through: 8n, active: [completed] },
			{ acknowledged_through: 8n, active: [pulled] }
		);
		await expect(runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })).rejects.toThrow('submitted');
		await recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter });
		await expect(recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter })).rejects.toThrow('no active Submitted');
		expect(setup.adapter.reconcileFromBlock).toHaveBeenCalledTimes(1);
		expect(setup.adapter.dispatch).toHaveBeenCalledTimes(1);
	});

	it('rejects concurrent recovery while another tab holds the wallet lock', async () => {
		let release!: () => void;
		const submitted = view(9n, openRequest, { Submitted: null });
		const pulled = view(9n, openRequest, { PullConfirmed: { block_index: 77n } });
		const completed = view(9n, openRequest, { Completed: { result: { OpenVault: { vault_id: 42n } } } });
		const setup = adapter(
			{ acknowledged_through: 8n, active: [submitted] },
			{ acknowledged_through: 8n, active: [completed] },
			{ acknowledged_through: 8n, active: [pulled] }
		);
		await expect(runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })).rejects.toThrow('submitted');
		vi.mocked(setup.adapter.reconcileFromBlock!).mockImplementationOnce(async () => {
			await new Promise<void>((resolve) => { release = resolve; });
			setup.setStatus({ acknowledged_through: 8n, active: [pulled] });
		});
		const first = recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter });
		await vi.waitFor(() => expect(release).toBeTypeOf('function'));
		await expect(recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter })).rejects.toThrow('Another tab');
		release();
		await expect(first).resolves.toEqual({ vaultId: 42 });
		expect(setup.adapter.reconcileFromBlock).toHaveBeenCalledTimes(1);
	});

	it('does not create a new collateral operation if another session ACKs after proof verification', async () => {
		const submitted = view(9n, openRequest, { Submitted: null });
		const pulled = view(9n, openRequest, { PullConfirmed: { block_index: 77n } });
		const setup = adapter({ acknowledged_through: 8n, active: [submitted] }, undefined, {
			acknowledged_through: 8n,
			active: [pulled]
		});
		await expect(runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })).rejects.toThrow('submitted');
		let statusRead = 0;
		vi.mocked(setup.adapter.getStatus).mockImplementation(async () => {
			statusRead += 1;
			if (statusRead === 1) return { acknowledged_through: 8n, active: [submitted] };
			if (statusRead === 2) return { acknowledged_through: 8n, active: [pulled] };
			return { acknowledged_through: 9n, active: [] };
		});

		await expect(recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter })).rejects.toThrow('acknowledged during recovery');

		expect(setup.adapter.approve).not.toHaveBeenCalled();
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
		expect(setup.adapter.acknowledge).not.toHaveBeenCalled();
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toBeNull();
	});
});
