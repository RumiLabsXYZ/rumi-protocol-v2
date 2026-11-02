import type { Principal } from '@dfinity/principal';
import { CONFIG } from '../config';
import type { IntentStore } from './cksolMinter';

type Mode = 'icp' | 'cycles';
// Existing public receipt contract: the backend preserves this prefix through
// refund errors and completion while its interrupted minter call remains held.
const INTERRUPTED_MESSAGE = 'Processing was interrupted by an upgrade; SOL status remains unknown';
export function cksolIcpNeedsReconciliation(receipt: any): boolean {
	return (
		typeof receipt?.message?.[0] === 'string' && receipt.message[0].startsWith(INTERRUPTED_MESSAGE)
	);
}
const terminal = (phase: unknown) => phase === 'Complete' || phase === 'FundingRejected';
function other(mode: Mode, owner: string) {
	return mode === 'icp'
		? { label: 'cycles', key: `rumi:cksol:paid:v1:${owner}`, method: 'get_cksol_payment' }
		: {
				label: 'ICP',
				key: `rumi:cksol:icp:v1:${CONFIG.currentCanisterId}:${owner}`,
				method: 'get_cksol_icp_payment'
			};
}
/** Must run inside the same owner Web Lock used by both payment methods. */
export function assertOtherCksolLocalSettled(storage: IntentStore, owner: string, mode: Mode) {
	const { label, key } = other(mode, owner);
	const raw = storage.getItem(key);
	if (!raw) return;
	try {
		const p = JSON.parse(raw);
		if (p.owner !== owner || !/^[1-9][0-9]*$/.test(p.sequence) || !Number.isFinite(p.createdAt))
			throw new Error();
		if (p.phase === 'NotStarted') return;
		if (
			p.phase === 'Receipt' &&
			!cksolIcpNeedsReconciliation(p.receipt) &&
			terminal(p.receipt?.phase) &&
			String(p.receipt.sequence) === p.sequence
		)
			return;
	} catch {
		/* Damaged records stay held; they do not prove no payment exists. */
	}
	throw new Error(
		`Resolve the saved ${label} payment before using another payment method. Its record is preserved.`
	);
}
export async function requireOtherCksolPaymentSettled(
	storage: IntentStore,
	owner: Principal,
	mode: Mode,
	backend: any
) {
	assertOtherCksolLocalSettled(storage, owner.toText(), mode);
	const { label, method } = other(mode, owner.toText());
	if (typeof backend[method] === 'function') {
		const p = (await backend[method](owner))?.[0];
		const phase = typeof p?.phase === 'string' ? p.phase : Object.keys(p?.phase ?? {})[0];
		if (p && (!terminal(phase) || cksolIcpNeedsReconciliation(p)))
			throw new Error(
				`Resolve the existing ${label} payment receipt before using another payment method.`
			);
	}
	assertOtherCksolLocalSettled(storage, owner.toText(), mode);
}
