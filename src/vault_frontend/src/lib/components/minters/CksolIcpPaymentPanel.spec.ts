import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import { Principal } from '@dfinity/principal';
const fx = vi.hoisted(() => ({
	q: vi.fn(),
	recover: vi.fn(),
	submit: vi.fn(),
	resume: vi.fn(),
	verify: vi.fn(),
	continue: vi.fn(),
	load: vi.fn(),
	open: vi.fn(),
	close: vi.fn(),
	wallet: 'plug'
}));
vi.mock('$lib/services/auth', () => ({
	auth: {
		subscribe(fn: any) {
			fn({ walletType: fx.wallet });
			return () => {};
		}
	}
}));
vi.mock('$lib/services/cksolOisySigner', () => ({
	beginCksolOisyAction: () => ({ ready: fx.open(), close: fx.close })
}));
vi.mock('$lib/services/cksolPayment', () => ({
	cksolPaymentKey: (owner: string) => `shared:${owner}`
}));
vi.mock('$lib/services/cksolIcpPayment', () => ({
	quoteCksolIcpPayment: fx.q,
	recoverCksolIcpPayment: fx.recover,
	submitCksolIcpPayment: fx.submit,
	resumeCksolIcpPayment: fx.resume,
	reconcileCksolIcpFunding: fx.verify,
	continueCksolIcpApproval: fx.continue,
	loadCksolIcpPayment: fx.load,
	icpPaymentSettled: (p: any) =>
		!p ||
		p.phase === 'NotStarted' ||
		(p.phase === 'Receipt' && ['Complete', 'FundingRejected'].includes(p.receipt?.phase))
}));
import Panel from './CksolIcpPaymentPanel.svelte';
const owner = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai');
let host: HTMLDivElement, instance: any;
const q = () => ({
	deposit: 26_295_000n,
	total: 26_315_000n,
	fee: 10_000n,
	balance: 100_000_000n,
	rate: 40_000n,
	terms: {
		available: true,
		attached_cycles: 1_000_000_000_000n,
		reserve_available_cycles: 2_000_000_000_000n
	}
});
const payment = (phase: string) => ({
	owner: owner.toText(),
	sequence: '1',
	createdAt: 1,
	phase: 'Receipt',
	receipt: {
		sequence: 1n,
		phase,
		deposit_e8s: 26_295_000n,
		charged_e8s: 1_200_000n,
		actual_refund_fee_e8s: 10_000n,
		refund_block: phase === 'Complete' ? [9n] : [],
		deposit_id: [3n]
	}
});
async function settle() {
	for (let i = 0; i < 80; i++) {
		await Promise.resolve();
		flushSync();
	}
}
const button = (label: string) =>
	[...host.querySelectorAll('button')].find((b) => b.textContent?.includes(label))!;
function render(props: any = {}) {
	instance = mount(Panel, { target: host, props: { owner, canMint: true, ...props } });
	flushSync();
}
beforeEach(() => {
	vi.resetAllMocks();
	fx.wallet = 'plug';
	fx.q.mockResolvedValue(q());
	fx.recover.mockResolvedValue(null);
	fx.load.mockReturnValue(null);
	fx.open.mockResolvedValue(undefined);
	fx.close.mockResolvedValue(undefined);
	Object.defineProperty(navigator, 'locks', {
		configurable: true,
		value: { request: async (_key: any, _opts: any, fn: any) => fn({}) }
	});
	host = document.createElement('div');
	document.body.appendChild(host);
});
afterEach(() => {
	if (instance) unmount(instance);
	host.remove();
});
describe('ICP-paid mint controls', () => {
	it('verifies a collection block separately from requesting its ICP refund', async () => {
		fx.wallet = 'oisy';
		const held = {
			...payment('FundingUnknown'),
			receipt: { ...payment('FundingUnknown').receipt, funding_block: [], deposit_id: [] }
		};
		fx.recover.mockResolvedValue(held);
		fx.verify.mockImplementation(async () => {
			const p = payment('RefundReady');
			fx.recover.mockResolvedValue(p);
			return p;
		});
		render({ blocked: true, canMint: false });
		await settle();
		const input = host.querySelector<HTMLInputElement>(
			'input[aria-label="ICP collection block number"]'
		)!;
		expect(button('Verify collection block').disabled).toBe(true);
		expect(host.textContent?.replace(/\s+/g, ' ')).toContain('not the approval block');
		expect(host.textContent?.replace(/\s+/g, ' ')).toContain(
			'Archived or nonmatching blocks leave this payment held'
		);
		input.value = '42';
		input.dispatchEvent(new Event('input', { bubbles: true }));
		flushSync();
		button('Verify collection block').click();
		expect(fx.open).toHaveBeenCalledOnce();
		await settle();
		expect(fx.verify).toHaveBeenCalledWith(held, '42', owner, localStorage, expect.any(Function));
		expect(fx.submit).not.toHaveBeenCalled();
		expect(fx.resume).not.toHaveBeenCalled();
		expect(button('Return unused ICP').disabled).toBe(false);
		expect(host.textContent?.replace(/\s+/g, ' ')).toContain('Collection verified');
	});
	it('offers only the interrupted ICP refund, then preserves the visible SOL hold after completion', async () => {
		const interrupted = (phase: string) => ({
			...payment(phase),
			receipt: {
				...payment(phase).receipt,
				deposit_id: [],
				charged_e8s: 0n,
				message: ['Processing was interrupted by an upgrade; SOL status remains unknown']
			}
		});
		fx.q.mockResolvedValue({
			...q(),
			balance: 0n,
			terms: { ...q().terms, available: false, reserve_available_cycles: 0n }
		});
		fx.recover.mockResolvedValue(interrupted('RefundReady'));
		fx.resume.mockImplementation(async () => {
			const p = interrupted('Complete');
			fx.recover.mockResolvedValue(p);
			return p;
		});
		const active = vi.fn();
		render({ blocked: true, canMint: false, onActive: active });
		await settle();
		expect(button('Return processing ICP').disabled).toBe(false);
		expect(host.textContent?.replace(/\s+/g, ' ')).toContain('SOL outcome unknown');
		expect(host.textContent?.replace(/\s+/g, ' ')).toContain('ICP ledger fees still apply');
		button('Return processing ICP').click();
		await settle();
		expect(fx.resume).toHaveBeenCalledOnce();
		expect(fx.submit).not.toHaveBeenCalled();
		expect(host.textContent?.replace(/\s+/g, ' ')).toContain('ICP refund confirmed');
		expect(host.textContent?.replace(/\s+/g, ' ')).toContain(
			'New minting with this account remains blocked'
		);
		expect(button('New mint held for review').disabled).toBe(true);
		expect(active).toHaveBeenLastCalledWith(true);
	});
	it('shows the fee and maximum before enabling the mint, with no automatic spending', async () => {
		render();
		await settle();
		expect(host.textContent).toContain('5% of the minter');
		expect(host.textContent).toContain('0.26315000 ICP');
		expect(host.textContent?.replace(/\s+/g, ' ')).toContain('Unused funds return as ICP');
		expect(button('Mint with ICP').disabled).toBe(false);
		expect(fx.submit).not.toHaveBeenCalled();
	});
	it('keeps the mint disabled when ICP is insufficient and does not request cycles from the user', async () => {
		fx.q.mockResolvedValue({ ...q(), balance: 0n });
		const ready = vi.fn();
		render({ onReady: ready });
		await settle();
		expect(host.textContent).toContain('Add at least 0.26315000 ICP');
		expect(button('Mint with ICP').disabled).toBe(true);
		expect(ready).toHaveBeenLastCalledWith(false);
		expect(fx.submit).not.toHaveBeenCalled();
	});
	it('gates new SOL instructions on the processing reserve and balance', async () => {
		fx.q.mockResolvedValue({
			...q(),
			terms: { ...q().terms, available: false, reserve_available_cycles: 0n }
		});
		render({ canMint: false });
		await settle();
		expect(host.textContent).toContain('Do not send more SOL');
		expect(host.textContent).not.toContain('Your ICP balance covers the maximum');
		expect(button('Mint with ICP').disabled).toBe(true);
	});
	it('guides the already-received SOL deposit through one explicit user mint action', async () => {
		fx.submit.mockImplementation(async () => {
			const p = payment('DepositPending');
			fx.recover.mockResolvedValue(p);
			return p;
		});
		const receipt = vi.fn();
		render({ onReceipt: receipt });
		await settle();
		button('Mint with ICP').click();
		await settle();
		expect(fx.submit).toHaveBeenCalledOnce();
		expect(receipt).toHaveBeenCalledWith(expect.objectContaining({ phase: 'DepositPending' }));
		expect(host.textContent).toContain('Checking the existing payment');
	});
	it('can return ICP while SOL minting is still in progress without collecting again', async () => {
		fx.recover.mockResolvedValue(payment('RefundReady'));
		fx.resume.mockResolvedValue(payment('Complete'));
		render({ blocked: true, canMint: false });
		await settle();
		expect(button('Return unused ICP').disabled).toBe(false);
		button('Return unused ICP').click();
		await settle();
		expect(fx.resume).toHaveBeenCalledOnce();
		expect(fx.submit).not.toHaveBeenCalled();
	});
	it('does not offer another approval for a held funding result', async () => {
		fx.recover.mockResolvedValue(payment('FundingUnknown'));
		render();
		await settle();
		expect(button('Mint with ICP').disabled).toBe(true);
		expect(host.textContent).toContain('Do not approve another deposit');
		expect(fx.submit).not.toHaveBeenCalled();
	});
	it('restores a settled ICP receipt without claiming the ckSOL mint has finished', async () => {
		fx.recover.mockResolvedValue(payment('Complete'));
		render();
		await settle();
		expect(host.textContent).toContain('ICP refund confirmed');
		expect(host.textContent).toContain('Processing payment settled');
		expect(host.textContent).toMatch(/Payment settlement does\s+not mean ckSOL has finished minting/);
		expect(host.textContent).not.toContain('ckSOL received');
	});
	it('opens the OISY channel on the action and closes it after the known result', async () => {
		fx.wallet = 'oisy';
		fx.submit.mockResolvedValue(payment('DepositPending'));
		render();
		await settle();
		button('Mint with ICP').click();
		expect(fx.open).toHaveBeenCalledOnce();
		await settle();
		expect(fx.submit).toHaveBeenCalledOnce();
		expect(fx.close).toHaveBeenCalledOnce();
	});
	it('retains damaged journals and blocks a new deposit', async () => {
		fx.recover.mockRejectedValue(new Error('malformed'));
		fx.load.mockImplementation(() => {
			throw new Error('invalid');
		});
		render();
		await settle();
		expect(host.textContent).toContain('record is preserved');
		expect(button('Mint with ICP').disabled).toBe(true);
		expect(fx.submit).not.toHaveBeenCalled();
	});
});

describe('bounded ICP receipt checks', () => {
  it('does not poll a completed inactive flow but allows one explicit read', async () => {
    vi.useFakeTimers(); fx.recover.mockResolvedValue(payment('Complete'));
    render({pollingActive:false}); await settle(); const reads=fx.recover.mock.calls.length;
    await vi.advanceTimersByTimeAsync(180_000); await settle(); expect(fx.recover).toHaveBeenCalledTimes(reads);
    await instance.checkAgain(); await settle(); expect(fx.recover).toHaveBeenCalledTimes(reads+1);
    await vi.advanceTimersByTimeAsync(180_000); await settle(); expect(fx.recover).toHaveBeenCalledTimes(reads+1);
    vi.useRealTimers();
  });
  it('continues an outstanding refund even when deposit polling has stopped', async () => {
    vi.useFakeTimers(); fx.recover.mockResolvedValue(payment('RefundReady'));
    render({pollingActive:false}); await settle(); const reads=fx.recover.mock.calls.length;
    await vi.advanceTimersByTimeAsync(15_000); await settle(); expect(fx.recover.mock.calls.length).toBeGreaterThan(reads);
    expect(fx.submit).not.toHaveBeenCalled(); vi.useRealTimers();
  });
});

describe('unavailable receipt is not settlement proof', () => {
  it('keeps retrying a failed authoritative read even without a local journal', async () => {
    vi.useFakeTimers();fx.recover.mockRejectedValue(new Error('receipt endpoint unavailable'));fx.load.mockReturnValue(null);
    const active=vi.fn();render({pollingActive:false,onActive:active});await settle();
    expect(active).toHaveBeenLastCalledWith(true);
    const reads=fx.recover.mock.calls.length;
    await vi.advanceTimersByTimeAsync(15_000);await settle();expect(fx.recover.mock.calls.length).toBeGreaterThan(reads);
    expect(fx.submit).not.toHaveBeenCalled();vi.useRealTimers();
  });
});

describe('observational mint status callbacks', () => {
  it('reports the saved request identity and successful check without changing payment behavior', async () => {
    const observed = vi.fn(); fx.recover.mockResolvedValue(payment('DepositPending'));
    render({onProgress:observed}); await settle();
    expect(observed).toHaveBeenLastCalledWith(expect.objectContaining({
      sequence:'1', phase:'DepositPending', depositId:3n, checking:false,
      lastChecked:expect.any(Number), unavailable:false,
    }));
    expect(fx.submit).not.toHaveBeenCalled(); expect(fx.resume).not.toHaveBeenCalled();
    fx.recover.mockRejectedValueOnce(new Error('synthetic receipt unavailable'));
    await instance.checkAgain(); await settle();
    expect(observed).toHaveBeenLastCalledWith(expect.objectContaining({unavailable:true}));
    expect(fx.submit).not.toHaveBeenCalled();
  });
  it('announces a user mint action before its result, and hides new-mint controls after verified success', async () => {
    const started = vi.fn(); let resolve!: (p:any)=>void;
    fx.submit.mockImplementationOnce(()=>new Promise(r=>resolve=r));
    render({onMintStart:started}); await settle();
    button('Mint with ICP').click(); await settle();
    expect(started).toHaveBeenCalledOnce(); expect(fx.submit).toHaveBeenCalledOnce();
    const result=payment('Complete'); fx.recover.mockResolvedValue(result); resolve(result); await settle();
    unmount(instance); instance=null;
    render({mintComplete:true}); await settle();
    expect(button('Mint with ICP')).toBeUndefined();
  });
  it('retains a refund action after verified mint success without announcing another mint', async () => {
    const started=vi.fn(); fx.recover.mockResolvedValue(payment('RefundReady'));
    fx.resume.mockResolvedValue(payment('Complete'));
    render({mintComplete:true,onMintStart:started}); await settle();
    button('Return unused ICP').click(); await settle();
    expect(fx.resume).toHaveBeenCalledOnce(); expect(started).not.toHaveBeenCalled();
    expect(fx.submit).not.toHaveBeenCalled();
  });
});
