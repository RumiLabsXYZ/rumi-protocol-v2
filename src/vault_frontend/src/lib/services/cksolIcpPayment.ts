import { Actor, HttpAgent } from '@dfinity/agent';
import { Principal } from '@dfinity/principal';
import { get } from 'svelte/store';
import { CONFIG, CANISTER_IDS } from '../config';
import { ICRC1_IDL } from '../idls/ledger.idl.js';
import { idlFactory as backendIdl } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
import { walletStore } from '../stores/wallet';
import { auth, walletSessionGeneration } from './auth';
import { getCksolRelay, type IntentStore } from './cksolMinter';
import { createOisyActor } from './oisySigner';
import { getCksolOisySignerAgent } from './cksolOisySigner';
import {
	assertOtherCksolLocalSettled,
	requireOtherCksolPaymentSettled,
	cksolIcpNeedsReconciliation
} from './cksolPaymentJournal';

export const CKSOL_CMC = 'rkp4c-7iaaa-aaaaa-aaaca-cai';
const MAX_U64 = (1n << 64n) - 1n;
// Read-only projection of the official CMC interface. No ICP is sent to CMC.
const RateIDL = ({ IDL }: any) =>
	IDL.Service({
		get_icp_xdr_conversion_rate: IDL.Func(
			[],
			[
				IDL.Record({
					data: IDL.Record({ xdr_permyriad_per_icp: IDL.Nat64, timestamp_seconds: IDL.Nat64 }),
					certificate: IDL.Vec(IDL.Nat8),
					hash_tree: IDL.Vec(IDL.Nat8)
				})
			],
			['query']
		)
	});
export interface CksolIcpTerms {
	next_sequence: bigint;
	attached_cycles: bigint;
	operating_fee_cycles: bigint;
	service_fee_bps: bigint;
	reserve_available_cycles: bigint;
	available: boolean;
	icp_ledger: Principal;
	cmc: Principal;
}
export interface CksolIcpQuote {
	owner: string;
	walletType: string;
	generation: number;
	terms: CksolIcpTerms;
	sequence: bigint;
	rate: bigint;
	rateTimestamp: bigint;
	fee: bigint;
	balance: bigint;
	allowance: bigint;
	maxCharge: bigint;
	deposit: bigint;
	approval: bigint;
	total: bigint;
}
export interface CksolIcpIntent {
	owner: string;
	sequence: string;
	createdAt: number;
	phase:
		| 'NotStarted'
		| 'Approving'
		| 'ApprovalUnknown'
		| 'Approved'
		| 'Starting'
		| 'StartUnknown'
		| 'Receipt';
	deposit?: string;
	rate?: string;
	rateTimestamp?: string;
	fee?: string;
	approvalBlock?: string;
	fundingCandidateBlock?: string;
	receipt?: any;
	message?: string;
}
const ceil = (a: bigint, b: bigint) => (a + b - 1n) / b;
export function icpEscrowAmounts(
	attached: bigint,
	operating: bigint,
	bps: bigint,
	rate: bigint,
	fee: bigint
) {
	if (attached <= 0n || operating < 0n || bps < 0n || bps > 10_000n || rate <= 0n || fee <= 0n)
		throw new Error('Invalid ICP processing terms.');
	const maxCharge = ceil(attached + ceil(attached * bps, 10_000n) + operating, rate);
	const deposit = maxCharge + 2n * fee,
		approval = deposit + fee,
		total = approval + fee;
	if (total > MAX_U64) throw new Error('ICP processing quote exceeds the supported amount.');
	return { maxCharge, deposit, approval, total };
}
function session(owner: Principal, generation: number, walletType: string, live: () => boolean) {
	const w = get(walletStore),
		a = get(auth);
	return (
		live() &&
		get(walletSessionGeneration) === generation &&
		w.isConnected &&
		!w.loading &&
		w.principal?.toText() === owner.toText() &&
		a.isConnected &&
		a.account?.owner?.toText() === owner.toText() &&
		a.walletType === walletType
	);
}
async function signer(actor: any, owner: Principal) {
	const agent = Actor.agentOf(actor);
	if (!agent || (await agent.getPrincipal()).toText() !== owner.toText())
		throw new Error('Wallet signer does not match the connected ICP account.');
}
function queryActor(id: string, idl: any) {
	return Actor.createActor(idl, {
		agent: new HttpAgent({ host: CONFIG.host }),
		canisterId: id
	}) as any;
}
export function cksolIcpPaymentKey(owner: string) {
	return `rumi:cksol:icp:v1:${CONFIG.currentCanisterId}:${owner}`;
}
function save(storage: IntentStore, p: CksolIcpIntent) {
	storage.setItem(
		cksolIcpPaymentKey(p.owner),
		JSON.stringify(p, (_k, v) => (typeof v === 'bigint' ? v.toString() : v))
	);
}
export function loadCksolIcpPayment(storage: IntentStore, owner: string): CksolIcpIntent | null {
	const raw = storage.getItem(cksolIcpPaymentKey(owner));
	if (!raw) return null;
	const p = JSON.parse(raw);
	if (
		p.owner !== owner ||
		!/^[1-9][0-9]*$/.test(p.sequence) ||
		BigInt(p.sequence) > MAX_U64 ||
		!Number.isFinite(p.createdAt) ||
		![
			'NotStarted',
			'Approving',
			'ApprovalUnknown',
			'Approved',
			'Starting',
			'StartUnknown',
			'Receipt'
		].includes(p.phase)
	)
		throw new Error('Saved ICP payment is invalid. Preserve it for support.');
	return p;
}
const normalize = (p: any) => ({
	...p,
	phase: typeof p.phase === 'string' ? p.phase : Object.keys(p.phase ?? {})[0]
});
export async function readCksolIcpPayment(owner: Principal) {
	const backend: any = getCksolRelay();
	if (typeof backend.get_cksol_icp_payment !== 'function')
		throw new Error('ICP processing payments are not available on this backend yet.');
	const p = (await backend.get_cksol_icp_payment(owner))?.[0];
	return p ? normalize(p) : null;
}
export function icpPaymentSettled(p: CksolIcpIntent | null) {
	return (
		!p ||
		p.phase === 'NotStarted' ||
		(p.phase === 'Receipt' &&
			!cksolIcpNeedsReconciliation(p.receipt) &&
			['Complete', 'FundingRejected'].includes(p.receipt?.phase))
	);
}
export async function recoverCksolIcpPayment(owner: Principal, storage: IntentStore) {
	const saved = loadCksolIcpPayment(storage, owner.toText()); // Damaged journals stay held, never become an empty state.
	const key = cksolIcpPaymentKey(owner.toText()),
		original = storage.getItem(key);
	const generation = get(walletSessionGeneration),
		walletType = get(auth).walletType ?? 'unknown';
	const receipt = await readCksolIcpPayment(owner);
	if (!session(owner, generation, walletType, () => true))
		throw new Error('Wallet session changed.');
	if (storage.getItem(key) !== original) return loadCksolIcpPayment(storage, owner.toText());
	if (
		!receipt ||
		(saved && BigInt(saved.sequence) > BigInt(receipt.sequence)) ||
		(saved && saved.sequence !== String(receipt.sequence) && !icpPaymentSettled(saved))
	)
		return saved;
	const result: CksolIcpIntent = {
		...saved,
		owner: owner.toText(),
		sequence: String(receipt.sequence),
		createdAt: saved?.createdAt ?? Date.now(),
		phase: 'Receipt',
		receipt
	};
	save(storage, result);
	return result;
}
export async function quoteCksolIcpPayment(
	owner: Principal,
	walletType: string
): Promise<CksolIcpQuote> {
	if (CONFIG.isLocal || owner.isAnonymous())
		throw new Error('Connect an ICP wallet on mainnet first.');
	const generation = get(walletSessionGeneration);
	if (!session(owner, generation, walletType, () => true))
		throw new Error('Wallet session changed.');
	const backend: any = getCksolRelay();
	if (typeof backend.get_cksol_icp_terms !== 'function')
		throw new Error('ICP processing payments are not available on this backend yet.');
	const terms: CksolIcpTerms = await backend.get_cksol_icp_terms();
	if (
		terms.icp_ledger.toText() !== CANISTER_IDS.ICP_LEDGER ||
		terms.cmc.toText() !== CKSOL_CMC ||
		BigInt(terms.attached_cycles) !== 1_000_000_000_000n ||
		BigInt(terms.operating_fee_cycles) !== 1_000_000_000n ||
		BigInt(terms.service_fee_bps) !== 500n
	)
		throw new Error('ICP processing terms differ from the reviewed fee schedule.');
	const ledger = queryActor(CANISTER_IDS.ICP_LEDGER, ICRC1_IDL),
		cmc = queryActor(CKSOL_CMC, RateIDL);
	const [rateReply, fee, balance, allowance] = await Promise.all([
		cmc.get_icp_xdr_conversion_rate(),
		ledger.icrc1_fee(),
		ledger.icrc1_balance_of({ owner, subaccount: [] }),
		ledger.icrc2_allowance({
			account: { owner, subaccount: [] },
			spender: { owner: Principal.fromText(CONFIG.currentCanisterId), subaccount: [] }
		})
	]);
	if (!session(owner, generation, walletType, () => true))
		throw new Error('Wallet session changed.');
	const rate = BigInt(rateReply.data.xdr_permyriad_per_icp),
		rateTimestamp = BigInt(rateReply.data.timestamp_seconds),
		now = BigInt(Math.floor(Date.now() / 1000));
	if (rate <= 0n || rateTimestamp > now || now - rateTimestamp > 600n)
		throw new Error('The ICP conversion rate is unavailable or stale. Refresh before approving.');
	const amounts = icpEscrowAmounts(
		BigInt(terms.attached_cycles),
		BigInt(terms.operating_fee_cycles),
		BigInt(terms.service_fee_bps),
		rate,
		BigInt(fee)
	);
	const sequence = BigInt(terms.next_sequence);
	if (sequence < 1n || sequence > MAX_U64) throw new Error('Invalid ICP payment sequence.');
	return {
		owner: owner.toText(),
		walletType,
		generation,
		terms,
		sequence,
		rate,
		rateTimestamp,
		fee: BigInt(fee),
		balance: BigInt(balance),
		allowance: BigInt(allowance.allowance),
		...amounts
	};
}
async function walletActor(id: string, idl: any, owner: Principal, walletType: string) {
	return walletType === 'oisy'
		? createOisyActor(id, idl, await getCksolOisySignerAgent(owner))
		: walletStore.getActor(id, idl);
}
async function dispatch(
	p: CksolIcpIntent,
	owner: Principal,
	storage: IntentStore,
	live: () => boolean
) {
	const generation = get(walletSessionGeneration),
		walletType = get(auth).walletType ?? 'unknown';
	if (!session(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	await requireOtherCksolPaymentSettled(storage, owner, 'icp', getCksolRelay());
	for (const field of ['deposit', 'rate', 'rateTimestamp', 'fee'] as const)
		if (!p[field] || !/^[0-9]+$/.test(p[field]!))
			throw new Error(
				'Saved ICP approval terms are unavailable. Preserve this request for support.'
			);
	const actor = await walletActor(CONFIG.currentCanisterId, backendIdl, owner, walletType);
	await signer(actor, owner);
	if (!session(owner, generation, walletType, live)) return p;
	assertOtherCksolLocalSettled(storage, owner.toText(), 'icp');
	if (loadCksolIcpPayment(storage, owner.toText())?.sequence !== p.sequence)
		throw new Error('Saved ICP request changed.');
	p.phase = 'Starting';
	save(storage, p);
	try {
		const reply = await actor.start_cksol_deposit_with_icp(
			BigInt(p.sequence),
			BigInt(p.deposit!),
			BigInt(p.rate!),
			BigInt(p.rateTimestamp!),
			BigInt(p.fee!)
		);
		if ('Ok' in reply) {
			p.phase = 'Receipt';
			p.receipt = normalize(reply.Ok);
		} else {
			// A typed reply ended this request. Check its durable record before
			// distinguishing pre-collection validation failure from a funded hold.
			const recorded = await readCksolIcpPayment(owner);
			if (recorded && String(recorded.sequence) === p.sequence) {
				p.phase = 'Receipt';
				p.receipt = recorded;
			} else if (!recorded || BigInt(recorded.sequence) < BigInt(p.sequence))
				p.phase = 'NotStarted';
			else p.phase = 'StartUnknown';
			p.message = String(reply.Err);
		}
	} catch (e) {
		p.phase = 'StartUnknown';
		p.message = String(e);
	}
	// Finish this exact owner's durable intent even when the UI switched accounts;
	// never replace a different sequence written by another session.
	const current = loadCksolIcpPayment(storage, owner.toText());
	if (current?.sequence === p.sequence && current.phase === 'Starting') save(storage, p);
	return p;
}
/** Caller holds the shared ckSOL payment Web Lock; no new payment can bypass an uncertain saved attempt. */
export async function submitCksolIcpPayment(
	q: CksolIcpQuote,
	owner: Principal,
	storage: IntentStore,
	live: () => boolean
) {
	if (q.owner !== owner.toText() || !session(owner, q.generation, q.walletType, live))
		throw new Error('Wallet session changed.');
	await requireOtherCksolPaymentSettled(storage, owner, 'icp', getCksolRelay());
	if (!icpPaymentSettled(loadCksolIcpPayment(storage, q.owner)))
		throw new Error('Resolve the saved ICP payment before approving another.');
	const previous = await readCksolIcpPayment(owner);
	if (
		previous &&
		(cksolIcpNeedsReconciliation(previous) ||
			!['Complete', 'FundingRejected'].includes(previous.phase))
	)
		throw new Error('Resolve the previous ICP payment before approving another.');
	const fresh = await quoteCksolIcpPayment(owner, q.walletType);
	if (!session(owner, q.generation, q.walletType, live)) throw new Error('Wallet session changed.');
	if (
		!fresh.terms.available ||
		BigInt(fresh.terms.reserve_available_cycles) < BigInt(fresh.terms.attached_cycles)
	)
		throw new Error(
			'The mint processing reserve is temporarily unavailable. Your existing SOL deposit does not need another transfer.'
		);
	if (fresh.balance < fresh.total)
		throw new Error(
			'Not enough ICP for the maximum deposit and ledger fees. Add ICP to this same connected account.'
		);
	for (const f of ['rate', 'fee', 'deposit', 'approval', 'total'] as const)
		if (fresh[f] !== q[f])
			throw new Error('ICP processing price changed. Review the refreshed quote before approving.');
	const ledger = await walletActor(CANISTER_IDS.ICP_LEDGER, ICRC1_IDL, owner, q.walletType);
	const backend = await walletActor(CONFIG.currentCanisterId, backendIdl, owner, q.walletType);
	await signer(ledger, owner);
	await signer(backend, owner);
	await requireOtherCksolPaymentSettled(storage, owner, 'icp', getCksolRelay());
	if (!session(owner, q.generation, q.walletType, live)) throw new Error('Wallet session changed.');
	assertOtherCksolLocalSettled(storage, owner.toText(), 'icp');
	const p: CksolIcpIntent = {
		owner: q.owner,
		sequence: fresh.sequence.toString(),
		createdAt: Date.now(),
		phase: 'Approving',
		deposit: fresh.deposit.toString(),
		rate: fresh.rate.toString(),
		rateTimestamp: fresh.rateTimestamp.toString(),
		fee: fresh.fee.toString()
	};
	save(storage, p);
	let approval: any;
	try {
		approval = await ledger.icrc2_approve({
			spender: { owner: Principal.fromText(CONFIG.currentCanisterId), subaccount: [] },
			amount: fresh.approval,
			expected_allowance: [fresh.allowance],
			fee: [fresh.fee],
			from_subaccount: [],
			memo: [],
			expires_at: [BigInt(p.createdAt + 600_000) * 1_000_000n],
			created_at_time: [BigInt(p.createdAt) * 1_000_000n]
		});
	} catch (e) {
		p.phase = 'ApprovalUnknown';
		p.message = String(e);
		save(storage, p);
		return p;
	}
	if ('Err' in approval) {
		p.phase = 'NotStarted';
		p.message = 'ICP approval was rejected. No processing deposit was collected.';
		save(storage, p);
		return p;
	}
	p.phase = 'Approved';
	p.approvalBlock = String(approval.Ok);
	save(storage, p);
	if (!session(owner, q.generation, q.walletType, live)) return p;
	return dispatch(p, owner, storage, live);
}
export async function continueCksolIcpApproval(
	p: CksolIcpIntent,
	owner: Principal,
	storage: IntentStore,
	live: () => boolean
) {
	const saved = loadCksolIcpPayment(storage, owner.toText());
	if (p.owner !== owner.toText() || saved?.sequence !== p.sequence || saved.phase !== 'Approved')
		throw new Error('Only this saved confirmed ICP approval can continue.');
	return dispatch(saved, owner, storage, live);
}
export async function resumeCksolIcpPayment(
	p: CksolIcpIntent,
	owner: Principal,
	storage: IntentStore,
	live: () => boolean
) {
	if (
		p.owner !== owner.toText() ||
		loadCksolIcpPayment(storage, owner.toText())?.sequence !== p.sequence
	)
		throw new Error('Saved ICP payment does not match this account.');
	const generation = get(walletSessionGeneration),
		walletType = get(auth).walletType ?? 'unknown';
	const receipt = await readCksolIcpPayment(owner);
	if (!session(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	if (
		!receipt ||
		String(receipt.sequence) !== p.sequence ||
		!['Funded', 'RefundReady', 'RefundUnknown', 'RefundPending'].includes(receipt.phase)
	)
		throw new Error('This payment needs receipt reconciliation before it can continue.');
	const actor = await walletActor(CONFIG.currentCanisterId, backendIdl, owner, walletType);
	await signer(actor, owner);
	if (!session(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	const reply = await actor.resume_cksol_icp_payment(BigInt(p.sequence));
	if ('Err' in reply) throw new Error(String(reply.Err));
	const current = loadCksolIcpPayment(storage, owner.toText());
	const result: CksolIcpIntent = { ...p, phase: 'Receipt', receipt: normalize(reply.Ok) };
	if (current?.sequence === p.sequence) save(storage, result);
	return result;
}

/** Verify the original collection only. The caller holds the shared owner Web Lock. */
export async function reconcileCksolIcpFunding(
	p: CksolIcpIntent,
	candidateBlock: string,
	owner: Principal,
	storage: IntentStore,
	live: () => boolean
) {
	if (!/^(0|[1-9][0-9]{0,19})$/.test(candidateBlock) || BigInt(candidateBlock) > MAX_U64)
		throw new Error('Enter a valid ICP collection block number.');
	const key = cksolIcpPaymentKey(owner.toText()),
		original = storage.getItem(key);
	const saved = loadCksolIcpPayment(storage, owner.toText());
	if (p.owner !== owner.toText() || saved?.sequence !== p.sequence)
		throw new Error('Saved ICP payment does not match this account.');
	const generation = get(walletSessionGeneration),
		walletType = get(auth).walletType ?? 'unknown';
	if (!session(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	const receipt = await readCksolIcpPayment(owner);
	if (!session(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	if (
		!receipt ||
		String(receipt.sequence) !== p.sequence ||
		!['FundingPending', 'FundingUnknown'].includes(receipt.phase) ||
		receipt.funding_block?.length
	)
		throw new Error(
			'Only an unresolved ICP collection can use this block check. Refresh its receipt.'
		);
	const actor = await walletActor(CONFIG.currentCanisterId, backendIdl, owner, walletType);
	await signer(actor, owner);
	if (!session(owner, generation, walletType, live)) throw new Error('Wallet session changed.');
	if (storage.getItem(key) !== original)
		throw new Error('Saved ICP request changed. Refresh its receipt.');
	if (typeof actor.reconcile_cksol_icp_funding_from_block !== 'function')
		throw new Error('ICP collection verification is not available on this backend yet.');
	const submitted = { ...saved, fundingCandidateBlock: candidateBlock };
	save(storage, submitted);
	const submittedRaw = storage.getItem(key);
	const reply = await actor.reconcile_cksol_icp_funding_from_block(
		BigInt(p.sequence),
		BigInt(candidateBlock)
	);
	if ('Err' in reply) throw new Error(String(reply.Err));
	const result: CksolIcpIntent = { ...submitted, phase: 'Receipt', receipt: normalize(reply.Ok) };
	if (storage.getItem(key) === submittedRaw) save(storage, result);
	return result;
}
