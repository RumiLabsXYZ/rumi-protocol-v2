import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';
import { Principal } from '@dfinity/principal';
import { ckErc20WithdrawalLockName, withCkErc20WithdrawalLock, type CkErc20LockManager } from '$lib/utils/ckerc20WithdrawalLock';

const mocks = vi.hoisted(() => {
	function readable<T>(initial: T) {
		let value = initial;
		const listeners = new Set<(next: T) => void>();
		return {
			subscribe(fn: (next: T) => void) { fn(value); listeners.add(fn); return () => listeners.delete(fn); },
			set(next: T) { value = next; for (const listener of listeners) listener(next); },
		};
	}
	return {
		isConnected: readable(false),
		principal: readable(null as any),
		getMinter: vi.fn(),
		discoverTokens: vi.fn(),
		getLedger: vi.fn(),
		parseTokenAmount: vi.fn(),
	};
});

vi.mock('$lib/stores/wallet', () => ({
	isConnected: mocks.isConnected,
	principal: mocks.principal,
}));

vi.mock('$lib/config', () => ({
	CANISTER_IDS: {
		CKUSDC_LEDGER: 'xevnm-gaaaa-aaaar-qafnq-cai',
		CKETH_LEDGER: 'ss2fx-dyaaa-aaaar-qacoq-cai',
		CKERC20_MINTER: 'sv3dd-oaaaa-aaaar-qacoa-cai',
	},
}));

vi.mock('$lib/services/ckerc20Minter', () => ({
	CKERC20_MINTER_DASHBOARD: 'https://example.invalid/dashboard',
	discoverCkErc20Tokens: mocks.discoverTokens,
	getCkErc20MinterActor: mocks.getMinter,
	getCkErc20LedgerActor: mocks.getLedger,
	approveAndWithdrawCkErc20: vi.fn(),
	assertTokenSupported: vi.fn(),
	encodeAddressWord: vi.fn(() => '0'.repeat(64)),
	encodeDepositErc20: vi.fn(),
	encodeUint256: vi.fn(),
	formatTokenAmount: (amount: bigint, decimals = 6, maxFraction = 6) => {
		const scale = 10n ** BigInt(decimals);
		const whole = amount / scale;
		const fraction = (amount % scale).toString().padStart(decimals, '0').slice(0, maxFraction).replace(/0+$/, '');
		return fraction ? `${whole}.${fraction}` : whole.toString();
	},
	getCkErc20WithdrawalQuote: vi.fn(),
	parseTokenAmount: mocks.parseTokenAmount,
	validateEthereumAddress: vi.fn(() => true),
}));

import Page from './+page.svelte';

const OWNER = Principal.fromUint8Array(Uint8Array.of(1));
const CKUSDC = {
	symbol: 'ckUSDC', decimals: 6, ledgerId: 'xevnm-gaaaa-aaaar-qafnq-cai',
	erc20Address: '0x1111111111111111111111111111111111111111', minimumDepositAmount: 1n,
};
const CKLINK = {
	symbol: 'ckLINK', decimals: 18, ledgerId: 'g4tto-rqaaa-aaaar-qageq-cai',
	erc20Address: '0x2222222222222222222222222222222222222222', minimumDepositAmount: 1n,
};
const CKETH_LEDGER = 'ss2fx-dyaaa-aaaar-qacoq-cai';
const ACCOUNT = '0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const HELPER = '0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const DISCONNECT_KEY = 'rumi:ckerc20:evm-disconnected';

type ProviderListener = (...args: any[]) => void;
type Provider = {
	request: (args: { method: string; params?: unknown[] }) => Promise<any>;
	on: (event: string, listener: ProviderListener) => void;
	removeListener: (event: string, listener: ProviderListener) => void;
};

function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((done) => { resolve = done; });
	return { promise, resolve };
}

function makeProvider(overrides: Partial<Record<string, (params?: unknown[]) => Promise<any>>> = {}) {
	const listeners = new Map<string, ProviderListener>();
	const provider: Provider = {
		request: vi.fn(async ({ method, params }: { method: string; params?: unknown[] }) => {
			if (overrides[method]) return overrides[method]!(params);
			if (method === 'eth_accounts') return [ACCOUNT];
			if (method === 'eth_requestAccounts') return [ACCOUNT];
			if (method === 'eth_chainId') return '0x1';
			if (method === 'eth_call') {
				const call = params?.[0] as { data?: string; to?: string } | undefined;
				if (call?.data?.startsWith('0x70a08231')) return `0x${(2_000_000n).toString(16)}`;
				if (call?.data?.startsWith('0xdd62ed3e')) return '0x0';
				if (call?.data !== '0x313ce567') return '0x0';
				return call.to?.toLowerCase() === CKLINK.erc20Address.toLowerCase() ? '0x12' : '0x6';
			}
			if (method === 'eth_sendTransaction') return `0x${'1'.repeat(64)}`;
			if (method === 'eth_getTransactionReceipt') return { status: '0x1' };
			throw new Error(`Unexpected EVM method ${method}`);
		}),
		on(event, listener) { listeners.set(event, listener); },
		removeListener(event, listener) { if (listeners.get(event) === listener) listeners.delete(event); },
	};
	return { provider, listeners };
}

async function settle(rounds = 12) {
	for (let i = 0; i < rounds; i++) {
		await Promise.resolve();
		await tick();
		flushSync();
	}
}

let host: HTMLDivElement;
let instance: unknown;
let provider: Provider;
let providerFixture: ReturnType<typeof makeProvider>;
let priorLocksDescriptor: PropertyDescriptor | undefined;

function render() {
	instance = mount(Page, { target: host });
	flushSync();
}

function clickButton(text: RegExp) {
	const button = Array.from(host.querySelectorAll('button')).find((candidate) => text.test(candidate.textContent ?? ''));
	expect(button, `button matching ${text}`).toBeTruthy();
	button!.click();
	flushSync();
	return button!;
}

async function chooseMintToken(ledgerId: string, symbol: string) {
	const trigger = host.querySelector<HTMLButtonElement>('.amount-input [aria-haspopup="listbox"]');
	expect(trigger, 'mint token selector').toBeTruthy();
	trigger!.click();
	await settle();
	const option = Array.from(host.querySelectorAll<HTMLButtonElement>('[role="option"]')).find((candidate) => candidate.textContent?.includes(symbol));
	expect(option, `option for ${symbol}`).toBeTruthy();
	option!.click();
	flushSync();
}

beforeEach(() => {
	priorLocksDescriptor = Object.getOwnPropertyDescriptor(navigator, 'locks');
	Object.defineProperty(navigator, 'locks', {
		configurable: true,
		value: { request: async (_name: string, _options: unknown, callback: (lock: unknown) => unknown) => callback({}) },
	});
	sessionStorage.clear();
	localStorage.clear();
	host = document.createElement('div');
	document.body.appendChild(host);
	mocks.isConnected.set(true);
	mocks.principal.set(OWNER);
	mocks.getMinter.mockReset().mockResolvedValue({ get_minter_info: vi.fn().mockResolvedValue({
		deposit_with_subaccount_helper_contract_address: [HELPER],
		minimum_deposit_amounts: [[
			{ erc20_contract_address: CKUSDC.erc20Address, minimum_deposit_amount: 1n },
			{ erc20_contract_address: CKLINK.erc20Address, minimum_deposit_amount: 1n },
		]],
	}) });
	mocks.discoverTokens.mockReset().mockResolvedValue([CKUSDC, CKLINK]);
	mocks.getLedger.mockReset().mockImplementation((ledgerId: string) => Promise.resolve({
		icrc1_total_supply: vi.fn().mockResolvedValue(ledgerId === CKLINK.ledgerId ? 900_000_000_000_000_000n : 50_000_000n),
		icrc1_balance_of: vi.fn().mockResolvedValue(0n),
	}));
	providerFixture = makeProvider();
	provider = providerFixture.provider;
	mocks.parseTokenAmount.mockReset().mockImplementation((value: string, decimals: number) => {
		const [whole, fraction = ''] = value.split('.');
		return BigInt(whole) * 10n ** BigInt(decimals) + BigInt((fraction + '0'.repeat(decimals)).slice(0, decimals) || '0');
	});
	Object.defineProperty(window, 'ethereum', { configurable: true, value: provider });
	flushSync();
});

afterEach(() => {
	if (instance) unmount(instance as any);
	instance = undefined;
	host.remove();
	delete (window as Window & { ethereum?: Provider }).ethereum;
	if (priorLocksDescriptor) Object.defineProperty(navigator, 'locks', priorLocksDescriptor);
	else delete (navigator as unknown as { locks?: unknown }).locks;
	vi.restoreAllMocks();
});

describe('ckERC20 page wallet and supply regressions', () => {
	it('does not send a deposit after the EVM account changes and changes back during approval confirmation', async () => {
		const approvalReceipt = deferred<{ status: string }>();
		providerFixture = makeProvider({ eth_getTransactionReceipt: () => approvalReceipt.promise });
		provider = providerFixture.provider;
		Object.defineProperty(window, 'ethereum', { configurable: true, value: provider });
		render();
		await settle();
		const amount = host.querySelector<HTMLInputElement>('#deposit-amount')!;
		amount.value = '1';
		amount.dispatchEvent(new Event('input', { bubbles: true }));
		flushSync();
		clickButton(/Approve and mint/);
		for (let i = 0; i < 20 && !(provider.request as any).mock.calls.some((call: any[]) => call[0]?.method === 'eth_sendTransaction'); i++) await settle();
		expect((provider.request as any).mock.calls.filter((call: any[]) => call[0]?.method === 'eth_sendTransaction')).toHaveLength(1);
		providerFixture.listeners.get('accountsChanged')?.(['0xcccccccccccccccccccccccccccccccccccccccc']);
		providerFixture.listeners.get('accountsChanged')?.([ACCOUNT]);
		approvalReceipt.resolve({ status: '0x1' });
		await settle(30);

		expect((provider.request as any).mock.calls.filter((call: any[]) => call[0]?.method === 'eth_sendTransaction')).toHaveLength(1);
		expect(host.textContent).toContain('account, network, or provider changed');
	});

	it('uses one exclusive withdrawal lock for different ckERC20 tokens sharing an owner allowance', async () => {
		const ownerText = OWNER.toText();
		const usdcLock = ckErc20WithdrawalLockName(ownerText, CKUSDC.ledgerId);
		const linkLock = ckErc20WithdrawalLockName(ownerText, CKLINK.ledgerId);
		expect(usdcLock).toBe(linkLock);

		const held = new Set<string>();
		const fakeLocks: CkErc20LockManager = {
			async request<T>(name: string, _options: { mode: 'exclusive'; ifAvailable: true }, callback: (lock: unknown | null) => Promise<T>): Promise<T> {
				if (held.has(name)) return callback(null);
				held.add(name);
				try { return await callback({}); } finally { held.delete(name); }
			},
		};
		let releaseFirst!: () => void;
		const firstGate = new Promise<void>((resolve) => { releaseFirst = resolve; });
		const first = withCkErc20WithdrawalLock(fakeLocks, ownerText, CKUSDC.ledgerId, async (lock) => {
			expect(lock).toBeTruthy();
			await firstGate;
		});
		let secondAcquired = true;
		await withCkErc20WithdrawalLock(fakeLocks, ownerText, CKLINK.ledgerId, async (lock) => { secondAcquired = !!lock; });
		expect(secondAcquired).toBe(false);
		releaseFirst();
		await first;
	});

	it('disconnects locally, ignores late balance/account updates, and remembers the choice after remount', async () => {
		const lateBalance = deferred<string>();
		const fixture = makeProvider({
			eth_call: (params) => {
				const call = (params?.[0] as { data?: string } | undefined);
				return call?.data === '0x313ce567' ? Promise.resolve('0x6') : lateBalance.promise;
			},
		});
		provider = fixture.provider;
		Object.defineProperty(window, 'ethereum', { configurable: true, value: provider });
		render();
		await settle();
		expect((provider.request as any).mock.calls.some((call: any[]) => call[0]?.method === 'eth_call' && call[0]?.params?.[0]?.data?.startsWith('0x70a08231'))).toBe(true);
		clickButton(/^Disconnect$/);
		fixture.listeners.get('accountsChanged')?.(['0xcccccccccccccccccccccccccccccccccccccccc']);
		lateBalance.resolve(`0x${(12_000_000n).toString(16)}`);
		await settle();
		expect(sessionStorage.getItem(DISCONNECT_KEY)).toBe('true');
		expect(host.textContent).toContain('Ethereum wallet');
		expect(host.textContent).toContain('Connect Ethereum to view balance');
		expect(host.textContent).not.toContain('0xcccccc');
		expect(host.querySelector<HTMLButtonElement>('.amount-actions button:last-child')?.disabled).toBe(true);

		unmount(instance as any);
		instance = undefined;
		host.replaceChildren();
		(provider.request as any).mockClear();
		render();
		await settle();
		expect(provider.request).not.toHaveBeenCalledWith({ method: 'eth_accounts' });
		expect(host.textContent).toContain('Ethereum wallet');
		expect(host.textContent).toContain('Connect Ethereum');
	});

	it('keeps an unresolved deposit marker across disconnect and restores its lock on explicit reconnect', async () => {
		const storageKey = `rumi:ckerc20:pending-deposit:${ACCOUNT}:${OWNER.toText()}:${CKUSDC.ledgerId}`;
		localStorage.setItem(storageKey, JSON.stringify({
			hash: '0xdeadbeef', amount: '2', recipient: OWNER.toText(), evmAccount: ACCOUNT,
			principal: OWNER.toText(), tokenLedgerId: CKUSDC.ledgerId, tokenSymbol: CKUSDC.symbol, createdAt: Date.now(),
		}));
		render();
		await settle();
		expect(host.textContent).toContain('A previous ckUSDC deposit is unresolved');
		clickButton(/^Disconnect$/);
		expect(localStorage.getItem(storageKey)).toContain('0xdeadbeef');
		expect(host.textContent).not.toContain('A previous ckUSDC deposit is unresolved');
		clickButton(/Connect Ethereum/);
		await settle();
		expect(host.textContent).toContain('A previous ckUSDC deposit is unresolved');
		expect(host.querySelector<HTMLButtonElement>('.primary')?.disabled).toBe(true);
		expect(provider.request).toHaveBeenCalledWith({ method: 'eth_requestAccounts' });
	});

	it('keeps old supply responses under their original token and leaves deposit enabled when stats fail', async () => {
		const oldSupply = deferred<bigint>();
		let firstUsdcSupply = true;
		mocks.getLedger.mockImplementation((ledgerId: string) => Promise.resolve({
			icrc1_total_supply: vi.fn(() => {
				if (ledgerId === CKUSDC.ledgerId && firstUsdcSupply) { firstUsdcSupply = false; return oldSupply.promise; }
				if (ledgerId === CKLINK.ledgerId) return Promise.resolve(900_000_000_000_000_000n);
				if (ledgerId === CKETH_LEDGER) return Promise.reject(new Error('stats unavailable'));
				return Promise.resolve(50_000_000n);
			}),
			icrc1_balance_of: vi.fn().mockResolvedValue(0n),
		}));
		render();
		await settle();
		const depositInput = host.querySelector<HTMLInputElement>('#deposit-amount')!;
		depositInput.value = '1.25';
		depositInput.dispatchEvent(new Event('input', { bubbles: true }));
		flushSync();
		await chooseMintToken(CKLINK.ledgerId, CKLINK.symbol);
		await settle();
		expect(host.querySelector<HTMLInputElement>('#deposit-amount')?.value).toBe('');
		expect(host.textContent).toContain('ckLINK');
		oldSupply.resolve(5_000_000_000_000_000_000n);
		await settle();
		expect(host.textContent).toContain('Some supply data is unavailable');
		const selectedSupply = host.querySelector('[aria-label="Circulating supply on ICP"] .supply-row strong');
		expect(selectedSupply?.textContent).toContain('0.9');
		expect(selectedSupply?.textContent).not.toContain('5');
		expect(host.querySelector<HTMLButtonElement>('.primary')?.disabled).toBe(false);
	});
});
