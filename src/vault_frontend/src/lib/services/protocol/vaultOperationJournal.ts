/**
 * Browser-side intent journal for collateral-pulling vault operations.
 *
 * The backend journal remains authoritative. This record binds the wallet,
 * operation ID, and exact arguments before any approval or dispatch so reloads
 * can only resume the same request.
 */

export type VaultPullRequest =
	| { OpenVault: { amount_e8s: bigint; collateral_type: string } }
	| {
			OpenVaultAndBorrow: {
				amount_e8s: bigint;
				borrow_amount_e8s: bigint;
				collateral_type: string;
			};
	  }
	| { AddMargin: { vault_id: bigint; amount_e8s: bigint } };

export type VaultPullPhase =
	| { Prepared: null }
	| { Submitted: null }
	| { PullConfirmed: { block_index: bigint } }
	| { VaultCredited: { block_index: bigint } }
	| { SafeNoEffect: { message: string } }
	| { Completed: { result: unknown } };

export interface VaultPullView {
	operation_id: bigint;
	request: VaultPullRequest;
	vault_id: bigint;
	phase: VaultPullPhase;
}

export interface VaultPullStatus {
	active: [] | [VaultPullView];
	acknowledged_through: bigint;
}

export function normalizeVaultPullStatus(status: VaultPullStatus): VaultPullStatus {
	const view = status.active[0];
	if (!view) return status;
	let request = view.request as any;
	if (request.OpenVault) {
		request = {
			OpenVault: {
				...request.OpenVault,
				collateral_type: principalText(request.OpenVault.collateral_type)
			}
		};
	} else if (request.OpenVaultAndBorrow) {
		request = {
			OpenVaultAndBorrow: {
				...request.OpenVaultAndBorrow,
				collateral_type: principalText(request.OpenVaultAndBorrow.collateral_type)
			}
		};
	} else if (request.AddMargin) {
		request = { AddMargin: { ...request.AddMargin } };
	}
	return { ...status, active: [{ ...view, request }] };
}

function principalText(value: unknown): string {
	if (typeof value === 'string') return value;
	if (
		value &&
		typeof value === 'object' &&
		'toText' in value &&
		typeof (value as { toText?: unknown }).toText === 'function'
	) {
		return (value as { toText(): string }).toText();
	}
	throw new Error(
		'Backend vault operation status has an invalid collateral principal. The operation remains held.'
	);
}

export interface VaultPullIntent {
	version: 1;
	owner: string;
	operationId: string;
	request: VaultPullRequest;
	savedAt: number;
}

export type VaultPullOperation = 'open' | 'open_and_borrow' | 'add_margin';

export interface VaultPullAdapter<T> {
	currentOwner(): string | null;
	assertCurrent?(): void;
	getStatus(): Promise<VaultPullStatus>;
	approve(request: VaultPullRequest): Promise<{ success: boolean; error?: string }>;
	dispatch(operationId: bigint, request: VaultPullRequest): Promise<unknown>;
	beforeDispatch?(): void;
	completed(view: VaultPullView): T;
	acknowledge(operationId: bigint): Promise<void>;
	reconcileFromBlock?(operationId: bigint, blockIndex: bigint): Promise<void>;
}

const STORAGE_PREFIX = 'rumi:vault-pull-operation:v1:';
const NAT64_MAX = 18_446_744_073_709_551_615n;
export const VAULT_PULL_STATUS_EVENT = 'rumi:vault-pull-status-change';

function notifySubmittedVaultPull(owner: string): void {
	if (typeof window === 'undefined') return;
	window.dispatchEvent(new CustomEvent(VAULT_PULL_STATUS_EVENT, { detail: { owner } }));
	if (typeof BroadcastChannel !== 'undefined') {
		try {
			const channel = new BroadcastChannel(VAULT_PULL_STATUS_EVENT);
			channel.postMessage({ owner });
			channel.close();
		} catch {
			// Cross-tab notification is advisory; held-operation handling remains authoritative.
		}
	}
}

export function vaultPullIntentKey(owner: string): string {
	return `${STORAGE_PREFIX}${owner}`;
}

export function sameVaultPullRequest(a: VaultPullRequest, b: VaultPullRequest): boolean {
	return (
		JSON.stringify(a, (_key, value) => (typeof value === 'bigint' ? value.toString() : value)) ===
		JSON.stringify(b, (_key, value) => (typeof value === 'bigint' ? value.toString() : value))
	);
}

export function parseVaultPullIntent(raw: string | null): VaultPullIntent | null {
	if (!raw) return null;
	try {
		const value = JSON.parse(raw, (_key, item) => {
			if (typeof item === 'string' && /^\d+n$/.test(item)) return BigInt(item.slice(0, -1));
			return item;
		});
		if (
			value?.version !== 1 ||
			typeof value.owner !== 'string' ||
			typeof value.operationId !== 'string' ||
			!/^\d+$/.test(value.operationId) ||
			!value.request ||
			typeof value.request !== 'object'
		)
			return null;
		return value as VaultPullIntent;
	} catch {
		return null;
	}
}

export function hasCompatibleVaultPullIntent(owner: string, operationId: bigint): boolean {
	const intent = readIntent(owner);
	return intent === null || intent.operationId === operationId.toString();
}

function serializeIntent(intent: VaultPullIntent): string {
	return JSON.stringify(intent, (_key, value) => (typeof value === 'bigint' ? `${value}n` : value));
}

function readIntent(owner: string): VaultPullIntent | null {
	const storage = globalThis.localStorage;
	const raw = storage.getItem(vaultPullIntentKey(owner));
	if (!raw) return null;
	const intent = parseVaultPullIntent(raw);
	if (!intent || intent.owner !== owner) {
		throw new Error(
			'A saved vault operation could not be verified. Keep this wallet connected and contact support before retrying.'
		);
	}
	return intent;
}

function saveIntent(intent: VaultPullIntent): void {
	globalThis.localStorage.setItem(vaultPullIntentKey(intent.owner), serializeIntent(intent));
	const saved = readIntent(intent.owner);
	if (
		!saved ||
		saved.operationId !== intent.operationId ||
		!sameVaultPullRequest(saved.request, intent.request)
	) {
		throw new Error(
			'Could not persist the exact vault operation before approval. No approval was requested.'
		);
	}
}

function clearIntent(owner: string, operationId: bigint): void {
	const key = vaultPullIntentKey(owner);
	const current = readIntent(owner);
	if (current?.operationId === operationId.toString()) globalThis.localStorage.removeItem(key);
}

function getBrowserLockManager(): LockManager | undefined {
	if (typeof navigator === 'undefined') return undefined;
	return navigator.locks;
}

function validateActive(view: VaultPullView, intent: VaultPullIntent): void {
	if (
		view.operation_id.toString() !== intent.operationId ||
		!sameVaultPullRequest(view.request, intent.request)
	) {
		throw new Error(
			`Vault operation ${intent.operationId} is active with different arguments. It remains held; do not start another operation.`
		);
	}
}

function terminal(view: VaultPullView): boolean {
	return 'Completed' in view.phase || 'SafeNoEffect' in view.phase;
}

function heldMessage(view: VaultPullView): string {
	if ('Submitted' in view.phase) {
		return `Vault operation ${view.operation_id} is submitted with an unknown transfer outcome. It remains held. Find and verify the exact collateral ledger transfer block index, then use reconcile_vault_collateral_pull_from_block for this operation ID. Do not guess a block or start another vault operation.`;
	}
	return `Vault operation ${view.operation_id} is still ${Object.keys(view.phase)[0]}. It remains held; retry the exact saved operation after recovery.`;
}

async function withCrossTabLock<T>(owner: string, run: () => Promise<T>): Promise<T> {
	const locks = getBrowserLockManager();
	if (!locks) {
		throw new Error(
			'This browser cannot safely lock vault operations across tabs. Use a browser with Web Locks enabled before continuing.'
		);
	}
	return locks.request(
		`rumi:vault-collateral-pull:${owner}`,
		{ mode: 'exclusive', ifAvailable: true },
		(lock) => {
			if (!lock)
				throw new Error(
					'Another tab is already handling this wallet vault operation. Wait for it to finish, then refresh the vault view.'
				);
			return run();
		}
	);
}

/**
 * Execute or resume one exact backend-journaled collateral operation.
 * All callers for the same wallet share a Web Lock for the full approval,
 * dispatch, status, and ACK lifecycle.
 */
export async function runVaultPullOperation<T>(args: {
	owner: string;
	request: VaultPullRequest;
	adapter: VaultPullAdapter<T>;
}): Promise<T> {
	const { owner, request, adapter } = args;
	return withCrossTabLock(owner, () => runVaultPullOperationLocked(owner, request, adapter));
}

async function runVaultPullOperationLocked<T>(
	owner: string,
	request: VaultPullRequest,
	adapter: VaultPullAdapter<T>,
	expectedRecoveredOperationId?: bigint
): Promise<T> {
		const assertOwner = () => {
			adapter.assertCurrent?.();
			if (adapter.currentOwner() !== owner)
				throw new Error(
					'Wallet identity changed during the vault operation. The saved operation remains held for its original wallet.'
				);
		};
		assertOwner();

		let intent = readIntent(owner);
		const status = await adapter.getStatus();
		assertOwner();
		const active = status.active[0];

		if (intent) {
			const operationId = BigInt(intent.operationId);
			if (status.acknowledged_through >= operationId) {
				clearIntent(owner, operationId);
				if (expectedRecoveredOperationId === operationId) {
					throw new Error(
						`Vault operation ${operationId} was acknowledged during recovery. Refresh vault data before starting another request.`
					);
				}
				intent = null;
			} else if (active) {
				validateActive(active, intent);
			} else if (operationId !== status.acknowledged_through + 1n) {
				throw new Error(
					`Saved vault operation ${operationId} no longer matches backend sequence ${status.acknowledged_through}. It remains held for recovery.`
				);
			}
		}
		if (expectedRecoveredOperationId !== undefined) {
			if (!intent || BigInt(intent.operationId) !== expectedRecoveredOperationId || !active) {
				throw new Error(
					`Recovered vault operation ${expectedRecoveredOperationId} no longer has its active backend record. Its exact intent remains held.`
				);
			}
			validateActive(active, intent);
			if (
				!('PullConfirmed' in active.phase || 'VaultCredited' in active.phase || 'Completed' in active.phase)
			) {
				throw new Error(
					`Vault operation ${expectedRecoveredOperationId} is not in a verified post-pull phase. Its exact intent remains held.`
				);
			}
		}

		if (!intent) {
			if (status.acknowledged_through >= NAT64_MAX) {
				throw new Error(
					'The backend vault operation sequence is exhausted. No new operation was approved or dispatched.'
				);
			}
			if (active) {
				intent = {
					version: 1,
					owner,
					operationId: active.operation_id.toString(),
					request: active.request,
					savedAt: Date.now()
				};
			} else {
				intent = {
					version: 1,
					owner,
					operationId: (status.acknowledged_through + 1n).toString(),
					request,
					savedAt: Date.now()
				};
			}
			saveIntent(intent);
		}

		const operationId = BigInt(intent.operationId);
		let currentStatus = status;
		let currentView = currentStatus.active[0];
		if (currentView) validateActive(currentView, intent);
		if (!sameVaultPullRequest(request, intent.request)) {
			throw new Error(
				`Vault operation ${operationId} is still bound to different arguments. It remains held; restore the exact saved request before retrying. No approval or dispatch was made for this action.`
			);
		}

		if (currentView && terminal(currentView)) {
			if ('Completed' in currentView.phase) {
				const result = adapter.completed(currentView);
				await adapter.acknowledge(operationId);
				assertOwner();
				const acknowledged = await adapter.getStatus();
				assertOwner();
				if (acknowledged.acknowledged_through < operationId) {
					throw new Error(
						`Vault operation ${operationId} completed, but ACK is not yet confirmed. Its exact intent remains saved.`
					);
				}
				clearIntent(owner, operationId);
				return result;
			}
			// SafeNoEffect can be retried with the same ID and exact request. Do not
			// ACK it here, because doing so would make that exact retry stale.
		}

		if (currentView && 'Submitted' in currentView.phase) {
			notifySubmittedVaultPull(owner);
			throw new Error(heldMessage(currentView));
		}

		if (
			!currentView ||
			'Prepared' in currentView.phase ||
			'SafeNoEffect' in currentView.phase ||
			'PullConfirmed' in currentView.phase ||
			'VaultCredited' in currentView.phase
		) {
			if (!currentView || 'Prepared' in currentView.phase || 'SafeNoEffect' in currentView.phase) {
				assertOwner();
				const approval = await adapter.approve(intent.request);
				assertOwner();
				if (!approval.success) {
					throw new Error(
						approval.error ||
							'Collateral approval failed. The exact vault operation remains saved for safe retry.'
					);
				}
			}
			try {
				assertOwner();
				adapter.beforeDispatch?.();
				await adapter.dispatch(operationId, intent.request);
			} catch {
				// Always read the wallet-authenticated backend journal after a lost or
				// typed reply. Its phase, rather than transport behavior, controls the
				// next step and whether this intent can be retired.
			}
		}

		assertOwner();
		currentStatus = await adapter.getStatus();
		assertOwner();
		currentView = currentStatus.active[0];
		if (!currentView) {
			if (currentStatus.acknowledged_through >= operationId) {
				clearIntent(owner, operationId);
				throw new Error(
					`Vault operation ${operationId} was already acknowledged; refresh vault data before starting another request.`
				);
			}
			throw new Error(
				`Vault operation ${operationId} has no backend status yet. Its exact intent remains saved; retry only the same arguments.`
			);
		}
		validateActive(currentView, intent);

		if ('Submitted' in currentView.phase) notifySubmittedVaultPull(owner);
		if (!terminal(currentView)) throw new Error(heldMessage(currentView));
		if ('SafeNoEffect' in currentView.phase) {
			const message = currentView.phase.SafeNoEffect.message;
			await adapter.acknowledge(operationId);
			assertOwner();
			const acknowledged = await adapter.getStatus();
			assertOwner();
			if (acknowledged.acknowledged_through < operationId) {
				throw new Error(
					`Vault operation ${operationId} had no effect, but ACK is not confirmed. Its exact intent remains saved.`
				);
			}
			clearIntent(owner, operationId);
			throw new Error(message || `Vault operation ${operationId} completed with no effect.`);
		}

		const result = adapter.completed(currentView);
		await adapter.acknowledge(operationId);
		assertOwner();
		const acknowledged = await adapter.getStatus();
		assertOwner();
		if (acknowledged.acknowledged_through < operationId) {
			throw new Error(
				`Vault operation ${operationId} completed, but ACK is not yet confirmed. Its exact intent remains saved.`
			);
		}
		clearIntent(owner, operationId);
		return result;
	}

/** Reconcile one explicitly supplied candidate for the currently saved Submitted operation. */
export async function recoverSubmittedVaultPullFromBlock<T>(args: {
	owner: string;
	blockIndex: bigint;
	adapter: VaultPullAdapter<T>;
}): Promise<T> {
	const { owner, blockIndex, adapter } = args;
	if (blockIndex < 0n || blockIndex > NAT64_MAX) {
		throw new Error('Enter a nonnegative ledger block index within the supported range. The saved operation remains held.');
	}
	const reconcile = adapter.reconcileFromBlock;
	if (!reconcile) throw new Error('Exact-block recovery is unavailable. The saved operation remains held.');
	return withCrossTabLock(owner, async () => {
		const assertOwner = () => {
			adapter.assertCurrent?.();
			if (adapter.currentOwner() !== owner)
				throw new Error('Wallet identity changed during recovery. The saved operation remains held for its original wallet.');
		};
		assertOwner();
		let intent = readIntent(owner);
		const before = await adapter.getStatus();
		assertOwner();
		const active = before.active[0];
		if (!active || !('Submitted' in active.phase))
			throw new Error(`There is no active Submitted vault operation to recover. Refresh its backend status.`);
		if (!intent) {
			intent = {
				version: 1,
				owner,
				operationId: active.operation_id.toString(),
				request: active.request,
				savedAt: Date.now()
			};
			saveIntent(intent);
		}
		const operationId = BigInt(intent.operationId);
		validateActive(active, intent);
		let reconcileError: unknown;
		try {
			await reconcile(operationId, blockIndex);
		} catch (error) {
			reconcileError = error;
		}
		assertOwner();
		const verified = await adapter.getStatus();
		assertOwner();
		const recovered = verified.active[0];
		if (
			!recovered ||
			recovered.operation_id !== operationId ||
			!('PullConfirmed' in recovered.phase || 'VaultCredited' in recovered.phase || 'Completed' in recovered.phase)
		) {
			if (reconcileError) throw reconcileError;
			throw new Error(`Block ${blockIndex} did not move vault operation ${operationId} past Submitted. Its exact intent remains saved.`);
		}
		validateActive(recovered, intent);
		return runVaultPullOperationLocked(owner, intent.request, adapter, operationId);
	});
}
