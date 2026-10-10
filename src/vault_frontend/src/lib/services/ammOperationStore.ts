export type AmmMutation = 'swap' | 'add_liquidity';
export type AmmOperationState = 'prepared' | 'held' | 'unavailable' | 'complete';

export interface AmmCanonicalRequest {
	operation: AmmMutation;
	poolId: string;
	tokenIn?: string;
	amountIn?: string;
	minAmountOut?: string;
	amountA?: string;
	amountB?: string;
	minLpShares?: string;
}

export interface AmmOperation {
	requestId: number[];
	canisterId: string;
	walletPrincipal: string;
	poolId: string;
	request: AmmCanonicalRequest;
	state: AmmOperationState;
	createdAt: number;
	message?: string;
}

const STORAGE_KEY = 'rumi.amm.operations.v1';
const LOCK_PREFIX = 'rumi-amm-v2';
const MAX_SEQUENCE = (1n << 64n) - 1n;

type LockManagerLike = {
	request<T>(name: string, options: { mode: 'exclusive' }, callback: () => Promise<T>): Promise<T>;
};

function browserStorage(): Storage {
	if (typeof localStorage === 'undefined')
		throw new Error(
			'Persistent AMM operation storage is unavailable. No approval or transaction was started.'
		);
	return localStorage;
}

function requestKey(request: AmmCanonicalRequest): string {
	return JSON.stringify(request);
}

export class AmmOperationStore {
	constructor(
		private readonly storage?: Storage,
		private readonly injectedLocks?: LockManagerLike
	) {}

	list(canisterId: string, walletPrincipal: string): AmmOperation[] {
		return this.read().filter(
			(op) => op.canisterId === canisterId && op.walletPrincipal === walletPrincipal
		);
	}

	unresolved(canisterId: string, walletPrincipal: string): AmmOperation[] {
		return this.list(canisterId, walletPrincipal).filter((op) => op.state !== 'complete');
	}

	async prepareWithSequence(
		canisterId: string,
		walletPrincipal: string,
		request: AmmCanonicalRequest,
		fetchNextSequence: () => Promise<bigint>
	): Promise<AmmOperation> {
		const operations = this.read();
		const existing = operations.find(
			(op) =>
				op.canisterId === canisterId &&
				op.walletPrincipal === walletPrincipal &&
				op.poolId === request.poolId &&
				op.state !== 'complete'
		);
		if (existing) {
			if (existing.state === 'unavailable') {
				throw new Error(
					existing.message ??
						'The AMM operation result is unavailable and the request cannot be replayed. Verify balances and history before any further action.'
				);
			}
			if (requestKey(existing.request) !== requestKey(request)) {
				throw new Error(
					'An AMM operation for this pool is still held. Check its status and follow its recovery guidance before starting another.'
				);
			}
			return existing;
		}
		const sequence = await fetchNextSequence();
		if (sequence < 1n || sequence > MAX_SEQUENCE) {
			throw new Error('The AMM returned an invalid request sequence. No request was saved or submitted.');
		}
		const cryptoApi = globalThis.crypto;
		if (!cryptoApi?.getRandomValues)
			throw new Error(
				'Secure request ID generation is unavailable. No approval or transaction was started.'
			);
		const bytes = new Uint8Array(32);
		cryptoApi.getRandomValues(bytes.subarray(8));
		for (let i = 7; i >= 0; i -= 1) {
			bytes[i] = Number(sequence >> (BigInt(7 - i) * 8n) & 0xffn);
		}
		const operation: AmmOperation = {
			requestId: Array.from(bytes),
			canisterId,
			walletPrincipal,
			poolId: request.poolId,
			request,
			state: 'prepared',
			createdAt: Date.now()
		};
		operations.push(operation);
		this.write(operations);
		return operation;
	}

	update(
		requestId: number[],
		patch: Partial<Pick<AmmOperation, 'state' | 'message'>>
	): AmmOperation {
		const operations = this.read();
		const index = operations.findIndex((op) => sameId(op.requestId, requestId));
		if (index < 0) throw new Error('The saved AMM operation could not be found.');
		operations[index] = { ...operations[index], ...patch };
		this.write(operations);
		return operations[index];
	}

	withCallerLock<T>(
		canisterId: string,
		walletPrincipal: string,
		run: () => Promise<T>
	): Promise<T> {
		const lockManager =
			this.injectedLocks ??
			(globalThis.navigator as (Navigator & { locks?: LockManagerLike }) | undefined)?.locks;
		if (!lockManager)
			throw new Error(
				'This browser cannot coordinate AMM operations across tabs. Use a browser with Web Locks enabled.'
			);
		return lockManager.request(
			`${LOCK_PREFIX}:${canisterId}:${walletPrincipal}`,
			{ mode: 'exclusive' },
			run
		);
	}

	private read(): AmmOperation[] {
		try {
			const raw = (this.storage ?? browserStorage()).getItem(STORAGE_KEY);
			return raw ? (JSON.parse(raw) as AmmOperation[]) : [];
		} catch {
			throw new Error('Saved AMM operations could not be read. No new transaction was started.');
		}
	}

	private write(operations: AmmOperation[]): void {
		try {
			(this.storage ?? browserStorage()).setItem(STORAGE_KEY, JSON.stringify(operations));
		} catch {
			throw new Error(
				'The AMM operation could not be saved. No approval or transaction was started.'
			);
		}
	}
}

export async function prepareSupportedAmmOperation(
	store: AmmOperationStore,
	actor: Record<string, unknown>,
	method: string,
	canisterId: string,
	walletPrincipal: string,
	request: AmmCanonicalRequest,
	fetchNextSequence: () => Promise<bigint>,
	ready = true
): Promise<AmmOperation> {
	if (!ready) {
		throw new Error(
			`AMM request-receipt endpoint ${method} is not ready. No approval or AMM mutation was started.`
		);
	}
	if (typeof actor?.[method] !== 'function') {
		throw new Error(
			`AMM request-receipt endpoint ${method} is unavailable. No approval or AMM mutation was started.`
		);
	}
	return store.prepareWithSequence(canisterId, walletPrincipal, request, fetchNextSequence);
}

export function markUnavailableAfterSequenceConflict(
	store: AmmOperationStore,
	requestId: number[],
	err: any
): boolean {
	const reason = err?.InvalidInput?.reason;
	if (reason !== 'stale request sequence; result unavailable and this ID cannot execute again (its terminal row may have been compacted)')
		return false;
	store.update(requestId, {
		state: 'unavailable',
		message:
			'The authenticated AMM response says this request ID cannot execute again, but its result is unavailable. The terminal record may have been compacted or another caller may have consumed the global sequence, so the result is ambiguous and cannot be replayed. Do not create a replacement. Verify token balances and transaction history manually.'
	});
	return true;
}

export function releaseDefinitelyUnstartedSequenceCollision(
	store: AmmOperationStore,
	requestId: number[],
	err: any
): boolean {
	const reason = err?.InvalidInput?.reason;
	if (
		reason !==
		'AMM request sequence is bound to another request; this request was definitely not started and no input transfer was dispatched'
	)
		return false;
	store.update(requestId, {
		state: 'complete',
		message:
			'The authenticated AMM response proves this request was not started and no input transfer was dispatched. You may retry this form manually to fetch the current sequence; no replacement request is created automatically.'
	});
	return true;
}

export function retainAfterStatusErr(store: AmmOperationStore, requestId: number[]): AmmOperation {
	return store.update(requestId, {
		message:
			'Status query returned Err. The saved request is still retained; do not create a new request ID.'
	});
}

export function applyInboundOperationStatus(
	store: AmmOperationStore,
	requestId: number[],
	result: any
): AmmOperation {
	if (result && 'Err' in result) {
		const reason = result.Err?.InvalidInput?.reason;
		if (releaseDefinitelyUnstartedSequenceCollision(store, requestId, result.Err)) {
			return store.update(requestId, {
				message:
					'The authenticated AMM status proves this request was not started and no input transfer was dispatched. You may retry this form manually to fetch the current sequence; no replacement request is created automatically.'
			});
		}
		if (
			reason ===
			'stale request sequence; result unavailable and this ID cannot execute again (its terminal row may have been compacted)' ||
			reason === 'historical result unavailable; operation will not be re-executed'
		) {
			return store.update(requestId, {
				state: 'unavailable',
				message:
					'The AMM reports that this request cannot execute again, but its result is unavailable and cannot be replayed. Do not create a replacement; verify token balances and transaction history manually.'
			});
		}
		return retainAfterStatusErr(store, requestId);
	}
	const phase = (result?.Ok?.operation ?? result?.Ok)?.phase;
	if (phase && 'ResultUnavailable' in phase) {
		const operation = result?.Ok?.operation ?? result?.Ok;
		const heldReason = operation?.held_reason?.[0];
		return store.update(requestId, {
			state: 'unavailable',
			message:
				`The AMM reports that this result is unavailable and the request cannot be replayed. ${heldReason ?? 'Verify token balances and transaction history manually.'} Do not create a replacement request.`
		});
	}
	if (phase && 'Completed' in phase) {
		return store.update(requestId, {
			state: 'complete',
			message: 'Canister reported the operation completed.'
		});
	}
	if (phase && 'ProvenNoEffect' in phase) {
		return store.update(requestId, {
			state: 'complete',
			message: 'Canister proved the request had no effect; the pool request slot is released.'
		});
	}
	return store.update(requestId, {
		state: 'held',
		message: 'Canister status is nonterminal. Keep this request ID and continue recovery.'
	});
}

function sameId(a: number[], b: number[]): boolean {
	return a.length === b.length && a.every((byte, index) => byte === b[index]);
}

export const ammOperationStore = new AmmOperationStore();
