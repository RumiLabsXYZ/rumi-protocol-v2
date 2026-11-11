import { beforeEach, describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
const fx = vi.hoisted(() => ({
	wallet: { isConnected: true, loading: false, principal: null as any },
	generation: 0,
	provider: 'oisy',
	fee: vi.fn(),
	approve: vi.fn(),
	swap: vi.fn(),
	agent: vi.fn(),
	begin: vi.fn(),
	verify: vi.fn(),
	close: vi.fn(),
	getActor: vi.fn(),
	ready: Promise.resolve(),
	oisy: true
}));
vi.mock('../stores/wallet', () => ({
	walletStore: {
		subscribe(fn: any) {
			fn(fx.wallet);
			return () => {};
		},
		getActor: fx.getActor
	}
}));
vi.mock('./auth', () => ({
	walletSessionGeneration: {
		subscribe(fn: any) {
			fn(fx.generation);
			return () => {};
		}
	},
	currentWalletType: {
		subscribe(fn: any) {
			fn(fx.provider);
			return () => {};
		}
	}
}));
vi.mock('./protocol/walletOperations', () => ({ isOisyWallet: () => fx.oisy }));
vi.mock('../config', () => ({
	CONFIG: { host: 'https://icp0.io', isLocal: false, icusd_ledgerIDL: {} },
	CANISTER_IDS: {
		THREEPOOL: 'fohh4-yyaaa-aaaap-qtkpa-cai',
		ICUSD_LEDGER: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
		CKUSDC_LEDGER: 'xevnm-gaaaa-aaaar-qafnq-cai',
		CKUSDT_LEDGER: 'cngnf-vqaaa-aaaar-qag4q-cai'
	}
}));
vi.mock('./pnp', () => ({ canisterIDLs: { three_pool: {} } }));
vi.mock('./ledgerFeeService', () => ({ fetchLedgerFee: fx.fee, getCachedLedgerFee: () => 10n }));
vi.mock('./tokenService', () => ({ TokenService: { getTokenBalance: vi.fn() } }));
vi.mock('./oisySigner', () => ({
	getOisySignerAgent: fx.agent,
	createOisyActor: (id: string) =>
		id === 'fohh4-yyaaa-aaaap-qtkpa-cai' ? { swap: fx.swap } : { icrc2_approve: fx.approve }
}));
vi.mock('./swapOisySigner', async (original) => ({
	...(await original<any>()),
	beginSwapOisyAction: fx.begin
}));
import { threePoolService } from './threePoolService';
const a = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai'),
	b = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
beforeEach(() => {
	vi.resetAllMocks();
	fx.wallet = { isConnected: true, loading: false, principal: a };
	fx.generation = 0;
	fx.provider = 'oisy';
	fx.oisy = true;
	fx.ready = Promise.resolve();
	fx.fee.mockResolvedValue(10n);
	fx.agent.mockResolvedValue({});
	fx.verify.mockResolvedValue(undefined);
	fx.close.mockResolvedValue(undefined);
	fx.begin.mockImplementation((_owner: any, assertCurrent: any) => ({
		agent: {},
		ready: fx.ready,
		verifyOwner: async () => {
			assertCurrent();
			await fx.verify();
			assertCurrent();
		},
		close: fx.close
	}));
	fx.approve.mockResolvedValue({ Ok: 7n });
	fx.swap.mockResolvedValue({ Ok: 990n });
	fx.getActor.mockImplementation(async (id: string) =>
		id === 'fohh4-yyaaa-aaaap-qtkpa-cai' ? { swap: fx.swap } : { icrc2_approve: fx.approve }
	);
});
describe('3pool swap owner/session boundary', () => {
	it.each(['principal', 'same-owner session', 'provider'])(
		'rejects a stale %s during fee lookup before any approval',
		async (change) => {
			let release!: (value: bigint) => void;
			fx.fee.mockImplementationOnce(() => new Promise((r) => (release = r)));
			const pending = threePoolService.swap(2, 0, 1_000n, 990n);
			const outcome = pending.catch((e) => e);
			await vi.waitFor(() => expect(fx.fee).toHaveBeenCalledOnce());
			if (change === 'principal') fx.wallet.principal = b;
			else if (change === 'provider') fx.provider = 'plug';
			else fx.generation++;
			release(10n);
			expect((await outcome).message).toContain('Wallet session changed');
			expect(fx.approve).not.toHaveBeenCalled();
			expect(fx.swap).not.toHaveBeenCalled();
		}
	);
	it('stops after an accepted approval if the session changes before pool execution', async () => {
		fx.approve.mockImplementationOnce(async () => {
			fx.generation++;
			return { Ok: 7n };
		});
		await expect(threePoolService.swap(2, 0, 1_000n, 990n)).rejects.toThrow(
			'Approval already succeeded'
		);
		expect(fx.approve).toHaveBeenCalledOnce();
		expect(fx.swap).not.toHaveBeenCalled();
	});
	it('verifies the expected owner before each mutating call and keeps exact amounts', async () => {
		await expect(threePoolService.swap(2, 0, 1_000n, 990n)).resolves.toBe(990n);
		expect(fx.begin).toHaveBeenCalledWith(a, expect.any(Function));
		expect(fx.verify).toHaveBeenCalledTimes(2);
		expect(fx.approve.mock.calls[0][0].amount).toBe(1_010n);
		expect(fx.swap).toHaveBeenCalledWith(2, 0, 1_000n, 990n);
		expect(fx.agent).not.toHaveBeenCalled();
		expect(fx.close).toHaveBeenCalledOnce();
	});
});

describe('3pool signer step results', () => {
	const mismatch = () =>
		Object.assign(new Error('The sender must match the owner of the signer.'), { code: 502 });
	it('blocks an owner mismatch before any approval', async () => {
		const { OisySwapOwnerMismatchError } = await import('./swapOisySigner');
		fx.verify.mockRejectedValueOnce(new OisySwapOwnerMismatchError());
		await expect(threePoolService.swap(2, 0, 1_000n, 990n)).rejects.toThrow(
			'approval and swap were not submitted'
		);
		expect(fx.approve).not.toHaveBeenCalled();
		expect(fx.swap).not.toHaveBeenCalled();
		expect(fx.close).toHaveBeenCalledOnce();
	});
	it('does not request the pool swap when owner verification fails after approval', async () => {
		const { OisySwapOwnerMismatchError } = await import('./swapOisySigner');
		fx.verify
			.mockResolvedValueOnce(undefined)
			.mockRejectedValueOnce(new OisySwapOwnerMismatchError());
		await expect(threePoolService.swap(2, 0, 1_000n, 990n)).rejects.toThrow(
			'Approval already succeeded. No swap was requested'
		);
		expect(fx.approve).toHaveBeenCalledOnce();
		expect(fx.swap).not.toHaveBeenCalled();
	});
	it('labels an approval-stage RPC502 separately from a swap-stage RPC502, without retrying either', async () => {
		fx.approve.mockRejectedValueOnce(mismatch());
		await expect(threePoolService.swap(2, 0, 1_000n, 990n)).rejects.toThrow(
			'approval and swap were not submitted'
		);
		expect(fx.approve).toHaveBeenCalledOnce();
		expect(fx.swap).not.toHaveBeenCalled();
		fx.approve.mockClear();
		fx.swap.mockRejectedValueOnce(mismatch());
		await expect(threePoolService.swap(2, 0, 1_000n, 990n)).rejects.toThrow(
			'Approval already succeeded. The wallet rejected the swap before submission'
		);
		expect(fx.approve).toHaveBeenCalledOnce();
		expect(fx.swap).toHaveBeenCalledOnce();
	});
	it('preserves unknown approval or swap transport failures without claiming owner rejection or retrying', async () => {
		fx.approve.mockRejectedValueOnce(new Error('unknown approval transport result'));
		await expect(threePoolService.swap(2, 0, 1_000n, 990n)).rejects.toThrow(
			'unknown approval transport result'
		);
		expect(fx.swap).not.toHaveBeenCalled();
		fx.approve.mockClear();
		fx.swap.mockRejectedValueOnce(new Error('unknown swap transport result'));
		await expect(threePoolService.swap(2, 0, 1_000n, 990n)).rejects.toThrow(
			'unknown swap transport result'
		);
		expect(fx.approve).toHaveBeenCalledOnce();
		expect(fx.swap).toHaveBeenCalledOnce();
	});
	it('preserves accepted swap success if popup cleanup fails', async () => {
		fx.close.mockRejectedValueOnce(new Error('cleanup failed'));
		await expect(threePoolService.swap(2, 0, 1_000n, 990n)).resolves.toBe(990n);
		expect(fx.swap).toHaveBeenCalledOnce();
	});
	it('reports completion for the original account when the session changes during an accepted swap', async () => {
		fx.swap.mockImplementationOnce(async () => {
			fx.generation++;
			return { Ok: 990n };
		});
		await expect(threePoolService.swap(2, 0, 1_000n, 990n)).rejects.toThrow(
			'swap completed for the original wallet'
		);
		expect(fx.approve).toHaveBeenCalledOnce();
		expect(fx.swap).toHaveBeenCalledOnce();
	});
	it.each(['plug', 'internet-identity'])(
		'preserves the standard %s swap path',
		async (provider) => {
			vi.useFakeTimers();
			fx.oisy = false;
			fx.provider = provider;
			const pending = threePoolService.swap(2, 0, 1_000n, 990n);
			await vi.advanceTimersByTimeAsync(2_000);
			await expect(pending).resolves.toBe(990n);
			expect(fx.begin).not.toHaveBeenCalled();
			expect(fx.approve).toHaveBeenCalledOnce();
			expect(fx.swap).toHaveBeenCalledOnce();
			vi.useRealTimers();
		}
	);
});
