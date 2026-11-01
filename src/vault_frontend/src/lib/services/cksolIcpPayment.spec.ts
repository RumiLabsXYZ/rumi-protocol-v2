import { beforeEach, describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
const fx = vi.hoisted(() => {
	const store = (v: any) => ({
		subscribe(fn: any) {
			fn(v);
			return () => {};
		},
		set(n: any) {
			v = n;
		}
	});
	return {
		wallet: store(null),
		auth: store(null),
		generation: store(0),
		backend: {} as any,
		ledger: {} as any,
		cmc: {} as any,
		updates: {} as any
	};
});
vi.mock('@dfinity/agent', () => ({
	Actor: {
		createActor: (_idl: any, o: any) =>
			o.canisterId === 'rkp4c-7iaaa-aaaaa-aaaca-cai' ? fx.cmc : fx.ledger,
		agentOf: (a: any) => a.agent
	},
	HttpAgent: class {}
}));
vi.mock('../config', () => ({
	CONFIG: {
		isLocal: false,
		host: 'https://icp0.io',
		currentCanisterId: 'tfesu-vyaaa-aaaap-qrd7a-cai'
	},
	CANISTER_IDS: { ICP_LEDGER: 'ryjl3-tyaaa-aaaaa-aaaba-cai' }
}));
vi.mock('../stores/wallet', () => ({
	walletStore: {
		subscribe: fx.wallet.subscribe,
		getActor: vi.fn(async (id: string) =>
			id === 'ryjl3-tyaaa-aaaaa-aaaba-cai' ? fx.ledger : fx.updates
		)
	}
}));
vi.mock('./auth', () => ({ auth: fx.auth, walletSessionGeneration: fx.generation }));
vi.mock('./cksolMinter', () => ({ getCksolRelay: () => fx.backend }));
vi.mock('./oisySigner', () => ({
	createOisyActor: (id: string) => (id === 'ryjl3-tyaaa-aaaaa-aaaba-cai' ? fx.ledger : fx.updates)
}));
vi.mock('./cksolOisySigner', () => ({ getCksolOisySignerAgent: () => ({}) }));
import {
	icpEscrowAmounts,
	quoteCksolIcpPayment,
	submitCksolIcpPayment,
	recoverCksolIcpPayment,
	loadCksolIcpPayment,
	cksolIcpPaymentKey,
	icpPaymentSettled,
	continueCksolIcpApproval
} from './cksolIcpPayment';
const owner = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai');
const other = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
const rate = 40_000n,
	fee = 10_000n;
function storage() {
	const m = new Map<string, string>();
	return {
		getItem: (k: string) => m.get(k) ?? null,
		setItem: (k: string, v: string) => {
			m.set(k, v);
		},
		removeItem: (k: string) => {
			m.delete(k);
		}
	};
}
beforeEach(() => {
	vi.clearAllMocks();
	fx.generation.set(0);
	fx.wallet.set({ isConnected: true, loading: false, principal: owner });
	fx.auth.set({ isConnected: true, account: { owner }, walletType: 'plug' });
	fx.backend = {
		get_cksol_icp_terms: vi.fn(async () => ({
			next_sequence: 1n,
			attached_cycles: 1_000_000_000_000n,
			operating_fee_cycles: 1_000_000_000n,
			service_fee_bps: 500n,
			reserve_available_cycles: 2_000_000_000_000n,
			available: true,
			icp_ledger: Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai'),
			cmc: Principal.fromText('rkp4c-7iaaa-aaaaa-aaaca-cai')
		})),
		get_cksol_icp_payment: vi.fn(async () => [])
	};
	fx.cmc = {
		get_icp_xdr_conversion_rate: vi.fn(async () => ({
			data: {
				xdr_permyriad_per_icp: rate,
				timestamp_seconds: BigInt(Math.floor(Date.now() / 1000))
			}
		}))
	};
	fx.ledger = {
		agent: { getPrincipal: async () => owner },
		icrc1_fee: vi.fn(async () => fee),
		icrc1_balance_of: vi.fn(async () => 100_000_000n),
		icrc2_allowance: vi.fn(async () => ({ allowance: 0n })),
		icrc2_approve: vi.fn(async () => ({ Ok: 7n }))
	};
	fx.updates = {
		agent: { getPrincipal: async () => owner },
		start_cksol_deposit_with_icp: vi.fn(async () => ({
			Ok: {
				sequence: 1n,
				phase: { Complete: null },
				deposit_id: [9n],
				charged_e8s: 1_200_000n,
				refundable_e8s: 0n,
				refund_block: [8n]
			}
		}))
	};
});
describe('ICP processing deposit', () => {
	it('keeps an interrupted mint held after its ICP refund completes', async () => {
		const s = storage(),
			q = await quoteCksolIcpPayment(owner, 'plug');
		const p = {
			owner: owner.toText(),
			sequence: '1',
			createdAt: 1,
			phase: 'Receipt' as const,
			receipt: {
				sequence: 1n,
				phase: 'Complete',
				message: ['Processing was interrupted by an upgrade; SOL status remains unknown']
			}
		};
		s.setItem(
			cksolIcpPaymentKey(owner.toText()),
			JSON.stringify(p, (_k, v) => (typeof v === 'bigint' ? v.toString() : v))
		);
		expect(icpPaymentSettled(p)).toBe(false);
		await expect(submitCksolIcpPayment(q, owner, s, () => true)).rejects.toThrow(
			'saved ICP payment'
		);
		s.removeItem(cksolIcpPaymentKey(owner.toText()));
		fx.backend.get_cksol_icp_payment.mockResolvedValue([
			{ ...p.receipt, phase: { Complete: null } }
		]);
		await expect(submitCksolIcpPayment(q, owner, s, () => true)).rejects.toThrow(
			'previous ICP payment'
		);
		expect(fx.ledger.icrc2_approve).not.toHaveBeenCalled();
	});
	it('caps the 5% service fee and reserves enough ICP for a positive refund after its fee', () => {
		expect(icpEscrowAmounts(1_000_000_000_000n, 1_000_000_000n, 500n, rate, fee)).toEqual({
			maxCharge: 26_275_000n,
			deposit: 26_295_000n,
			approval: 26_305_000n,
			total: 26_315_000n
		});
		expect(icpEscrowAmounts(1n, 0n, 500n, 3n, 10n).maxCharge).toBe(1n);
	});
	it('quotes the connected default ICP account and refuses stale rates', async () => {
		const q = await quoteCksolIcpPayment(owner, 'plug');
		expect(q.deposit).toBe(26_295_000n);
		expect(fx.ledger.icrc1_balance_of).toHaveBeenCalledWith({ owner, subaccount: [] });
		fx.cmc.get_icp_xdr_conversion_rate.mockResolvedValue({
			data: { xdr_permyriad_per_icp: rate, timestamp_seconds: 1n }
		});
		await expect(quoteCksolIcpPayment(owner, 'plug')).rejects.toThrow('conversion rate');
	});
	it('does not approve ICP when the reserve is unavailable or ICP is insufficient', async () => {
		const q = await quoteCksolIcpPayment(owner, 'plug');
		fx.backend.get_cksol_icp_terms.mockImplementation(async () => ({
			...(await {
				next_sequence: 1n,
				attached_cycles: 1_000_000_000_000n,
				operating_fee_cycles: 1_000_000_000n,
				service_fee_bps: 500n,
				reserve_available_cycles: 0n,
				available: false,
				icp_ledger: Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai'),
				cmc: Principal.fromText('rkp4c-7iaaa-aaaaa-aaaca-cai')
			})
		}));
		await expect(submitCksolIcpPayment(q, owner, storage(), () => true)).rejects.toThrow('reserve');
		expect(fx.ledger.icrc2_approve).not.toHaveBeenCalled();
		fx.backend.get_cksol_icp_terms.mockResolvedValue({
			...q.terms,
			available: true,
			reserve_available_cycles: 2_000_000_000_000n
		});
		fx.ledger.icrc1_balance_of.mockResolvedValue(1n);
		await expect(submitCksolIcpPayment(q, owner, storage(), () => true)).rejects.toThrow('ICP');
		expect(fx.ledger.icrc2_approve).not.toHaveBeenCalled();
	});
	it('approves the exact ICP cap then binds dispatch to the quoted rate and sequence', async () => {
		const s = storage(),
			q = await quoteCksolIcpPayment(owner, 'plug');
		const result = await submitCksolIcpPayment(q, owner, s, () => true);
		expect(fx.ledger.icrc2_approve).toHaveBeenCalledWith(
			expect.objectContaining({
				amount: 26_305_000n,
				fee: [fee],
				expected_allowance: [0n],
				from_subaccount: []
			})
		);
		expect(fx.updates.start_cksol_deposit_with_icp).toHaveBeenCalledWith(
			1n,
			26_295_000n,
			rate,
			q.rateTimestamp,
			fee
		);
		expect(result.receipt.phase).toBe('Complete');
	});
	it('preserves a lost approval reply and never sends a second approval', async () => {
		fx.ledger.icrc2_approve.mockRejectedValue(new Error('response lost'));
		const s = storage(),
			q = await quoteCksolIcpPayment(owner, 'plug');
		expect((await submitCksolIcpPayment(q, owner, s, () => true)).phase).toBe('ApprovalUnknown');
		await expect(submitCksolIcpPayment(q, owner, s, () => true)).rejects.toThrow('saved');
		expect(fx.ledger.icrc2_approve).toHaveBeenCalledOnce();
		expect(fx.updates.start_cksol_deposit_with_icp).not.toHaveBeenCalled();
	});
	it('restores the same backend receipt after a lost start reply without spending again', async () => {
		fx.updates.start_cksol_deposit_with_icp.mockRejectedValue(new Error('reply lost'));
		const s = storage(),
			q = await quoteCksolIcpPayment(owner, 'plug');
		expect((await submitCksolIcpPayment(q, owner, s, () => true)).phase).toBe('StartUnknown');
		fx.backend.get_cksol_icp_payment.mockResolvedValue([
			{ sequence: 1n, phase: { RefundUnknown: null } }
		]);
		expect((await recoverCksolIcpPayment(owner, s))?.receipt.phase).toBe('RefundUnknown');
		expect(fx.ledger.icrc2_approve).toHaveBeenCalledOnce();
		expect(fx.updates.start_cksol_deposit_with_icp).toHaveBeenCalledOnce();
	});
	it('stops dispatch if the session changes after approval', async () => {
		fx.ledger.icrc2_approve.mockImplementation(async () => {
			fx.generation.set(1);
			return { Ok: 7n };
		});
		const s = storage(),
			q = await quoteCksolIcpPayment(owner, 'plug');
		expect((await submitCksolIcpPayment(q, owner, s, () => true)).phase).toBe('Approved');
		expect(fx.updates.start_cksol_deposit_with_icp).not.toHaveBeenCalled();
	});
	it('rejects an actor for a different principal before approving', async () => {
		fx.updates.agent = { getPrincipal: async () => other };
		const q = await quoteCksolIcpPayment(owner, 'plug');
		await expect(submitCksolIcpPayment(q, owner, storage(), () => true)).rejects.toThrow('signer');
		expect(fx.ledger.icrc2_approve).not.toHaveBeenCalled();
	});
	it('retains malformed journals and a newer uncertain local request', async () => {
		const s = storage(),
			key = cksolIcpPaymentKey(owner.toText());
		s.setItem(key, 'broken');
		await expect(recoverCksolIcpPayment(owner, s)).rejects.toThrow();
		expect(s.getItem(key)).toBe('broken');
		s.setItem(
			key,
			JSON.stringify({ owner: owner.toText(), sequence: '2', phase: 'StartUnknown', createdAt: 1 })
		);
		fx.backend.get_cksol_icp_payment.mockResolvedValue([
			{ sequence: 1n, phase: { Complete: null } }
		]);
		expect((await recoverCksolIcpPayment(owner, s))?.phase).toBe('StartUnknown');
		expect(loadCksolIcpPayment(s, owner.toText())?.sequence).toBe('2');
	});
	it('continues a known approval without another ICP approval', async () => {
		const s = storage(),
			q = await quoteCksolIcpPayment(owner, 'plug');
		let live = true;
		fx.ledger.icrc2_approve.mockImplementation(async () => {
			live = false;
			return { Ok: 7n };
		});
		const pending = await submitCksolIcpPayment(q, owner, s, () => live);
		live = true;
		expect((await continueCksolIcpApproval(pending, owner, s, () => live)).receipt.phase).toBe(
			'Complete'
		);
		expect(fx.ledger.icrc2_approve).toHaveBeenCalledOnce();
	});
	it('releases only a typed pre-collection rejection after confirming no matching backend receipt', async () => {
		fx.updates.start_cksol_deposit_with_icp.mockResolvedValue({
			Err: 'Payment terms changed or expired; refresh quote'
		});
		const s = storage(),
			q = await quoteCksolIcpPayment(owner, 'plug');
		expect((await submitCksolIcpPayment(q, owner, s, () => true)).phase).toBe('NotStarted');
		expect(fx.ledger.icrc2_approve).toHaveBeenCalledOnce();
	});
	it.each(['ApprovalUnknown', 'StartUnknown', 'damaged'])(
		'does not bypass a cycles-paid %s journal in another tab',
		async (phase) => {
			const s = storage(),
				q = await quoteCksolIcpPayment(owner, 'plug');
			s.setItem(
				`rumi:cksol:paid:v1:${owner.toText()}`,
				phase === 'damaged'
					? 'broken'
					: JSON.stringify({ owner: owner.toText(), sequence: '4', phase, createdAt: 1 })
			);
			await expect(submitCksolIcpPayment(q, owner, s, () => true)).rejects.toThrow('cycles');
			expect(fx.ledger.icrc2_approve).not.toHaveBeenCalled();
			expect(fx.updates.start_cksol_deposit_with_icp).not.toHaveBeenCalled();
		}
	);
	it('stops before approval on an unresolved cycles-paid backend receipt', async () => {
		fx.backend.get_cksol_payment = vi.fn(async () => [
			{ sequence: 4n, phase: { FundingUnknown: null } }
		]);
		const q = await quoteCksolIcpPayment(owner, 'plug');
		await expect(submitCksolIcpPayment(q, owner, storage(), () => true)).rejects.toThrow('cycles');
		expect(fx.ledger.icrc2_approve).not.toHaveBeenCalled();
	});
});
