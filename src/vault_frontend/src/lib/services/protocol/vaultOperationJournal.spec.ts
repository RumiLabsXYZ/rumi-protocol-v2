import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
	runVaultPullOperation,
	sameVaultPullRequest,
	parseVaultPullIntent,
	inspectVaultPullRecovery,
	cancelSavedVaultPullOperation,
	resumeSavedVaultPullOperation,
	type VaultPullIntent,
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

function adapter(
	initial: VaultPullStatus,
	afterDispatch?: VaultPullStatus,
	afterReconcile?: VaultPullStatus
) {
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
	return {
		adapter: instance,
		calls,
		currentStatus: () => status,
		setStatus: (next: VaultPullStatus) => {
			status = next;
		}
	};
}

describe('runVaultPullOperation', () => {
	beforeEach(() => {
		localStorage.clear();
		Object.defineProperty(navigator, 'locks', { configurable: true, value: lockManager() });
	});


  it('holds a new interrupted request after an earlier completion, then permits changed terms only after fenced cancellation', async () => {
    const exact: VaultPullRequest = { OpenVaultAndBorrow: { amount_e8s: 140_000_000n, borrow_amount_e8s: 240_000_000n, collateral_type: 'ryjl3-tyaaa-aaaaa-aaaba-cai' } };
    const changed: VaultPullRequest = { OpenVaultAndBorrow: { ...exact.OpenVaultAndBorrow, amount_e8s: 143_000_000n, borrow_amount_e8s: 246_000_000n } };
    const setup = adapter({ acknowledged_through: 2n, active: [] });
    vi.mocked(setup.adapter.approve).mockResolvedValueOnce({ success: false, error: 'Approval interrupted' });
    await expect(runVaultPullOperation({owner, request: exact, adapter: setup.adapter})).rejects.toThrow('Approval interrupted');
    const raw = localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)!;
    expect(parseVaultPullIntent(raw)?.operationId).toBe('3');
    await expect(runVaultPullOperation({owner, request: changed, adapter: setup.adapter})).rejects.toThrow('still bound to different arguments');
    expect(setup.adapter.approve).toHaveBeenCalledTimes(1);
    expect(setup.adapter.dispatch).not.toHaveBeenCalled();
    expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toBe(raw);
    const reviewed = await inspectVaultPullRecovery({owner, adapter: setup.adapter});
    setup.adapter.cancel = vi.fn(async id => { setup.setStatus({acknowledged_through:id, active:[]}); return id; });
    await cancelSavedVaultPullOperation({owner, intent: reviewed.intent!, adapter: setup.adapter});
    expect(setup.adapter.cancel).toHaveBeenCalledWith(3n, exact);
    expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toBeNull();
    vi.mocked(setup.adapter.dispatch).mockImplementationOnce(async (id, request) => {
      setup.setStatus({acknowledged_through:3n, active:[view(id, request, {Completed:{result:{OpenVault:{vault_id:77n}}}})]});
      return null;
    });
    await expect(runVaultPullOperation({owner, request: changed, adapter: setup.adapter})).resolves.toEqual({vaultId:77});
    expect(setup.adapter.dispatch).toHaveBeenCalledWith(4n, changed);
  });

	it('compares exact typed requests independently of object field ordering', () => {
		expect(
			sameVaultPullRequest(openRequest, {
				OpenVault: {
					collateral_type: (openRequest as any).OpenVault.collateral_type,
					amount_e8s: 125_000_000n
				}
			})
		).toBe(true);
	});

	it('rejects malformed typed v1 requests instead of treating them as resumable', () => {
		const raw = JSON.stringify({
			version: 1,
			owner,
			operationId: '1',
			savedAt: 1,
			request: {
				OpenVault: {
					amount_e8s: '125000000',
					collateral_type: (openRequest as any).OpenVault.collateral_type
				}
			}
		});
		expect(parseVaultPullIntent(raw)).toBeNull();
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

	it('uses definitive dispatch and ACK replies without two extra signed status checks', async () => {
		const setup = adapter({ acknowledged_through: 4n, active: [] });
		setup.adapter.dispatch = vi.fn(async () => ({ vault_id: 42n, block_index: 7n }));
		setup.adapter.resultFromDispatch = vi.fn(() => ({ vaultId: 42 }));
		setup.adapter.acknowledge = vi.fn(async () => 5n);
		expect(
			await runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).toEqual({ vaultId: 42 });
		expect(setup.adapter.getStatus).toHaveBeenCalledTimes(1);
		expect(setup.adapter.approve).toHaveBeenCalledTimes(1);
		expect(setup.adapter.dispatch).toHaveBeenCalledTimes(1);
		expect(setup.adapter.acknowledge).toHaveBeenCalledWith(5n);
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toBeNull();
	});

	it('keeps an ambiguous dispatch on authoritative journal recovery and never accepts a guessed result', async () => {
		const submitted = view(5n, openRequest, { Submitted: null });
		const setup = adapter({ acknowledged_through: 4n, active: [] });
		setup.adapter.dispatch = vi.fn(async () => {
			setup.setStatus({ acknowledged_through: 4n, active: [submitted] });
			throw new Error('lost reply');
		});
		setup.adapter.resultFromDispatch = vi.fn(() => ({ vaultId: 42 }));
		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('unknown transfer outcome');
		expect(setup.adapter.getStatus).toHaveBeenCalledTimes(2);
		expect(setup.adapter.resultFromDispatch).not.toHaveBeenCalled();
		expect(setup.adapter.acknowledge).not.toHaveBeenCalled();
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toBeTruthy();
	});

	it('retains the exact intent when a certified ACK does not cover this operation', async () => {
		const setup = adapter({ acknowledged_through: 4n, active: [] });
		setup.adapter.dispatch = vi.fn(async () => ({ vault_id: 42n, block_index: 7n }));
		setup.adapter.resultFromDispatch = vi.fn(() => ({ vaultId: 42 }));
		setup.adapter.acknowledge = vi.fn(async () => 4n);
		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('ACK did not confirm');
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toBeTruthy();
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

	it('treats a confirmed wide collateral receipt as held and never offers candidate recovery', async () => {
		const pending = {
			...view(9n, openRequest, { Submitted: null }),
			confirmed_wide_block_index: [18_446_744_073_709_551_616n] as [bigint]
		};
		const setup = adapter({ acknowledged_through: 8n, active: [pending] });
		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('was confirmed at ledger block 18446744073709551616');
		await expect(
			recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter })
		).rejects.toThrow('Do not retry the transfer or submit a candidate block');
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
		expect(setup.adapter.reconcileFromBlock).not.toHaveBeenCalled();
		expect(setup.adapter.acknowledge).not.toHaveBeenCalled();
	});

	it('notifies the same tab after verified save and when status becomes Submitted', async () => {
		const submitted = view(1n, openRequest, { Submitted: null });
		const setup = adapter(
			{ acknowledged_through: 0n, active: [] },
			{ acknowledged_through: 0n, active: [submitted] }
		);
		const listener = vi.fn();
		window.addEventListener('rumi:vault-pull-status-change', listener);

		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('submitted');

		expect(listener).toHaveBeenCalledTimes(2);
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
		const completed = view(9n, openRequest, {
			Completed: { result: { OpenVault: { vault_id: 42n } } }
		});
		const setup = adapter(
			{ acknowledged_through: 8n, active: [submitted] },
			{ acknowledged_through: 8n, active: [completed] },
			{ acknowledged_through: 8n, active: [pulled] }
		);
		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('submitted');
		const result = await recoverSubmittedVaultPullFromBlock({
			owner,
			blockIndex: 77n,
			adapter: setup.adapter
		});
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
		const completed = view(9n, openRequest, {
			Completed: { result: { OpenVault: { vault_id: 42n } } }
		});
		const setup = adapter(
			{ acknowledged_through: 8n, active: [submitted] },
			{ acknowledged_through: 8n, active: [completed] },
			{ acknowledged_through: 8n, active: [pulled] }
		);
		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('submitted');
		vi.mocked(setup.adapter.reconcileFromBlock!).mockImplementationOnce(async () => {
			setup.setStatus({ acknowledged_through: 8n, active: [pulled] });
			throw new Error('reconcile reply lost');
		});

		const result = await recoverSubmittedVaultPullFromBlock({
			owner,
			blockIndex: 77n,
			adapter: setup.adapter
		});

		expect(result).toEqual({ vaultId: 42 });
		expect(setup.adapter.getStatus).toHaveBeenCalledTimes(6);
		expect(setup.adapter.dispatch).toHaveBeenCalledWith(9n, openRequest);
		expect(setup.adapter.acknowledge).toHaveBeenCalledWith(9n);
	});

	it('keeps the saved intent when the backend rejects a false candidate', async () => {
		const setup = adapter({
			acknowledged_through: 8n,
			active: [view(9n, openRequest, { Submitted: null })]
		});
		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('submitted');
		vi.mocked(setup.adapter.reconcileFromBlock!).mockRejectedValueOnce(
			new Error('false candidate')
		);
		await expect(
			recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 999n, adapter: setup.adapter })
		).rejects.toThrow('false candidate');
		await expect(
			recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 999n, adapter: setup.adapter })
		).rejects.toThrow('did not move');
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
		expect(setup.adapter.acknowledge).not.toHaveBeenCalled();
		expect(setup.adapter.reconcileFromBlock).toHaveBeenCalledTimes(2);
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toContain('9');
	});

	it('stops if wallet identity changes during the recovery await', async () => {
		const setup = adapter({
			acknowledged_through: 8n,
			active: [view(9n, openRequest, { Submitted: null })]
		});
		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('submitted');
		let identity = owner;
		setup.adapter.currentOwner = () => identity;
		vi.mocked(setup.adapter.reconcileFromBlock!).mockImplementationOnce(async () => {
			identity = 'another-wallet';
		});
		await expect(
			recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter })
		).rejects.toThrow('identity changed');
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
		vi.mocked(setup.adapter.reconcileFromBlock!).mockImplementationOnce(async () => {
			generation = 2;
		});
		await expect(
			recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter })
		).rejects.toThrow('stale wallet session');
		expect(setup.adapter.currentOwner()).toBe(owner);
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toBeTruthy();
	});

	it('rebuilds a missing local intent from the authenticated exact Submitted status', async () => {
		const submitted = view(12n, openRequest, { Submitted: null });
		const pulled = view(12n, openRequest, { PullConfirmed: { block_index: 88n } });
		const completed = view(12n, openRequest, {
			Completed: { result: { OpenVault: { vault_id: 42n } } }
		});
		const setup = adapter(
			{ acknowledged_through: 11n, active: [submitted] },
			{ acknowledged_through: 11n, active: [completed] },
			{ acknowledged_through: 11n, active: [pulled] }
		);

		const result = await recoverSubmittedVaultPullFromBlock({
			owner,
			blockIndex: 88n,
			adapter: setup.adapter
		});

		expect(result).toEqual({ vaultId: 42 });
		expect(setup.adapter.reconcileFromBlock).toHaveBeenCalledWith(12n, 88n);
		expect(setup.adapter.dispatch).toHaveBeenCalledWith(12n, openRequest);
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).toBeNull();
	});

	it('does not permit a repeated recovery after the saved intent is acknowledged', async () => {
		const submitted = view(9n, openRequest, { Submitted: null });
		const pulled = view(9n, openRequest, { PullConfirmed: { block_index: 77n } });
		const completed = view(9n, openRequest, {
			Completed: { result: { OpenVault: { vault_id: 42n } } }
		});
		const setup = adapter(
			{ acknowledged_through: 8n, active: [submitted] },
			{ acknowledged_through: 8n, active: [completed] },
			{ acknowledged_through: 8n, active: [pulled] }
		);
		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('submitted');
		await recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter });
		await expect(
			recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter })
		).rejects.toThrow('no active Submitted');
		expect(setup.adapter.reconcileFromBlock).toHaveBeenCalledTimes(1);
		expect(setup.adapter.dispatch).toHaveBeenCalledTimes(1);
	});

	it('rejects concurrent recovery while another tab holds the wallet lock', async () => {
		let release!: () => void;
		const submitted = view(9n, openRequest, { Submitted: null });
		const pulled = view(9n, openRequest, { PullConfirmed: { block_index: 77n } });
		const completed = view(9n, openRequest, {
			Completed: { result: { OpenVault: { vault_id: 42n } } }
		});
		const setup = adapter(
			{ acknowledged_through: 8n, active: [submitted] },
			{ acknowledged_through: 8n, active: [completed] },
			{ acknowledged_through: 8n, active: [pulled] }
		);
		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('submitted');
		vi.mocked(setup.adapter.reconcileFromBlock!).mockImplementationOnce(async () => {
			await new Promise<void>((resolve) => {
				release = resolve;
			});
			setup.setStatus({ acknowledged_through: 8n, active: [pulled] });
		});
		const first = recoverSubmittedVaultPullFromBlock({
			owner,
			blockIndex: 77n,
			adapter: setup.adapter
		});
		await vi.waitFor(() => expect(release).toBeTypeOf('function'));
		await expect(
			recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter })
		).rejects.toThrow('Another tab');
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
		await expect(
			runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
		).rejects.toThrow('submitted');
		let statusRead = 0;
		vi.mocked(setup.adapter.getStatus).mockImplementation(async () => {
			statusRead += 1;
			if (statusRead === 1) return { acknowledged_through: 8n, active: [submitted] };
			if (statusRead === 2) return { acknowledged_through: 8n, active: [pulled] };
			return { acknowledged_through: 9n, active: [] };
		});

		await expect(
			recoverSubmittedVaultPullFromBlock({ owner, blockIndex: 77n, adapter: setup.adapter })
		).rejects.toThrow('acknowledged during recovery');

		expect(setup.adapter.approve).not.toHaveBeenCalled();
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
		expect(setup.adapter.acknowledge).not.toHaveBeenCalled();
		expect(localStorage.getItem(`rumi:vault-pull-operation:v1:${owner}`)).not.toBeNull();
	});
});

describe('reviewed saved vault request recovery', () => {
	const key = `rumi:vault-pull-operation:v1:${owner}`;
	beforeEach(() => {
		localStorage.clear();
		Object.defineProperty(navigator, 'locks', { configurable: true, value: lockManager() });
	});
	async function review(
		setup: ReturnType<typeof adapter>,
		request: VaultPullRequest = openRequest
	) {
		const saved: VaultPullIntent = { version: 1, owner, operationId: '1', request, savedAt: 100 };
		localStorage.setItem(
			key,
			JSON.stringify(saved, (_k, v) => (typeof v === 'bigint' ? `${v}n` : v))
		);
		const result = await inspectVaultPullRecovery({ owner, adapter: setup.adapter });
		expect(result.intent).not.toBeNull();
		return result.intent!;
	}
	it('shows a mismatched saved request without mutating it or dispatching', async () => {
		const other: VaultPullRequest = {
			OpenVault: {
				amount_e8s: 999n,
				collateral_type: (openRequest as any).OpenVault.collateral_type
			}
		};
		const setup = adapter({
			acknowledged_through: 0n,
			active: [view(1n, other, { Prepared: null })]
		});
		setup.adapter.cancel = vi.fn(async () => 1n);
		const intent = await review(setup);
		expect(intent.request).toEqual(openRequest);
		await expect(
			cancelSavedVaultPullOperation({ owner, intent, adapter: setup.adapter })
		).rejects.toThrow('different arguments');
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
		expect(localStorage.getItem(key)).toBeTruthy();
	});
	it('retires a legacy unmarked intent only through one authoritative backend update', async () => {
		const setup = adapter({ acknowledged_through: 0n, active: [] });
		setup.adapter.cancel = vi.fn(async (id, request) => {
			expect(request).toEqual(openRequest);
			expect(localStorage.getItem(key)).toBeTruthy();
			return id;
		});
		const intent = await review(setup);
		expect(await cancelSavedVaultPullOperation({ owner, intent, adapter: setup.adapter })).toBe(1n);
		expect(setup.adapter.cancel).toHaveBeenCalledTimes(1);
		expect(setup.adapter.approve).not.toHaveBeenCalled();
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
		expect(localStorage.getItem(key)).toBeNull();
	});
	it('does not clear on empty status after a lost cancel reply, and never auto-retries', async () => {
		const setup = adapter({ acknowledged_through: 0n, active: [] });
		setup.adapter.cancel = vi.fn(async () => {
			throw new Error('lost cancel reply');
		});
		const intent = await review(setup);
		await expect(
			cancelSavedVaultPullOperation({ owner, intent, adapter: setup.adapter })
		).rejects.toThrow('lost cancel reply');
		expect(setup.adapter.cancel).toHaveBeenCalledTimes(1);
		expect(localStorage.getItem(key)).toBeTruthy();
		expect(setup.adapter.getStatus).toHaveBeenCalledTimes(3); // inspection + before update + once after loss
	});
	it('clears after a lost cancel reply only when its watermark is confirmed', async () => {
		const setup = adapter({ acknowledged_through: 0n, active: [] });
		setup.adapter.cancel = vi.fn(async () => {
			setup.setStatus({ acknowledged_through: 1n, active: [] });
			throw new Error('lost cancel reply');
		});
		const intent = await review(setup);
		expect(await cancelSavedVaultPullOperation({ owner, intent, adapter: setup.adapter })).toBe(1n);
		expect(setup.adapter.cancel).toHaveBeenCalledTimes(1);
		expect(localStorage.getItem(key)).toBeNull();
	});
	it('keeps replacement same-ID storage when a retirement update returns', async () => {
		const setup = adapter({ acknowledged_through: 0n, active: [] });
		setup.adapter.cancel = vi.fn(async () => {
			const current = JSON.parse(localStorage.getItem(key)!);
			current.savedAt = 101;
			localStorage.setItem(key, JSON.stringify(current));
			return 1n;
		});
		const intent = await review(setup);
		await expect(
			cancelSavedVaultPullOperation({ owner, intent, adapter: setup.adapter })
		).rejects.toThrow('changed after review');
		expect(JSON.parse(localStorage.getItem(key)!).savedAt).toBe(101);
	});
	it('rejects changed wallet/session or raw intent before clearing', async () => {
		const setup = adapter({ acknowledged_through: 0n, active: [] });
		let current = true;
		setup.adapter.assertCurrent = () => {
			if (!current) throw new Error('session changed');
		};
		setup.adapter.cancel = vi.fn(async () => {
			current = false;
			return 1n;
		});
		const intent = await review(setup);
		await expect(
			cancelSavedVaultPullOperation({ owner, intent, adapter: setup.adapter })
		).rejects.toThrow('session changed');
		expect(localStorage.getItem(key)).toBeTruthy();
	});
	it.each([
		{ Submitted: null },
		{ PullConfirmed: { block_index: 7n } },
		{ VaultCredited: { block_index: 7n } },
		{ Completed: { result: { OpenVault: { vault_id: 42n } } } }
	] as VaultPullView['phase'][])(
		'refuses cancellation of dispatched/positive/completed phase %#',
		async (phase) => {
			const setup = adapter({ acknowledged_through: 0n, active: [view(1n, openRequest, phase)] });
			setup.adapter.cancel = vi.fn(async () => 1n);
			const intent = await review(setup);
			await expect(
				cancelSavedVaultPullOperation({ owner, intent, adapter: setup.adapter })
			).rejects.toThrow('cannot be cancelled');
			expect(setup.adapter.cancel).not.toHaveBeenCalled();
		}
	);
	it('never allocates a fresh ID when the reviewed request is already acknowledged', async () => {
		const setup = adapter({ acknowledged_through: 0n, active: [] });
		const intent = await review(setup);
		setup.setStatus({ acknowledged_through: 1n, active: [] });
		await expect(
			resumeSavedVaultPullOperation({ owner, intent, adapter: setup.adapter })
		).rejects.toThrow('acknowledged during recovery');
		expect(setup.adapter.approve).not.toHaveBeenCalled();
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
		expect(localStorage.getItem(key)).toBeTruthy();
	});
	it('resumes the exact reviewed ID and raw bigint amounts through terminal ACK', async () => {
		const completed = view(1n, openRequest, {
			Completed: { result: { OpenVault: { vault_id: 42n } } }
		});
		const setup = adapter(
			{ acknowledged_through: 0n, active: [] },
			{ acknowledged_through: 0n, active: [completed] }
		);
		const intent = await review(setup);
		expect(await resumeSavedVaultPullOperation({ owner, intent, adapter: setup.adapter })).toEqual({
			vaultId: 42
		});
		expect(setup.adapter.dispatch).toHaveBeenCalledWith(1n, openRequest);
		expect(localStorage.getItem(key)).toBeNull();
	});
	it('does not dispatch a reviewed request replaced during approval', async () => {
		const setup = adapter({ acknowledged_through: 0n, active: [] });
		const intent = await review(setup);
		setup.adapter.approve = vi.fn(async () => {
			localStorage.setItem(key, localStorage.getItem(key)! + ' ');
			return { success: true };
		});
		await expect(
			resumeSavedVaultPullOperation({ owner, intent, adapter: setup.adapter })
		).rejects.toThrow('changed after review');
		expect(setup.adapter.dispatch).not.toHaveBeenCalled();
	});
	it('announces a verified save only after releasing the operation lock', async () => {
		const setup = adapter({ acknowledged_through: 0n, active: [] });
		setup.adapter.approve = vi.fn(async () => ({ success: false, error: 'approval declined' }));
		let discovery: ReturnType<typeof inspectVaultPullRecovery> | undefined;
		const listener = () => {
			discovery = inspectVaultPullRecovery({ owner, adapter: setup.adapter });
		};
		window.addEventListener('rumi:vault-pull-status-change', listener);
		try {
			await expect(
				runVaultPullOperation({ owner, request: openRequest, adapter: setup.adapter })
			).rejects.toThrow('approval declined');
			expect(discovery).toBeDefined();
			expect((await discovery!).intent?.request).toEqual(openRequest);
		} finally {
			window.removeEventListener('rumi:vault-pull-status-change', listener);
		}
	});
	it('keeps malformed v1 records held and does not replace them with empty recovery', async () => {
		const setup = adapter({ acknowledged_through: 0n, active: [] });
		localStorage.setItem(key, '{"version":1}');
		await expect(inspectVaultPullRecovery({ owner, adapter: setup.adapter })).rejects.toThrow(
			'could not be verified'
		);
		expect(localStorage.getItem(key)).toBe('{"version":1}');
	});
});
