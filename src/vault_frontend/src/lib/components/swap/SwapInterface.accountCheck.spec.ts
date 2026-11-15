import { mount, unmount, tick } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
const fx = vi.hoisted(() => ({ inspect: vi.fn(), execute: vi.fn() }));
vi.mock('../../services/swapOisySigner', () => ({ inspectOisySwapAccount: fx.inspect }));
vi.mock('../../services/auth', async () => {
	const { writable } = await import('svelte/store');
	return { currentWalletType: writable('oisy'), walletSessionGeneration: writable(0) };
});
vi.mock('../../stores/wallet', async () => {
	const { writable } = await import('svelte/store');
	return {
		walletStore: writable({ isConnected: true, loading: false, principal: null, tokenBalances: {} })
	};
});
vi.mock('../../services/ammService', () => ({
	AMM_SWAP_V2_READY: false,
	AMM_TOKENS: [
		{ ledgerId: 'rrkah-fqaaa-aaaaa-aaaaq-cai', symbol: 'icUSD', decimals: 8, color: '#aaa' },
		{ ledgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai', symbol: 'ckUSDC', decimals: 6, color: '#bbb' }
	],
	ammService: { getHeldOperations: () => [] },
	parseTokenAmount: () => 0n,
	formatTokenAmount: () => '0.00'
}));
vi.mock('../../services/swapRouter', () => ({
	resolveRoute: vi.fn(),
	executeRoute: fx.execute,
	preWarmOisySigner: vi.fn(),
	preWarmOisyFees: vi.fn(),
	checkIcpswapUnusedBalances: vi.fn().mockResolvedValue([]),
	recoverIcpswapBalance: vi.fn(),
	preWarmRecovery: vi.fn(),
	AMM1_ROUTING_PAUSED: true
}));
vi.mock('../../services/ledgerFeeService', () => ({
	fetchLedgerFee: vi.fn(),
	getCachedLedgerFee: () => 10n
}));
vi.mock('../../config', () => ({ CANISTER_IDS: {} }));
vi.mock('../../services/protocol/oisyResilience', () => ({
	callWithOisyFalseNegativeGuard: vi.fn(),
	isOisyLandedSentinel: () => false,
	isOisyArrFalseNegative: () => false
}));
vi.mock('../../services/tokenService', () => ({ TokenService: { getTokenBalance: vi.fn() } }));
import SwapInterface from './SwapInterface.svelte';
import { walletStore } from '../../stores/wallet';
import { currentWalletType } from '../../services/auth';
let component: ReturnType<typeof mount> | null = null,
	target: HTMLDivElement;
afterEach(async () => {
	if (component) await unmount(component);
	component = null;
	target?.remove();
	fx.inspect.mockReset();
	fx.execute.mockReset();
});
it('keeps inspection collapsed on a healthy OISY swap page and available through optional help', async () => {
	const owner = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai');
	(walletStore as any).set({
		isConnected: true,
		loading: false,
		principal: owner,
		tokenBalances: {}
	});
	currentWalletType.set('oisy');
	fx.inspect.mockResolvedValue({
		connectedOwner: owner.toText(),
		signerOwners: [owner.toText()],
		matches: true
	});
	target = document.createElement('div');
	document.body.append(target);
	component = mount(SwapInterface, { target });
	await tick();
	const check = [...target.querySelectorAll('button')].find((b) =>
		b.textContent?.includes('Check signing account (read only)')
	);
	const help = target.querySelector('details');
	expect(help).not.toBeNull();
	expect(help!.open).toBe(false);
	expect(help!.querySelector('summary')?.textContent).toContain('Wallet connection help');
	expect(help!.contains(check!)).toBe(true);
	expect(check).toBeDefined();
	expect(fx.inspect).not.toHaveBeenCalled();
	expect(fx.execute).not.toHaveBeenCalled();
	help!.querySelector('summary')!.click();
	await tick();
	expect(help!.open).toBe(true);
	expect(fx.inspect).not.toHaveBeenCalled();
	check!.click();
	await vi.waitFor(() => expect(fx.inspect).toHaveBeenCalledOnce());
	await tick();
	expect(target.textContent).toContain('matches this Rumi connection');
	expect(fx.execute).not.toHaveBeenCalled();
	currentWalletType.set('internet-identity');
	await tick();
	expect(
		[...target.querySelectorAll('button')].some((b) =>
			b.textContent?.includes('Check signing account')
		)
	).toBe(false);
});
