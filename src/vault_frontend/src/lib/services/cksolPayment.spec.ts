import { beforeEach, describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
import { IDL } from '@dfinity/candid';
import { idlFactory as canonicalBackendIDL } from '../../../../declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';

const fx = vi.hoisted(() => {
	function store<T>(value: T) {
		const listeners = new Set<(v: T) => void>();
		return {
			subscribe(fn: (v: T) => void) {
				listeners.add(fn);
				fn(value);
				return () => listeners.delete(fn);
			},
			set(next: T) {
				value = next;
				listeners.forEach((fn) => fn(value));
			}
		};
	}
	return {
		wallet: store<any>({ isConnected: true, loading: false, principal: null }),
		auth: store<any>({ isConnected: true, account: null, walletType: 'plug' }),
		generation: store(0),
		backendQuery: {} as Record<string, any>,
		ledgerQuery: {} as Record<string, any>,
		relayQuery: {} as Record<string, any>,
		walletActors: {} as Record<string, any>,
		oisyActor: vi.fn(),
		signer: vi.fn(),
		calls: [] as string[]
	};
});
vi.mock('@dfinity/agent', () => ({
	Actor: {
		createActor: vi.fn((_idl: any, opts: any) =>
			opts.canisterId?.toText?.() === 'um5iw-rqaaa-aaaaq-qaaba-cai'
				? fx.ledgerQuery
				: fx.backendQuery
		),
		agentOf: (actor: any) => actor.agent
	},
	HttpAgent: class {
		constructor(_args: any) {}
	}
}));
vi.mock('../config', () => ({
	CONFIG: {
		isLocal: false,
		host: 'https://icp-api.io',
		currentCanisterId: 'rrkah-fqaaa-aaaaa-aaaaq-cai'
	},
	CANISTER_IDS: { CYCLES_LEDGER: 'um5iw-rqaaa-aaaaq-qaaba-cai' }
}));
vi.mock('../idls/ledger.idl.js', () => ({ ICRC1_IDL: () => ({}) }));
vi.mock('../stores/wallet', () => ({
	walletStore: {
		subscribe: fx.wallet.subscribe,
		getActor: vi.fn(async (id: string) => fx.walletActors[id])
	}
}));
vi.mock('./auth', () => ({ auth: fx.auth, walletSessionGeneration: fx.generation }));
vi.mock('./cksolOisySigner', () => ({ getCksolOisySignerAgent: vi.fn(async () => ({})) }));
vi.mock('./oisySigner', () => ({
	getOisySignerAgent: vi.fn(async () => ({})),
	createOisyActor: vi.fn((id: string) => {
		fx.oisyActor(id);
		return id === 'um5iw-rqaaa-aaaaq-qaaba-cai'
			? fx.walletActors[id]
			: fx.walletActors['rrkah-fqaaa-aaaaa-aaaaq-cai'];
	})
}));
vi.mock('./cksolMinter', () => ({ getCksolRelay: () => fx.relayQuery }));
import {
	PaidBackendIDL,
	claimCksolRefund,
	continueApprovedCksolPayment,
	cksolPaymentKey,
	loadCksolPayment,
	quoteCksolPayment,
	recoverCksolPayment,
	resumeFundedCksolPayment,
	submitCksolPayment
} from './cksolPayment';

const owner = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai');
const ledgerId = Principal.fromText('um5iw-rqaaa-aaaaq-qaaba-cai');
const fee = 100_000_000n,
	funding = 1_001_000_000_000n,
	approvalAmount = funding + fee;
let nextSequence = 1n;
function quoteReply(sequence = nextSequence) {
	return {
		cycles_ledger: ledgerId,
		next_sequence: sequence,
		funding_cycles: funding,
		processing_fee: 1_000_000_000n,
		ledger_fee: fee,
		approval_amount: approvalAmount,
		refund_fee_reserve: fee
	};
}
function memory() {
	const m = new Map<string, string>();
	return {
		getItem: (k: string) => m.get(k) ?? null,
		setItem: (k: string, v: string) => {
			m.set(k, v);
		},
		removeItem: (k: string) => m.delete(k)
	};
}
function receipt(sequence = 1n, phase: any = { FundingPending: null }): any {
	return {
		sequence,
		created_at_time: 1n,
		phase,
		funding_block: [],
		deposit_id: [],
		refundable_cycles: 0n,
		refund_block: [],
		message: []
	};
}
function setup(walletType = 'plug') {
	fx.calls.length = 0;
	fx.oisyActor.mockClear();
	fx.generation.set(0);
	fx.wallet.set({ isConnected: true, loading: false, principal: owner });
	fx.auth.set({ isConnected: true, account: { owner }, walletType });
	fx.relayQuery.get_cksol_deposit = vi.fn(async () => []);
	nextSequence = 1n;
	fx.backendQuery.get_cksol_payment_quote = vi.fn(async () => quoteReply());
	fx.backendQuery.get_cksol_payment = vi.fn(async () => []);
	fx.backendQuery.start_cksol_deposit_paid = vi.fn(async (sequence: bigint, maxPayment: bigint) => {
		fx.calls.push('start');
		expect(maxPayment).toBe(funding);
		return { Ok: receipt(sequence) };
	});
	fx.backendQuery.claim_cksol_cycle_refund = vi.fn(async (sequence: bigint) => {
		fx.calls.push('refund');
		return { Ok: receipt(sequence, { RefundPending: null }) };
	});
	const agent = { getPrincipal: async () => owner };
	const ledger = {
		agent,
		icrc1_balance_of: vi.fn(async () => 5_000_000_000_000n),
		icrc1_fee: vi.fn(async () => fee),
		icrc2_allowance: vi.fn(async () => ({ allowance: 0n, expires_at: [] })),
		icrc2_approve: vi.fn(async () => {
			fx.calls.push('approve');
			return { Ok: 7n };
		})
	};
	fx.ledgerQuery = {
		...ledger,
		icrc1_balance_of: ledger.icrc1_balance_of,
		icrc1_fee: ledger.icrc1_fee,
		icrc2_allowance: ledger.icrc2_allowance
	};
	fx.walletActors[ledgerId.toText()] = ledger;
	fx.walletActors['rrkah-fqaaa-aaaaa-aaaaq-cai'] = { agent, ...fx.backendQuery };
	fx.oisyActor.mockImplementation((id: string) =>
		id === ledgerId.toText() ? ledger : fx.walletActors['rrkah-fqaaa-aaaaa-aaaaq-cai']
	);
	return { ledger, storage: memory() };
}
const live = () => true;

describe('user-funded ckSOL payment', () => {
	beforeEach(() => {
		vi.clearAllMocks();
	});
	it.each(['plug', 'internet-identity'])(
		'approves and dispatches sequentially for %s',
		async (walletType) => {
			const { ledger, storage } = setup(walletType);
			const q = await quoteCksolPayment(owner, walletType);
			const intent = await submitCksolPayment(q, owner, storage, live);
			expect(intent.phase).toBe('Receipt');
			expect(intent.receipt.phase).toBe('FundingPending');
			expect(fx.calls).toEqual(['approve', 'start']);
			expect(ledger.icrc2_approve).toHaveBeenCalledWith(
				expect.objectContaining({
					amount: approvalAmount,
					expected_allowance: [0n],
					fee: [fee],
					expires_at: [expect.any(BigInt)]
				})
			);
		}
	);
	it('saves the OISY approval for a separate processing click after the channel can close', async () => {
		const { storage } = setup('oisy');
		const q = await quoteCksolPayment(owner, 'oisy');
		const approved = await submitCksolPayment(q, owner, storage, live);
		expect(approved.phase).toBe('Approved');
		expect(approved.approvalBlock).toBe('7');
		expect(fx.calls).toEqual(['approve']);
		fx.ledgerQuery.icrc2_allowance.mockResolvedValue({allowance: approvalAmount, expires_at: []});
		const result = await continueApprovedCksolPayment(owner, approved, storage, live);
		expect(result.phase).toBe('Receipt');
		expect(result.receipt.phase).toBe('FundingPending');
		expect(fx.calls).toEqual(['approve', 'start']);
	});
	it('revalidates an old displayed quote after the user sends SOL', async () => {
		const { storage } = setup();
		const q = await quoteCksolPayment(owner, 'plug');
		q.quotedAt -= 5 * 60_000;
		const intent = await submitCksolPayment(q, owner, storage, live);
		expect(intent.phase).toBe('Receipt');
		expect(intent.approvalAmount).toBe('1001100000000');
		expect(intent.receipt.phase).toBe('FundingPending');
	});
	it('blocks changed terms when revalidating an old displayed quote', async () => {
		const { ledger, storage } = setup();
		const q = await quoteCksolPayment(owner, 'plug');
		q.quotedAt -= 5 * 60_000;
		fx.backendQuery.get_cksol_payment_quote.mockResolvedValueOnce(quoteReply(2n));
		await expect(submitCksolPayment(q, owner, storage, live)).rejects.toThrow('Payment quote changed');
		expect(ledger.icrc2_approve).not.toHaveBeenCalled();
		expect(loadCksolPayment(storage, owner.toText())).toBeNull();
	});
	it('blocks an old quote from a replaced wallet session before approval', async () => {
		const { ledger, storage } = setup();
		const q = await quoteCksolPayment(owner, 'plug');
		q.quotedAt -= 5 * 60_000;
		fx.generation.set(1);
		await expect(submitCksolPayment(q, owner, storage, live)).rejects.toThrow('Wallet session');
		expect(ledger.icrc2_approve).not.toHaveBeenCalled();
		expect(loadCksolPayment(storage, owner.toText())).toBeNull();
	});
	it('reconciles the known unsent OISY popup failure without discarding the original record', async () => {
		const { storage } = setup('oisy');
		const saved = { owner: owner.toText(), sequence: '1', phase: 'ApprovalUnknown', createdAt: 123,
			message: 'Error: Signer window should not be opened outside of click handler, see: https://github.com/slide-computer/signer-js/blob/main/packages/signer-web/README.md#channels-must-be-established-in-a-click-handler' };
		const raw = JSON.stringify(saved);
		storage.setItem(cksolPaymentKey(owner.toText()), raw);
		const recovered = await recoverCksolPayment(owner, storage);
		expect(recovered?.phase).toBe('NotStarted');
		expect(storage.getItem(`${cksolPaymentKey(owner.toText())}:unsent-approval:1:123`)).toBe(raw);
		expect(fx.calls).toEqual([]);
	});
	it('preserves a popup-failure journal when an allowance exists', async () => {
		const { storage } = setup('oisy');
		const saved = { owner: owner.toText(), sequence: '1', phase: 'ApprovalUnknown', createdAt: 123,
			message: 'Error: Signer window should not be opened outside of click handler, see: https://github.com/slide-computer/signer-js/blob/main/packages/signer-web/README.md#channels-must-be-established-in-a-click-handler' };
		storage.setItem(cksolPaymentKey(owner.toText()), JSON.stringify(saved));
		fx.ledgerQuery.icrc2_allowance.mockResolvedValueOnce({ allowance: approvalAmount, expires_at: [] });
		expect((await recoverCksolPayment(owner, storage))?.phase).toBe('ApprovalUnknown');
		expect(fx.calls).toEqual([]);
	});
	it('preserves an actual lost approval reply even with no current allowance or backend receipt', async () => {
		const { storage } = setup('oisy');
		storage.setItem(cksolPaymentKey(owner.toText()), JSON.stringify({owner:owner.toText(),sequence:'1',phase:'ApprovalUnknown',createdAt:123,message:'approval response lost'}));
		expect((await recoverCksolPayment(owner, storage))?.phase).toBe('ApprovalUnknown');
		expect(fx.calls).toEqual([]);
	});
	it('keeps the same intent after a lost approval response and never reapproves', async () => {
		const { ledger, storage } = setup();
		ledger.icrc2_approve.mockRejectedValueOnce(new Error('approval response lost'));
		const q = await quoteCksolPayment(owner, 'plug');
		const first = await submitCksolPayment(q, owner, storage, live);
		expect(first.phase).toBe('ApprovalUnknown');
		expect(loadCksolPayment(storage, owner.toText())?.sequence).toBe('1');
		await expect(submitCksolPayment(q, owner, storage, live)).rejects.toThrow(
			'do not approve or start again'
		);
		expect(ledger.icrc2_approve).toHaveBeenCalledOnce();
		expect(fx.backendQuery.start_cksol_deposit_paid).not.toHaveBeenCalled();
	});
	it('keeps an unknown start response and blocks a new approval or sequence', async () => {
		const { ledger, storage } = setup();
		const update = fx.walletActors['rrkah-fqaaa-aaaaa-aaaaq-cai'];
		update.start_cksol_deposit_paid = vi.fn(async () => {
			fx.calls.push('start');
			throw new Error('start response lost');
		});
		const q = await quoteCksolPayment(owner, 'plug');
		const first = await submitCksolPayment(q, owner, storage, live);
		expect(first.phase).toBe('StartUnknown');
		await expect(submitCksolPayment(q, owner, storage, live)).rejects.toThrow(
			'do not approve or start again'
		);
		expect(ledger.icrc2_approve).toHaveBeenCalledOnce();
		expect(update.start_cksol_deposit_paid).toHaveBeenCalledOnce();
	});
	it('recovers a durable backend receipt even when local storage has no intent', async () => {
		const { storage } = setup();
		fx.backendQuery.get_cksol_payment.mockResolvedValueOnce([receipt(4n, { RefundReady: null })]);
		const recovered = await recoverCksolPayment(owner, storage);
		expect(recovered).toMatchObject({
			owner: owner.toText(),
			sequence: '4',
			phase: 'Receipt',
			receipt: { phase: 'RefundReady' }
		});
		expect(recovered?.maxPayment).toBeUndefined();
		expect(loadCksolPayment(storage, owner.toText())).toMatchObject({
			owner: owner.toText(),
			sequence: '4',
			phase: 'Receipt'
		});
	});
	it('resumes a recovered Funded receipt using only the freshly validated quote cap', async () => {
		const { storage } = setup();
		fx.backendQuery.get_cksol_payment.mockResolvedValueOnce([receipt(4n, { Funded: null })]);
		const recovered = await recoverCksolPayment(owner, storage);
		const update = fx.walletActors['rrkah-fqaaa-aaaaa-aaaaq-cai'];
		const result = await resumeFundedCksolPayment(owner, recovered!, storage, live);
		expect(update.start_cksol_deposit_paid).toHaveBeenCalledWith(4n, funding);
		expect(result.maxPayment).toBe(funding.toString());
		expect(loadCksolPayment(storage, owner.toText())?.maxPayment).toBe(funding.toString());
	});
	it('preserves an uncertain local intent when the durable receipt has a different sequence', async () => {
		const { storage } = setup();
		const saved: any = {
			owner: owner.toText(),
			sequence: '7',
			phase: 'StartUnknown',
			createdAt: 1
		};
		storage.setItem(cksolPaymentKey(owner.toText()), JSON.stringify(saved));
		fx.backendQuery.get_cksol_payment.mockResolvedValueOnce([receipt(8n, { Complete: null })]);
		const recovered = await recoverCksolPayment(owner, storage);
		expect(recovered).toEqual(saved);
		expect(loadCksolPayment(storage, owner.toText())).toEqual(saved);
	});
	it.each([{ Queued: 42n }, { SubmittedOrUnknown: null }, { Rejected: 'minter rejected' }])(
		'blocks pre-existing legacy relay state before approval: %o',
		async (status) => {
			const { ledger, storage } = setup();
			const q = await quoteCksolPayment(owner, 'plug');
			fx.relayQuery.get_cksol_deposit.mockResolvedValueOnce([{ status, started_at_ns: 1n }]);
			await expect(submitCksolPayment(q, owner, storage, live)).rejects.toThrow(/legacy deposit/i);
			expect(ledger.icrc2_approve).not.toHaveBeenCalled();
			expect(fx.backendQuery.start_cksol_deposit_paid).not.toHaveBeenCalled();
		}
	);
	it('fails closed before approval when the read-only legacy relay check fails', async () => {
		const { ledger, storage } = setup();
		const q = await quoteCksolPayment(owner, 'plug');
		fx.relayQuery.get_cksol_deposit.mockRejectedValueOnce(new Error('relay query failed'));
		await expect(submitCksolPayment(q, owner, storage, live)).rejects.toThrow('relay query failed');
		expect(ledger.icrc2_approve).not.toHaveBeenCalled();
	});
	it('rejects changed fee terms before asking for approval', async () => {
		const { ledger, storage } = setup();
		const q = await quoteCksolPayment(owner, 'plug');
		fx.backendQuery.get_cksol_payment_quote.mockResolvedValueOnce({
			...quoteReply(),
			ledger_fee: fee + 1n
		});
		await expect(submitCksolPayment(q, owner, storage, live)).rejects.toThrow(
			'reviewed fee schedule'
		);
		expect(ledger.icrc2_approve).not.toHaveBeenCalled();
	});
	it('rejects a switched wallet session before approval', async () => {
		const { ledger, storage } = setup();
		const q = await quoteCksolPayment(owner, 'plug');
		fx.backendQuery.get_cksol_payment.mockImplementationOnce(async () => {
			fx.generation.set(1);
			return [];
		});
		await expect(submitCksolPayment(q, owner, storage, live)).rejects.toThrow(
			'Wallet session changed'
		);
		expect(ledger.icrc2_approve).not.toHaveBeenCalled();
	});
	it.each([{ FundingPending: null }, { DepositPending: null }])(
		'does not offer refund claims outside Funded and RefundReady',
		async (phase) => {
			const { storage } = setup();
			const update = fx.walletActors['rrkah-fqaaa-aaaaa-aaaaq-cai'];
			await expect(claimCksolRefund(owner, receipt(1n, phase), live)).rejects.toThrow(
				'funded or refund-ready'
			);
			expect(update.claim_cksol_cycle_refund).not.toHaveBeenCalled();
		}
	);
	it.each(['plug', 'internet-identity', 'oisy'])(
		'claims the user refund from %s using the payment sequence',
		async (walletType) => {
			setup(walletType);
			const update = fx.walletActors['rrkah-fqaaa-aaaaa-aaaaq-cai'];
			const result = await claimCksolRefund(owner, receipt(9n, { RefundReady: null }), live);
			expect(update.claim_cksol_cycle_refund).toHaveBeenCalledWith(9n);
			expect(result.Ok.phase).toEqual({ RefundPending: null });
			if (walletType === 'oisy') expect(fx.oisyActor).toHaveBeenCalledTimes(1);
		}
	);
	it('defines the frozen paid endpoints with positional nat64 arguments and nat64 refunds', () => {
		const service: any = PaidBackendIDL({ IDL });
		const methods = Object.fromEntries(service._fields);
		expect(methods.start_cksol_deposit_paid.argTypes.map((t: any) => t.display())).toEqual([
			'nat64',
			'nat64'
		]);
		expect(methods.claim_cksol_cycle_refund.argTypes.map((t: any) => t.display())).toEqual([
			'nat64'
		]);
		const payment = methods.get_cksol_payment.retTypes[0]._type;
		const refundable = payment._fields.find(([name]: any[]) => name === 'refundable_cycles')?.[1];
		expect(refundable.display()).toBe('nat64');
		const quote = methods.get_cksol_payment_quote.retTypes[0];
		expect(quote._fields.find(([name]: any[]) => name === 'next_sequence')?.[1].display()).toBe(
			'nat64'
		);
	});
	it('matches all paid endpoint signatures in the generated canonical backend Candid', () => {
		const service: any = PaidBackendIDL({ IDL }),
			canonical: any = canonicalBackendIDL({ IDL });
		for (const name of [
			'get_cksol_payment_quote',
			'get_cksol_payment',
			'start_cksol_deposit_paid',
			'claim_cksol_cycle_refund'
		]) {
			const local = Object.fromEntries(service._fields)[name],
				generated = Object.fromEntries(canonical._fields)[name];
			expect(generated, `canonical backend is missing ${name}`).toBeDefined();
			expect({
				args: local.argTypes.map((t: any) => t.display()),
				returns: local.retTypes.map((t: any) => t.display()),
				annotations: local.annotations
			}).toEqual({
				args: generated.argTypes.map((t: any) => t.display()),
				returns: generated.retTypes.map((t: any) => t.display()),
				annotations: generated.annotations
			});
		}
	});
	it('uses the global quote sequence even when the owner has a lower prior sequence', async () => {
		const { storage } = setup();
		fx.backendQuery.get_cksol_payment.mockResolvedValueOnce([receipt(1n, { Complete: null })]);
		nextSequence = 37n;
		const q = await quoteCksolPayment(owner, 'plug');
		const intent = await submitCksolPayment(q, owner, storage, live);
		expect(intent.sequence).toBe('37');
		expect(fx.backendQuery.start_cksol_deposit_paid).toHaveBeenCalledWith(37n, funding);
	});
	it('allows a stale global sequence denial to retry only with a fresh sequence', async () => {
		const { storage } = setup();
		const submitted: bigint[] = [];
		const update = fx.walletActors['rrkah-fqaaa-aaaaa-aaaaq-cai'];
		update.start_cksol_deposit_paid = vi.fn(async (sequence: bigint) => {
			submitted.push(sequence);
			return submitted.length === 1
				? { Err: 'Refresh the ckSOL payment sequence' }
				: { Ok: receipt(sequence) };
		});
		nextSequence = 37n;
		const q = await quoteCksolPayment(owner, 'plug');
		const first = await submitCksolPayment(q, owner, storage, live);
		expect(first.phase).toBe('NotStarted');
		expect(first.sequence).toBe('37');
		expect(submitted).toEqual([37n]);
		nextSequence = 38n;
		const fresh = await quoteCksolPayment(owner, 'plug');
		const second = await submitCksolPayment(fresh, owner, storage, live);
		expect(second.phase).toBe('Receipt');
		expect(second.sequence).toBe('38');
		expect(submitted).toEqual([37n, 38n]);
	});
	it('overwrites an existing cycles allowance with a fresh exact capped approval', async () => {
		const { ledger, storage } = setup();
		ledger.icrc2_allowance.mockResolvedValue({ allowance: 123n, expires_at: [] });
		const q = await quoteCksolPayment(owner, 'plug');
		await submitCksolPayment(q, owner, storage, live);
		expect(ledger.icrc2_approve).toHaveBeenCalledWith(
			expect.objectContaining({ amount: approvalAmount, expected_allowance: [123n] })
		);
	});
});
