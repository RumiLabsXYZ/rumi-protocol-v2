import { mount, unmount } from 'svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

const fx = vi.hoisted(() => ({
	address: vi.fn(),
	info: vi.fn(),
	update: vi.fn(),
	redeem: vi.fn(),
	signer: vi.fn()
}));
vi.mock('$lib/stores/wallet', async () => {
	const { writable } = await import('svelte/store');
	const { Principal } = await import('@dfinity/principal');
	return {
		isConnected: writable(true),
		principal: writable(Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai'))
	};
});
vi.mock('$lib/services/auth', async () => {
	const { writable } = await import('svelte/store');
	return { walletSessionGeneration: writable(0) };
});
vi.mock('$lib/config', () => ({
	CANISTER_IDS: { CKDOGE_LEDGER: 'ryjl3-tyaaa-aaaaa-aaaba-cai', CKDOGE_MINTER: 'aaaaa-aa' }
}));
vi.mock('$lib/services/ckdogeMinterActors', () => ({
	getPublicMinterActor: async () => ({ get_doge_address: fx.address, get_minter_info: fx.info }),
	updateDogeBalanceForOwner: fx.update,
	redeemDogeWithApproval: fx.redeem,
	DogeWithdrawalPreflightError: class extends Error {}
}));
vi.mock('$lib/services/minterOisySigner', () => ({ beginMinterOisyAction: fx.signer }));
vi.mock('$lib/services/protocol/walletOperations', () => ({ isOisyWallet: () => false }));
vi.mock('$lib/services/ledgerFeeService', () => ({
	fetchLedgerFee: async () => 10_000n,
	getCachedLedgerFee: () => 10_000n
}));
vi.mock('$lib/utils/dogeMinterRecovery', () => ({
	readDogeWithdrawal: () => null,
	replaceDogeWithdrawal: vi.fn(),
	withDogeWithdrawalLock: (_owner: string, _minter: string, fn: () => unknown) => fn(),
	isDogeWithdrawalTerminal: () => false
}));
import Page from './+page.svelte';

let component: ReturnType<typeof mount> | null = null;
let target: HTMLDivElement;
const confirmedCopy = 'Deposit confirmed and waiting to mint. Confirmation count unavailable.';
const emptyCopy = 'No deposit detected on this address yet.';
const pill = () => target.querySelector('.doge-status-pill')?.textContent?.trim();

beforeEach(() => {
	fx.address.mockReset().mockResolvedValue('DmockDepositAddress');
	fx.info.mockReset().mockRejectedValue(new Error('Minter metadata unavailable'));
	fx.update.mockReset().mockResolvedValue({ Ok: [{ Checked: {} }] });
	fx.redeem.mockReset();
	fx.signer.mockReset();
	target = document.createElement('div');
	document.body.append(target);
});
afterEach(async () => {
	if (component) await unmount(component);
	component = null;
	target.remove();
	expect(fx.redeem).not.toHaveBeenCalled();
	expect(fx.signer).not.toHaveBeenCalled();
});

it('acknowledges a Checked deposit when minter confirmation metadata is unavailable', async () => {
	component = mount(Page, { target });
	await vi.waitFor(() => expect(pill()).toBe('Confirmed, waiting to mint'));
	expect(target.textContent).toContain(confirmedCopy);
	expect(target.textContent).not.toContain(emptyCopy);
	expect(target.querySelector('[role="progressbar"]')).toBeNull();
});

it('retains the real full confirmation meter when minter metadata is available', async () => {
	fx.info.mockResolvedValue({
		min_confirmations: 60,
		deposit_doge_min_amount: 1n,
		retrieve_doge_min_amount: 1n
	});
	component = mount(Page, { target });
	await vi.waitFor(() =>
		expect(target.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow')).toBe('60')
	);
	expect(pill()).toBe('Confirmed, waiting to mint');
	expect(target.querySelector('[role="progressbar"]')?.getAttribute('aria-valuemax')).toBe('60');
	expect(target.textContent).not.toContain(confirmedCopy);
	expect(target.textContent).not.toContain(emptyCopy);
});

it('keeps no-deposit copy for an empty result with unavailable metadata', async () => {
	fx.update.mockResolvedValue({ Ok: [] });
	component = mount(Page, { target });
	await vi.waitFor(() => expect(fx.update).toHaveBeenCalledOnce());
	await vi.waitFor(() => expect(target.textContent).toContain(emptyCopy));
	expect(target.textContent).not.toContain(confirmedCopy);
	expect(target.querySelector('[role="progressbar"]')).toBeNull();
});

it('does not override a minted result when another UTXO is still Checked', async () => {
	fx.update.mockResolvedValue({
		Ok: [{ Checked: {} }, { Minted: { minted_amount: 100_000_000n, block_index: 1n } }]
	});
	component = mount(Page, { target });
	await vi.waitFor(() => expect(pill()).toBe('Minted'));
	expect(target.textContent).not.toContain(confirmedCopy);
	expect(target.textContent).not.toContain(emptyCopy);
});
