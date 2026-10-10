import { describe, expect, it } from 'vitest';
import {
	AmmOperationStore,
	applyInboundOperationStatus,
	prepareSupportedAmmOperation,
	markUnavailableAfterSequenceConflict,
	releaseDefinitelyUnstartedSequenceCollision
} from './ammOperationStore';
import { isSameAmmWallet } from './ammService';

class MemoryStorage implements Storage {
	private values = new Map<string, string>();
	failWrites = false;
	get length() {
		return this.values.size;
	}
	clear() {
		this.values.clear();
	}
	getItem(key: string) {
		return this.values.get(key) ?? null;
	}
	key(index: number) {
		return [...this.values.keys()][index] ?? null;
	}
	removeItem(key: string) {
		this.values.delete(key);
	}
	setItem(key: string, value: string) {
		if (this.failWrites) throw new Error('quota');
		this.values.set(key, value);
	}
}

const request = {
	operation: 'swap' as const,
	poolId: 'ICP-3USD',
	tokenIn: 'aaaaa-aa',
	amountIn: '100',
	minAmountOut: '90'
};

describe('AMM operation persistence', () => {
	it('fails before an operation can be dispatched when storage fails', async () => {
		const storage = new MemoryStorage();
		storage.failWrites = true;
		const store = new AmmOperationStore(storage);
		await expect(store.prepareWithSequence('amm', 'wallet', request, async () => 1n)).rejects.toThrow(/could not be saved/i);
	});

	it('does not persist a stuck row when the v2 endpoint is unavailable', async () => {
		const store = new AmmOperationStore(new MemoryStorage());
		await expect(
			prepareSupportedAmmOperation(store, {}, 'swap_v2', 'amm', 'wallet', request, async () => 1n)
		).rejects.toThrow(/endpoint swap_v2 is unavailable/i);
		expect(store.unresolved('amm', 'wallet')).toEqual([]);
	});

	it('does not persist an operation when sequence lookup fails', async () => {
		const store = new AmmOperationStore(new MemoryStorage());
		await expect(
			store.prepareWithSequence('amm', 'wallet', request, async () => {
				throw new Error('sequence unavailable');
			})
		).rejects.toThrow(/sequence unavailable/i);
		expect(store.unresolved('amm', 'wallet')).toEqual([]);
	});

	it('does not persist an add-liquidity row until explicit readiness is enabled', async () => {
		const store = new AmmOperationStore(new MemoryStorage());
		const actor = { add_liquidity_v2: async () => ({ Err: {} }) };
		await expect(
			prepareSupportedAmmOperation(
				store,
				actor,
				'add_liquidity_v2',
				'amm',
				'wallet',
				{ ...request, operation: 'add_liquidity', amountA: '10', amountB: '20' },
				async () => 1n,
				false
			)
		).rejects.toThrow(/not ready/i);
		expect(store.unresolved('amm', 'wallet')).toEqual([]);
	});

	it('survives reload and reuses the exact same request ID for the exact request', async () => {
		const storage = new MemoryStorage();
		const firstStore = new AmmOperationStore(storage);
		const first = await firstStore.prepareWithSequence('amm', 'wallet', request, async () => 17n);
		const afterReload = new AmmOperationStore(storage);
		expect(afterReload.list('amm', 'wallet')).toEqual([first]);
		const shouldNotFetch = async () => { throw new Error('sequence should not be fetched'); };
		expect((await afterReload.prepareWithSequence('amm', 'wallet', request, shouldNotFetch)).requestId).toEqual(first.requestId);
		await expect(afterReload.prepareWithSequence('amm', 'wallet', { ...request, amountIn: '101' }, shouldNotFetch)).rejects.toThrow(/still held/i);
		expect(first.requestId.slice(0, 8)).toEqual([0, 0, 0, 0, 0, 0, 0, 17]);
		expect(first.requestId.slice(8)).toHaveLength(24);
	});

	it('serializes work for the same canister, wallet, and pool across tabs', async () => {
		let occupied = false;
		const locks = {
			async request<T>(
				_name: string,
				_options: { mode: 'exclusive' },
				callback: () => Promise<T>
			): Promise<T> {
				if (occupied) throw new Error('lock held');
				occupied = true;
				try {
					return await callback();
				} finally {
					occupied = false;
				}
			}
		};
		const store = new AmmOperationStore(new MemoryStorage(), locks);
		let unblock!: () => void;
		const gate = new Promise<void>((resolve) => {
			unblock = resolve;
		});
		const active = store.withCallerLock('amm', 'wallet', async () => gate);
		await expect(
			store.withCallerLock('amm', 'wallet', async () => undefined)
		).rejects.toThrow(/lock held/);
		unblock();
		await active;
	});

	it('fences wallet changes after async boundaries', () => {
		expect(isSameAmmWallet(true, 'wallet-a', 'wallet-a')).toBe(true);
		expect(isSameAmmWallet(true, 'wallet-b', 'wallet-a')).toBe(false);
		expect(isSameAmmWallet(false, 'wallet-a', 'wallet-a')).toBe(false);
	});

	it('retains status Err and releases only completed or proven-no-effect requests', async () => {
		const store = new AmmOperationStore(new MemoryStorage());
		const held = await store.prepareWithSequence('amm', 'wallet', request, async () => 1n);
		store.update(held.requestId, { state: 'held' });

		applyInboundOperationStatus(store, held.requestId, { Err: { InvalidInput: null } });
		expect(store.unresolved('amm', 'wallet')).toHaveLength(1);
		applyInboundOperationStatus(store, held.requestId, { Err: { InvalidInput: { reason: 'stale request sequence; result unavailable and this ID cannot execute again (its terminal row may have been compacted)' } } });
		expect(store.unresolved('amm', 'wallet')).toHaveLength(1);
		expect(store.list('amm', 'wallet')[0].state).toBe('unavailable');
		expect(store.list('amm', 'wallet')[0].message).toMatch(/result is unavailable/i);
		await expect(store.prepareWithSequence('amm', 'wallet', request, async () => 9n)).rejects.toThrow(/cannot be replayed/i);

		applyInboundOperationStatus(store, held.requestId, {
			Ok: { phase: { Completed: null } }
		});
		expect(store.unresolved('amm', 'wallet')).toHaveLength(0);

		const noEffect = await store.prepareWithSequence('amm', 'wallet', request, async () => 2n);
		applyInboundOperationStatus(store, noEffect.requestId, {
			Ok: { operation: { phase: { ProvenNoEffect: null } } }
		});
		expect(store.unresolved('amm', 'wallet')).toHaveLength(0);
	});

	it('marks ambiguous stale sequence errors unavailable without replay or replacement', async () => {
		const store = new AmmOperationStore(new MemoryStorage());
		const held = await store.prepareWithSequence('amm', 'wallet', request, async () => 5n);
		store.update(held.requestId, { state: 'held' });
		expect(markUnavailableAfterSequenceConflict(store, held.requestId, { InvalidInput: { reason: 'other' } })).toBe(false);
		expect(store.unresolved('amm', 'wallet')).toHaveLength(1);
		expect(markUnavailableAfterSequenceConflict(store, held.requestId, { InvalidInput: { reason: 'stale request sequence; result unavailable and this ID cannot execute again (its terminal row may have been compacted)' } })).toBe(true);
		expect(store.unresolved('amm', 'wallet')).toHaveLength(1);
		expect(store.list('amm', 'wallet')[0].state).toBe('unavailable');
		expect(store.list('amm', 'wallet')[0].message).toMatch(/cannot execute again.*result is unavailable/i);
		await expect(store.prepareWithSequence('amm', 'wallet', request, async () => 6n)).rejects.toThrow(/cannot be replayed/i);
		await expect(store.prepareWithSequence('amm', 'wallet', { ...request, amountIn: '101' }, async () => 7n)).rejects.toThrow(/cannot be replayed/i);
	});

	it('releases only an authenticated retained-owner collision that proves no input transfer', async () => {
		const store = new AmmOperationStore(new MemoryStorage());
		const held = await store.prepareWithSequence('amm', 'wallet', request, async () => 10n);
		store.update(held.requestId, { state: 'held' });
		const definiteNoStart = {
			InvalidInput: {
				reason: 'AMM request sequence is bound to another request; this request was definitely not started and no input transfer was dispatched'
			}
		};
		expect(releaseDefinitelyUnstartedSequenceCollision(store, held.requestId, { InvalidInput: { reason: 'different Err' } })).toBe(false);
		expect(releaseDefinitelyUnstartedSequenceCollision(store, held.requestId, definiteNoStart)).toBe(true);
		expect(store.unresolved('amm', 'wallet')).toHaveLength(0);
		expect(store.list('amm', 'wallet')[0].message).toMatch(/no input transfer was dispatched/i);
		const next = await store.prepareWithSequence('amm', 'wallet', request, async () => 11n);
		expect(next.requestId.slice(0, 8)).toEqual([0, 0, 0, 0, 0, 0, 0, 11]);
		const statusHeld = await store.prepareWithSequence('amm', 'wallet', request, async () => 12n);
		const status = applyInboundOperationStatus(store, statusHeld.requestId, { Err: definiteNoStart });
		expect(status.state).toBe('complete');
		expect(status.message).toMatch(/status proves.*no input transfer/i);
	});

	it('keeps ResultUnavailable terminal and blocks replay using the backend held reason', async () => {
		const store = new AmmOperationStore(new MemoryStorage());
		const held = await store.prepareWithSequence('amm', 'wallet', request, async () => 7n);
		const result = applyInboundOperationStatus(store, held.requestId, {
			Ok: {
				operation: {
					phase: { ResultUnavailable: null },
					held_reason: ['payout history was retired']
				}
			}
		});
		expect(result.state).toBe('unavailable');
		expect(result.message).toMatch(/payout history was retired/);
		await expect(store.prepareWithSequence('amm', 'wallet', request, async () => 8n)).rejects.toThrow(/cannot be replayed/i);
		expect(store.unresolved('amm', 'wallet')).toHaveLength(1);
	});
});
