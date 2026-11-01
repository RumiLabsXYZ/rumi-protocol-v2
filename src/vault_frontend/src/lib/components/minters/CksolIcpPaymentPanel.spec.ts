import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import { Principal } from '@dfinity/principal';
const fx = vi.hoisted(() => ({
	q: vi.fn(),
	recover: vi.fn(),
	submit: vi.fn(),
	resume: vi.fn(),
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
		expect(host.textContent).toContain('Mint progress is shown separately');
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
