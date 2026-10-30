import { Actor, HttpAgent } from '@dfinity/agent';
import { IDL } from '@dfinity/candid';
import { Principal } from '@dfinity/principal';
import { get } from 'svelte/store';
import { CONFIG, CANISTER_IDS } from '../config';
import { ICRC1_IDL } from '../idls/ledger.idl.js';
import { walletStore } from '../stores/wallet';
import { auth, walletSessionGeneration } from './auth';
import { getOisySignerAgent, createOisyActor } from './oisySigner';
import { getCksolRelay, type IntentStore } from './cksolMinter';

export type PaymentPhase =
	| 'FundingPending'
	| 'FundingUnknown'
	| 'Funded'
	| 'DepositPending'
	| 'RefundReady'
	| 'RefundPending'
	| 'RefundUnknown'
	| 'Complete'
	| 'FundingRejected';
export interface CksolPayment {
	sequence: bigint;
	created_at_time: bigint;
	phase: PaymentPhase;
	funding_block: [] | [bigint];
	deposit_id: [] | [bigint];
	refundable_cycles: bigint;
	refund_block: [] | [bigint];
	message: [] | [string];
}
export interface CksolPaymentQuote {
	cycles_ledger: Principal;
	next_sequence: bigint;
	funding_cycles: bigint;
	processing_fee: bigint;
	ledger_fee: bigint;
	approval_amount: bigint;
	refund_fee_reserve: bigint;
	balance: bigint;
	allowance: bigint;
	owner: string;
	walletType: string;
	sessionGeneration: number;
	quotedAt: number;
}
export interface CksolPaymentIntent {
	owner: string;
	sequence: string;
	maxPayment?: string;
	approvalAmount?: string;
	phase:
		| 'NotStarted'
		| 'Approving'
		| 'ApprovalUnknown'
		| 'Approved'
		| 'Starting'
		| 'StartUnknown'
		| 'Receipt';
	approvalBlock?: string;
	createdAt: number;
	message?: string;
	receipt?: any;
}

let anonymousBackend: any;
const STALE_PAYMENT_SEQUENCE_ERROR = 'Refresh the ckSOL payment sequence';
// PostMessageTransport rejects this before opening a channel or sending a call.
const UNSENT_OISY_CLICK_ERROR = 'Error: Signer window should not be opened outside of click handler, see: https://github.com/slide-computer/signer-js/blob/main/packages/signer-web/README.md#channels-must-be-established-in-a-click-handler';
const PaymentPhaseIDL = IDL.Variant({
	FundingPending: IDL.Null,
	FundingUnknown: IDL.Null,
	Funded: IDL.Null,
	DepositPending: IDL.Null,
	RefundReady: IDL.Null,
	RefundPending: IDL.Null,
	RefundUnknown: IDL.Null,
	Complete: IDL.Null,
	FundingRejected: IDL.Null
});
const PaymentIDL = IDL.Record({
	sequence: IDL.Nat64,
	created_at_time: IDL.Nat64,
	phase: PaymentPhaseIDL,
	funding_block: IDL.Opt(IDL.Nat),
	deposit_id: IDL.Opt(IDL.Nat64),
	refundable_cycles: IDL.Nat64,
	refund_block: IDL.Opt(IDL.Nat),
	message: IDL.Opt(IDL.Text)
});
export const PaidBackendIDL = ({ IDL }: any) =>
	IDL.Service({
		get_cksol_payment_quote: IDL.Func(
			[],
			[
				IDL.Record({
					cycles_ledger: IDL.Principal,
					next_sequence: IDL.Nat64,
					funding_cycles: IDL.Nat64,
					processing_fee: IDL.Nat64,
					ledger_fee: IDL.Nat64,
					approval_amount: IDL.Nat64,
					refund_fee_reserve: IDL.Nat64
				})
			],
			['query']
		),
		get_cksol_payment: IDL.Func([IDL.Principal], [IDL.Opt(PaymentIDL)], ['query']),
		start_cksol_deposit_paid: IDL.Func(
			[IDL.Nat64, IDL.Nat64],
			[IDL.Variant({ Ok: PaymentIDL, Err: IDL.Text })],
			[]
		),
		claim_cksol_cycle_refund: IDL.Func(
			[IDL.Nat64],
			[IDL.Variant({ Ok: PaymentIDL, Err: IDL.Text })],
			[]
		)
	});
function mainnetAgent() {
	if (CONFIG.isLocal) throw new Error('ckSOL paid processing is available on ICP mainnet only.');
	return new HttpAgent({ host: CONFIG.host });
}
function backendActor() {
	return (anonymousBackend ??= Actor.createActor(PaidBackendIDL, {
		agent: mainnetAgent(),
		canisterId: CONFIG.currentCanisterId
	}));
}
function cyclesActor(id: Principal) {
	return Actor.createActor(ICRC1_IDL as any, { agent: mainnetAgent(), canisterId: id });
}
function isLive(owner: Principal, generation: number, walletType: string, live: () => boolean) {
	const w = get(walletStore),
		a = get(auth);
	return (
		live() &&
		get(walletSessionGeneration) === generation &&
		!w.loading &&
		w.isConnected &&
		w.principal?.toText() === owner.toText() &&
		a.isConnected &&
		a.account?.owner?.toText() === owner.toText() &&
		a.walletType === walletType
	);
}
async function verifySigner(actor: any, owner: Principal) {
	const agent = Actor.agentOf(actor);
	if (!agent || (await agent.getPrincipal()).toText() !== owner.toText())
		throw new Error(
			'Wallet signer principal differs from the connected ICP account. Reconnect before continuing.'
		);
}
export function cksolPaymentKey(owner: string) {
	return `rumi:cksol:paid:v1:${owner}`;
}
export function saveCksolPayment(storage: IntentStore, intent: CksolPaymentIntent) {
	storage.setItem(
		cksolPaymentKey(intent.owner),
		JSON.stringify(intent, (_key, value) => (typeof value === 'bigint' ? value.toString() : value))
	);
}
export function loadCksolPayment(storage: IntentStore, owner: string): CksolPaymentIntent | null {
	const raw = storage.getItem(cksolPaymentKey(owner));
	if (!raw) return null;
	const d = JSON.parse(raw);
	if (
		d.owner !== owner ||
		!/^[1-9][0-9]*$/.test(d.sequence) ||
		![
			'NotStarted',
			'Approving',
			'ApprovalUnknown',
			'Approved',
			'Starting',
			'StartUnknown',
			'Receipt'
		].includes(d.phase) ||
		!Number.isFinite(d.createdAt)
	)
		throw new Error('Saved ckSOL payment intent is invalid. Preserve it for support.');
	return d;
}
export async function quoteCksolPayment(
	owner: Principal,
	walletType: string
): Promise<CksolPaymentQuote> {
	if (CONFIG.isLocal || !owner || owner.isAnonymous())
		throw new Error('Connect an ICP wallet on mainnet first.');
	const generation = get(walletSessionGeneration),
		wallet = get(walletStore);
	if (wallet.principal?.toText() !== owner.toText() || !wallet.isConnected || wallet.loading)
		throw new Error('Wallet session changed.');
	const backend: any = backendActor();
	if (typeof backend.get_cksol_payment_quote !== 'function')
		throw new Error('Paid ckSOL processing is not enabled on this backend.');
	const raw = await backend.get_cksol_payment_quote();
	if (Principal.from(raw.cycles_ledger).toText() !== CANISTER_IDS.CYCLES_LEDGER)
		throw new Error(
			'Backend cycles ledger does not match the configured ICP mainnet cycles ledger. Do not approve.'
		);
	if (
		BigInt(raw.processing_fee) !== 1_000_000_000n ||
		BigInt(raw.funding_cycles) !== 1_000_000_000_000n + BigInt(raw.processing_fee) ||
		BigInt(raw.ledger_fee) !== 100_000_000n ||
		BigInt(raw.approval_amount) !== BigInt(raw.funding_cycles) + BigInt(raw.ledger_fee) ||
		BigInt(raw.refund_fee_reserve) !== 100_000_000n
	)
		throw new Error(
			'Backend payment quote differs from the reviewed fee schedule. Do not approve.'
		);
	const cyclesLedger = Principal.from(raw.cycles_ledger);
	const ledger: any = cyclesActor(cyclesLedger),
		account = { owner, subaccount: [] };
	const spender = { owner: Principal.fromText(CONFIG.currentCanisterId), subaccount: [] };
	const [balance, allowance, fee] = await Promise.all([
		ledger.icrc1_balance_of(account),
		ledger.icrc2_allowance({ account, spender }),
		ledger.icrc1_fee()
	]);
	if (BigInt(raw.ledger_fee) !== BigInt(fee))
		throw new Error('Cycles ledger fee differs from the backend quote. Refresh the quote.');
	const nextSequence = BigInt(raw.next_sequence);
	if (nextSequence < 1n)
		throw new Error('Backend returned an invalid ckSOL payment sequence. Refresh the quote.');
	return {
		cycles_ledger: cyclesLedger,
		next_sequence: nextSequence,
		funding_cycles: BigInt(raw.funding_cycles),
		processing_fee: BigInt(raw.processing_fee),
		ledger_fee: BigInt(fee),
		approval_amount: BigInt(raw.approval_amount),
		refund_fee_reserve: BigInt(raw.refund_fee_reserve),
		balance: BigInt(balance),
		allowance: BigInt(allowance.allowance),
		owner: owner.toText(),
		walletType,
		sessionGeneration: generation,
		quotedAt: Date.now()
	};
}
function phaseName(value: any): PaymentPhase {
	return (typeof value === 'string' ? value : Object.keys(value ?? {})[0]) as PaymentPhase;
}
export async function readCksolPayment(owner: Principal) {
	const actor: any = backendActor();
	if (typeof actor.get_cksol_payment !== 'function')
		throw new Error('Paid ckSOL receipt lookup is not enabled on this backend.');
	const r = await actor.get_cksol_payment(owner);
	const p = r?.[0] ?? null;
	return p ? { ...p, phase: phaseName(p.phase) } : null;
}
export async function recoverCksolPayment(owner: Principal, storage: IntentStore) {
	let saved: CksolPaymentIntent | null = null,
		invalidRaw: string | null = null;
	try {
		saved = loadCksolPayment(storage, owner.toText());
	} catch (error) {
		invalidRaw = storage.getItem(cksolPaymentKey(owner.toText()));
		if (!invalidRaw) throw error;
	}
	const originalRaw = storage.getItem(cksolPaymentKey(owner.toText()));
	const generation = get(walletSessionGeneration);
	const receipt = await readCksolPayment(owner);
	if (!receipt) {
		// Recover only this known pre-send UI failure. Lost replies and all other
		// unknown outcomes remain held. Keep the original journal for support.
		if (saved?.phase === 'ApprovalUnknown' && saved.approvalBlock === undefined &&
			saved.message === UNSENT_OISY_CLICK_ERROR && get(auth).walletType === 'oisy') {
			try {
				const q = await quoteCksolPayment(owner, 'oisy');
				if (q.allowance === 0n && q.next_sequence === BigInt(saved.sequence) &&
					isLive(owner, generation, 'oisy', () => true) &&
					storage.getItem(cksolPaymentKey(saved.owner)) === originalRaw) {
					storage.setItem(`${cksolPaymentKey(saved.owner)}:unsent-approval:${saved.sequence}:${saved.createdAt}`, originalRaw!);
					const recovered: CksolPaymentIntent = { ...saved, phase: 'NotStarted',
						message: 'OISY blocked the approval before sending it. No cycles allowance or backend payment was found. You can review the fee and retry.' };
					saveCksolPayment(storage, recovered);
					return recovered;
				}
			} catch { /* Preserve the held journal if any reconciliation check fails. */ }
		}
		return saved;
	}
	if (saved && BigInt(receipt.sequence) === BigInt(saved.sequence)) {
		saved.phase = 'Receipt';
		saved.receipt = receipt;
		saveCksolPayment(storage, saved);
		return saved;
	}
	if (
		saved &&
		['Approving', 'ApprovalUnknown', 'Approved', 'Starting', 'StartUnknown'].includes(saved.phase)
	)
		return saved;
	const recovered: CksolPaymentIntent = {
		owner: owner.toText(),
		sequence: BigInt(receipt.sequence).toString(),
		phase: 'Receipt',
		createdAt: Date.now(),
		receipt
	};
	if (invalidRaw)
		storage.setItem(`${cksolPaymentKey(owner.toText())}:invalid:${Date.now()}`, invalidRaw);
	try {
		saveCksolPayment(storage, recovered);
	} catch {
		/* durable backend receipt remains available to this view */
	}
	return recovered;
}
async function requireNoLegacyDeposit(
	owner: Principal,
	generation: number,
	walletType: string,
	live: () => boolean
) {
	const result = await getCksolRelay().get_cksol_deposit(owner);
	if (!isLive(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	const attempt = result?.[0];
	if (!attempt) return;
	const status = attempt.status ?? {};
	if ('Rejected' in status)
		throw new Error(
			'A rejected legacy deposit attempt must be acknowledged before a paid deposit can start.'
		);
	if ('Queued' in status)
		throw new Error(
			`Legacy deposit ${status.Queued} is still retained. Resolve and acknowledge it before paid processing.`
		);
	if ('SubmittedOrUnknown' in status)
		throw new Error(
			'A legacy deposit has an unknown outcome. Keep it for reconciliation; paid processing is blocked.'
		);
	throw new Error('Legacy deposit status is unrecognized. Paid processing is blocked.');
}
export async function submitCksolPayment(
	q: CksolPaymentQuote,
	owner: Principal,
	storage: IntentStore,
	live: () => boolean
): Promise<CksolPaymentIntent> {
	const walletType = get(auth).walletType ?? 'unknown';
	if (
		q.owner !== owner.toText() ||
		q.walletType !== walletType ||
		q.sessionGeneration !== get(walletSessionGeneration) ||
		!isLive(owner, q.sessionGeneration, walletType, live)
	)
		throw new Error('Wallet session changed. Refresh before approving.');
	const key = cksolPaymentKey(q.owner);
	let intent = loadCksolPayment(storage, q.owner);
	const backend: any = backendActor();
	// Resolve the backend-owned sequence before deciding whether another payment can start.
	let previous: CksolPayment | null = null;
	try {
		previous = await readCksolPayment(owner);
	} catch {
		throw new Error('Could not read the payment receipt. Do not approve or start another payment.');
	}
	if (!isLive(owner, q.sessionGeneration, walletType, live))
		throw new Error('Wallet session changed.');
	if (
		intent &&
		['Approving', 'ApprovalUnknown', 'Approved', 'Starting', 'StartUnknown'].includes(intent.phase)
	) {
		if (previous && BigInt(previous.sequence) === BigInt(intent.sequence)) {
			intent.phase = 'Receipt';
			intent.receipt = previous;
			saveCksolPayment(storage, intent);
			return intent;
		}
		throw new Error(
			'A payment may already be in progress. Receipt is unavailable; do not approve or start again.'
		);
	}
	if (
		intent &&
		intent.phase === 'Receipt' &&
		intent.receipt?.phase !== 'Complete' &&
		intent.receipt?.phase !== 'FundingRejected'
	)
		throw new Error('Resolve the saved payment receipt before starting another payment.');
	if (previous && !['Complete', 'FundingRejected'].includes(previous.phase))
		throw new Error('A previous payment is unresolved. Refresh its receipt before starting again.');
	// Revalidate displayed terms after the user has sent SOL; display age alone is not a payment change.
	const fresh = await quoteCksolPayment(owner, walletType);
	if (!isLive(owner, q.sessionGeneration, walletType, live))
		throw new Error('Wallet session changed.');
	for (const f of [
		'next_sequence',
		'funding_cycles',
		'processing_fee',
		'ledger_fee',
		'approval_amount',
		'refund_fee_reserve'
	] as const)
		if (fresh[f] !== q[f])
			throw new Error('Payment quote changed. Refresh the quote before approval.');
	const seq = fresh.next_sequence;
	const max = fresh.approval_amount + fresh.ledger_fee;
	if (fresh.balance < max)
		throw new Error(
			`Cycles ledger balance is too low. Need ${max} cycles including the approval transaction fee.`
		);
	const g = q.sessionGeneration,
		wt = q.walletType;
	const ledgerActor: any =
		wt === 'oisy'
			? createOisyActor(q.cycles_ledger.toText(), ICRC1_IDL, await getOisySignerAgent(owner))
			: await walletStore.getActor(q.cycles_ledger.toText(), ICRC1_IDL);
	if (!isLive(owner, g, wt, live)) throw new Error('Wallet session changed.');
	await verifySigner(ledgerActor, owner);
	if (!isLive(owner, g, wt, live)) throw new Error('Wallet session changed.');
	await requireNoLegacyDeposit(owner, g, wt, live);
	const createdAt = Date.now();
	intent = {
		owner: q.owner,
		sequence: seq.toString(),
		maxPayment: fresh.funding_cycles.toString(),
		approvalAmount: fresh.approval_amount.toString(),
		phase: 'Approving',
		createdAt
	};
	saveCksolPayment(storage, intent);
	let approval: any;
	try {
		approval = await ledgerActor.icrc2_approve({
			spender: { owner: Principal.fromText(CONFIG.currentCanisterId), subaccount: [] },
			amount: fresh.approval_amount,
			expected_allowance: [fresh.allowance],
			fee: [fresh.ledger_fee],
			from_subaccount: [],
			memo: [],
			expires_at: [BigInt(createdAt + 10 * 60_000) * 1_000_000n],
			created_at_time: [BigInt(createdAt) * 1_000_000n]
		});
	} catch (e) {
		intent.phase = 'ApprovalUnknown';
		intent.message = String(e);
		saveCksolPayment(storage, intent);
		return intent;
	}
	if ('Err' in approval) {
		intent.phase = 'NotStarted';
		intent.message = `Approval rejected: ${JSON.stringify(approval.Err, (_key, value) => (typeof value === 'bigint' ? value.toString() : value))}`;
		saveCksolPayment(storage, intent);
		return intent;
	}
	intent.phase = 'Approved';
	intent.approvalBlock = BigInt(approval.Ok).toString();
	saveCksolPayment(storage, intent);
	if (!isLive(owner, g, wt, live)) return intent;
	if (wt === 'oisy') {
		// The SDK closes its channel 200 ms after a signed response. A fresh
		// click must open the next channel before the network quote and start.
		intent.message = 'Cycles approved. SOL processing has not started. Click Process my deposit to authorize processing.';
		saveCksolPayment(storage, intent);
		return intent;
	}
	// Refresh all backend terms immediately before the irreversible dispatch.
	const before = await quoteCksolPayment(owner, wt);
	if (before.next_sequence !== seq) {
		intent.phase = 'NotStarted';
		intent.message =
			'The backend payment sequence changed after approval. Refresh the quote; no funding was started.';
		saveCksolPayment(storage, intent);
		return intent;
	}
	for (const f of [
		'funding_cycles',
		'processing_fee',
		'ledger_fee',
		'approval_amount',
		'refund_fee_reserve'
	] as const)
		if (before[f] !== fresh[f]) {
			intent.message = 'Backend quote changed after approval; dispatch stopped.';
			saveCksolPayment(storage, intent);
			return intent;
		}
	intent.phase = 'Starting';
	saveCksolPayment(storage, intent);
	try {
		const updateActor: any =
			wt === 'oisy'
				? createOisyActor(CONFIG.currentCanisterId, PaidBackendIDL, await getOisySignerAgent(owner))
				: await walletStore.getActor(CONFIG.currentCanisterId, PaidBackendIDL);
		if (!isLive(owner, g, wt, live)) return intent;
		await verifySigner(updateActor, owner);
		if (!isLive(owner, g, wt, live)) return intent;
		const result = await updateActor.start_cksol_deposit_paid(seq, fresh.funding_cycles);
		if ('Err' in result) {
			if (String(result.Err) === STALE_PAYMENT_SEQUENCE_ERROR) {
				intent.phase = 'NotStarted';
				intent.message =
					'Backend sequence was taken before funding. Refresh the quote and submit again.';
			} else {
				intent.phase = 'StartUnknown';
				intent.message = String(result.Err);
				try {
					const receipt = await readCksolPayment(owner);
					if (receipt && BigInt(receipt.sequence) === seq) {
						intent.phase = 'Receipt';
						intent.receipt = receipt;
					}
				} catch {}
			}
		} else {
			intent.phase = 'Receipt';
			intent.receipt = { ...result.Ok, phase: phaseName(result.Ok.phase) };
		}
	} catch (e) {
		intent.phase = 'StartUnknown';
		intent.message = String(e);
		try {
			const receipt = await readCksolPayment(owner);
			if (receipt && BigInt(receipt.sequence) === seq) {
				intent.phase = 'Receipt';
				intent.receipt = receipt;
			}
		} catch {}
	}
	saveCksolPayment(storage, intent);
	return intent;
}
export async function claimCksolRefund(
	owner: Principal,
	payment: CksolPayment,
	live: () => boolean
) {
	const generation = get(walletSessionGeneration),
		walletType = get(auth).walletType ?? 'unknown';
	if (
		!payment ||
		!['RefundReady', 'Funded'].includes(phaseName(payment.phase)) ||
		payment.sequence <= 0n
	)
		throw new Error('Only a funded or refund-ready payment can claim a cycles refund.');
	if (!isLive(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	const actor: any =
		walletType === 'oisy'
			? createOisyActor(CONFIG.currentCanisterId, PaidBackendIDL, await getOisySignerAgent(owner))
			: await walletStore.getActor(CONFIG.currentCanisterId, PaidBackendIDL);
	if (!isLive(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	await verifySigner(actor, owner);
	if (!isLive(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	return actor.claim_cksol_cycle_refund(payment.sequence);
}
export async function resumeFundedCksolPayment(
	owner: Principal,
	intent: CksolPaymentIntent,
	storage: IntentStore,
	live: () => boolean
) {
	const generation = get(walletSessionGeneration),
		walletType = get(auth).walletType ?? 'unknown';
	if (
		intent.owner !== owner.toText() ||
		!intent.receipt ||
		phaseName(intent.receipt.phase) !== 'Funded'
	)
		throw new Error('Only a confirmed funded payment can resume minter dispatch.');
	if (!isLive(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	const terms = await quoteCksolPayment(owner, walletType);
	if (!isLive(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	intent.maxPayment = terms.funding_cycles.toString();
	saveCksolPayment(storage, intent);
	const actor: any =
		walletType === 'oisy'
			? createOisyActor(CONFIG.currentCanisterId, PaidBackendIDL, await getOisySignerAgent(owner))
			: await walletStore.getActor(CONFIG.currentCanisterId, PaidBackendIDL);
	if (!isLive(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	await verifySigner(actor, owner);
	if (!isLive(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	const result = await actor.start_cksol_deposit_paid(
		BigInt(intent.sequence),
		BigInt(intent.maxPayment)
	);
	if ('Err' in result) throw new Error(String(result.Err));
	intent.phase = 'Receipt';
	intent.receipt = { ...result.Ok, phase: phaseName(result.Ok.phase) };
	return intent;
}
export async function continueApprovedCksolPayment(
	owner: Principal,
	intent: CksolPaymentIntent,
	storage: IntentStore,
	live: () => boolean
) {
	const generation = get(walletSessionGeneration),
		walletType = get(auth).walletType ?? 'unknown';
	if (
		intent.owner !== owner.toText() ||
		intent.phase !== 'Approved' ||
		intent.approvalBlock === undefined ||
		!intent.maxPayment ||
		!intent.approvalAmount
	)
		throw new Error('Only a confirmed approval that has not started can continue.');
	if (!isLive(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	const previous = await readCksolPayment(owner);
	if (!isLive(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	if (previous && BigInt(previous.sequence) >= BigInt(intent.sequence)) {
		if (BigInt(previous.sequence) === BigInt(intent.sequence)) {
			intent.phase = 'Receipt';
			intent.receipt = previous;
			saveCksolPayment(storage, intent);
			return intent;
		}
		throw new Error(
			'A newer payment sequence exists. Preserve this request and resolve the latest receipt.'
		);
	}
	const fresh = await quoteCksolPayment(owner, walletType);
	if (!isLive(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	if (fresh.next_sequence !== BigInt(intent.sequence)) {
		intent.phase = 'NotStarted';
		intent.message =
			'The backend payment sequence changed after approval. Refresh the quote; no funding was started.';
		saveCksolPayment(storage, intent);
		return intent;
	}
	if (
		fresh.funding_cycles !== BigInt(intent.maxPayment) ||
		fresh.approval_amount !== BigInt(intent.approvalAmount) ||
		fresh.allowance < BigInt(intent.approvalAmount)
	)
		throw new Error(
			'Saved approval is no longer available or backend terms changed. Do not approve again; refresh the receipt or contact support.'
		);
	intent.phase = 'Starting';
	saveCksolPayment(storage, intent);
	try {
		const actor: any =
			walletType === 'oisy'
				? createOisyActor(CONFIG.currentCanisterId, PaidBackendIDL, await getOisySignerAgent(owner))
				: await walletStore.getActor(CONFIG.currentCanisterId, PaidBackendIDL);
		if (!isLive(owner, generation, walletType, live)) return intent;
		await verifySigner(actor, owner);
		if (!isLive(owner, generation, walletType, live)) return intent;
		const result = await actor.start_cksol_deposit_paid(
			BigInt(intent.sequence),
			BigInt(intent.maxPayment)
		);
		if ('Err' in result) {
			if (String(result.Err) === STALE_PAYMENT_SEQUENCE_ERROR) {
				intent.phase = 'NotStarted';
				intent.message =
					'Backend sequence was taken before funding. Refresh the quote and submit again.';
			} else {
				intent.phase = 'StartUnknown';
				intent.message = String(result.Err);
				try {
					const r = await readCksolPayment(owner);
					if (r && BigInt(r.sequence) === BigInt(intent.sequence)) {
						intent.phase = 'Receipt';
						intent.receipt = r;
					}
				} catch {}
			}
		} else {
			intent.phase = 'Receipt';
			intent.receipt = { ...result.Ok, phase: phaseName(result.Ok.phase) };
		}
	} catch (e) {
		intent.phase = 'StartUnknown';
		intent.message = String(e);
		try {
			const r = await readCksolPayment(owner);
			if (r && BigInt(r.sequence) === BigInt(intent.sequence)) {
				intent.phase = 'Receipt';
				intent.receipt = r;
			}
		} catch {}
	}
	saveCksolPayment(storage, intent);
	return intent;
}
