import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';
import { Principal } from '@dfinity/principal';
import type { CkErc20LockManager } from '$lib/utils/ckerc20WithdrawalLock';

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
		generation: readable(0),
		getMinter: vi.fn(),
		discoverTokens: vi.fn(),
		getLedger: vi.fn(),
		getQuote: vi.fn(),
		parseAmount: vi.fn(),
		approveAndWithdraw: vi.fn(),
		isOisy: vi.fn(),
		beginAction: vi.fn(),
	};
});

vi.mock('$lib/services/auth', () => ({ walletSessionGeneration: mocks.generation }));

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
	isCkErc20OisyWallet: mocks.isOisy,
	CKERC20_MINTER_DASHBOARD: 'https://example.invalid/dashboard',
	discoverCkErc20Tokens: mocks.discoverTokens,
	getCkErc20MinterActor: mocks.getMinter,
	getCkErc20LedgerActor: mocks.getLedger,
	approveAndWithdrawCkErc20: mocks.approveAndWithdraw,
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
	getCkErc20WithdrawalQuote: mocks.getQuote,
	parseTokenAmount: mocks.parseAmount,
	validateEthereumAddress: vi.fn(() => true),
}));

vi.mock('$lib/services/minterOisySigner', () => ({ beginMinterOisyAction: mocks.beginAction }));

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
				if (call?.data !== '0x313ce567') return '0x0';
				return call.to?.toLowerCase() === CKLINK.erc20Address.toLowerCase() ? '0x12' : '0x6';
			}
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
	vi.useFakeTimers();
	Object.defineProperty(navigator, 'locks', { configurable: true, value: { request: vi.fn(async (_name, _options, callback) => callback({})) } });
	sessionStorage.clear();
	localStorage.clear();
	host = document.createElement('div');
	document.body.appendChild(host);
	mocks.generation.set(0);
	mocks.isConnected.set(true);
	mocks.principal.set(OWNER);
	mocks.getMinter.mockReset().mockResolvedValue({ get_minter_info: vi.fn().mockResolvedValue({ deposit_with_subaccount_helper_contract_address: [HELPER] }), get_events: vi.fn().mockResolvedValue({ events: [], total_event_count: 0n }) });
	mocks.discoverTokens.mockReset().mockResolvedValue([CKUSDC, CKLINK]);
	mocks.parseAmount.mockReset().mockReturnValue(1n);
	mocks.getQuote.mockReset().mockImplementation((token: typeof CKUSDC, amount: bigint, owner: Principal) => Promise.resolve({
		owner, tokenLedgerId: token.ledgerId, amount, ckTokenFee: 1n, ckEthFee: 1n,
		maxTransactionFee: 1n, ckTokenAllowance: 2n, ckEthAllowance: 2n,
		ckTokenBalance: 10n, ckEthBalance: 10n, quotedAtMs: Date.now(), minterPriceTimestampMs: null,
	}));
	mocks.approveAndWithdraw.mockReset();
	mocks.isOisy.mockReset().mockReturnValue(false);
	mocks.beginAction.mockReset();
	mocks.getLedger.mockReset().mockImplementation((ledgerId: string) => Promise.resolve({
		icrc1_total_supply: vi.fn().mockResolvedValue(ledgerId === CKLINK.ledgerId ? 900_000_000_000_000_000n : 50_000_000n),
		icrc1_balance_of: vi.fn().mockResolvedValue(0n),
	}));
	const fixture = makeProvider();
	provider = fixture.provider;
	Object.defineProperty(window, 'ethereum', { configurable: true, value: provider });
	flushSync();
});

afterEach(() => {
	if (instance) unmount(instance as any);
	host.remove();
	delete (window as Window & { ethereum?: Provider }).ethereum;
	vi.restoreAllMocks();
	vi.useRealTimers();
});

describe('ckERC20 page wallet and supply regressions', () => {
	it('holds one owner lock across different-token withdrawal submissions', async () => {
		const held = new Set<string>();
		const names: string[] = [];
		const locks: CkErc20LockManager = {
			async request<T>(name: string, _options: { mode: 'exclusive'; ifAvailable: true }, callback: (lock: unknown | null) => Promise<T>): Promise<T> {
				names.push(name);
				if (held.has(name)) return callback(null);
				held.add(name);
				try { return await callback({}); } finally { held.delete(name); }
			},
		};
		const oldLocks = Object.getOwnPropertyDescriptor(navigator, 'locks');
		Object.defineProperty(navigator, 'locks', { configurable: true, value: locks });
		const releaseFirst = deferred<{ ckEthBurnBlock: bigint; ckTokenBurnBlock: bigint }>();
		mocks.approveAndWithdraw.mockImplementationOnce(() => releaseFirst.promise)
			.mockResolvedValue({ ckEthBurnBlock: 3n, ckTokenBurnBlock: 4n });
		const firstHost = host;
		let secondHost: HTMLDivElement | undefined;
		let secondInstance: unknown;
		try {
			render();
			await settle();
			clickButton(/^Redeem$/);
			await settle();
			const firstAmount = host.querySelector<HTMLInputElement>('#redeem-amount')!;
			firstAmount.value = '1';
			firstAmount.dispatchEvent(new Event('input', { bubbles: true }));
			const firstAddress = host.querySelector<HTMLInputElement>('#redeem-address')!;
			firstAddress.value = ACCOUNT;
			firstAddress.dispatchEvent(new Event('input', { bubbles: true }));
			await settle();
			await vi.advanceTimersByTimeAsync(400);
			await settle();
			clickButton(/Approve and request USDC redemption/);
			await settle();
			expect(mocks.approveAndWithdraw).toHaveBeenCalledTimes(1);

			secondHost = document.createElement('div');
			document.body.appendChild(secondHost);
			host = secondHost;
			secondInstance = mount(Page, { target: secondHost });
			flushSync();
			await settle();
			clickButton(/^Redeem$/);
			await settle();
			await chooseMintToken(CKLINK.ledgerId, CKLINK.symbol);
			await settle();
			const secondAmount = host.querySelector<HTMLInputElement>('input[id="redeem-amount"]')!;
			secondAmount.value = '1';
			secondAmount.dispatchEvent(new Event('input', { bubbles: true }));
			const secondAddress = host.querySelector<HTMLInputElement>('input[id="redeem-address"]')!;
			secondAddress.value = ACCOUNT;
			secondAddress.dispatchEvent(new Event('input', { bubbles: true }));
			await settle();
			await vi.advanceTimersByTimeAsync(400);
			await settle();
			clickButton(/Approve and request LINK redemption/);
			await settle();
			expect(names).toEqual([`rumi:ckerc20:withdrawal:${OWNER.toText()}`, `rumi:ckerc20:withdrawal:${OWNER.toText()}`]);
			expect(mocks.approveAndWithdraw).toHaveBeenCalledTimes(1);
			expect(host.textContent).toContain('withdrawal for this wallet is active in another tab');
		} finally {
			releaseFirst.resolve({ ckEthBurnBlock: 1n, ckTokenBurnBlock: 2n });
			await settle();
			if (secondInstance) unmount(secondInstance as any);
			secondHost?.remove();
			host = firstHost;
			if (oldLocks) Object.defineProperty(navigator, 'locks', oldLocks);
			else Reflect.deleteProperty(navigator, 'locks');
		}
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
			hash: HASH, amount: '2', recipient: OWNER.toText(), evmAccount: ACCOUNT,
			principal: OWNER.toText(), tokenLedgerId: CKUSDC.ledgerId, tokenSymbol: CKUSDC.symbol, createdAt: Date.now(),
		}));
		render();
		await settle();
		expect(host.textContent).toContain('A previous ckUSDC deposit is unresolved');
		clickButton(/^Disconnect$/);
		expect(localStorage.getItem(storageKey)).toContain(HASH);
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

const HASH = `0x${'ab'.repeat(32)}`;
const depositKey = () => `rumi:ckerc20:pending-deposit:${ACCOUNT}:${OWNER.toText()}:${CKUSDC.ledgerId}`;
const withdrawalKey = () => `rumi:ckerc20:pending-withdrawal:${OWNER.toText()}`;
function savedDeposit(extra: Record<string, unknown> = {}) {
  return JSON.stringify({ hash: HASH, amount: '2', recipient: OWNER.toText(), evmAccount: ACCOUNT, principal: OWNER.toText(), tokenLedgerId: CKUSDC.ledgerId, tokenSymbol: CKUSDC.symbol, createdAt: 10, ...extra });
}
function savedWithdrawal(extra: Record<string, unknown> = {}) {
  return JSON.stringify({ amount: '1', recipient: ACCOUNT, owner: OWNER.toText(), tokenLedgerId: CKUSDC.ledgerId, tokenSymbol: CKUSDC.symbol, createdAt: 10, ckEthBurn: '3', ckTokenBurn: '4', ...extra });
}
function receipt(status = '0x1') { return { transactionHash: HASH, status, blockHash: `0x${'cd'.repeat(32)}`, blockNumber: '0x10' }; }
function setInput(selector: string, value: string) {
  const input = host.querySelector<HTMLInputElement>(selector)!;
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
}
function installStatus(status: any, extra: Record<string, unknown> = {}) {
  const withdrawal_status = vi.fn().mockResolvedValue([{ withdrawal_id: 3n, from: OWNER, from_subaccount: [], token_symbol: 'ckUSDC', withdrawal_amount: 1n, recipient_address: ACCOUNT, status, ...extra }]);
  mocks.getMinter.mockResolvedValue({ get_minter_info: vi.fn().mockResolvedValue({ deposit_with_subaccount_helper_contract_address: [HELPER] }), withdrawal_status });
  return withdrawal_status;
}

describe('ckERC20 automatic lifecycle reconciliation', () => {
  it.each(['0x1', '0x0'])('reconciles a saved deposit receipt %s without sending another transaction', async (status) => {
    localStorage.setItem(depositKey(), savedDeposit());
    const fixture = makeProvider({ eth_getTransactionReceipt: async () => receipt(status), eth_getBlockByNumber: async (params) => params?.[0] === 'finalized' ? { number: '0x20' } : { number: '0x10', hash: `0x${'cd'.repeat(32)}` } });
    Object.defineProperty(window, 'ethereum', { configurable: true, value: fixture.provider });
    render(); await settle();
    expect(localStorage.getItem(depositKey())).toBeNull();
    expect(host.querySelector<HTMLButtonElement>('.primary')?.disabled).toBe(status === '0x1');
    if (status === '0x1') {
      expect(localStorage.getItem(`${depositKey()}:confirmed`)).toContain(HASH);
      expect(host.textContent).toContain('Ethereum deposit confirmed; awaiting ckUSDC mint');
      // Arbitrary ledger balances are not evidence of mint finality.
      window.dispatchEvent(new Event('focus')); await settle();
      expect(host.textContent).toContain('awaiting ckUSDC mint');
    } else expect(host.textContent).toContain('Ethereum transaction reverted');
    expect((fixture.provider.request as any).mock.calls.some(([args]: any[]) => args.method === 'eth_sendTransaction')).toBe(false);
  });

  it('keeps pending and no-hash deposits held, with no user-clear control', async () => {
    localStorage.setItem(depositKey(), savedDeposit({ hash: '' }));
    render(); await settle();
    window.dispatchEvent(new Event('focus')); await settle();
    expect(host.textContent).toContain('contact Rumi support');
    expect(host.textContent).not.toContain('Clear retry lock');
    expect(host.querySelector<HTMLButtonElement>('.primary')?.disabled).toBe(true);
    expect(localStorage.getItem(depositKey())).toContain('"hash":""');
  });

  it('does not replace a newer deposit journal with a delayed old receipt', async () => {
    const late = deferred<any>();
    localStorage.setItem(depositKey(), savedDeposit());
    const fixture = makeProvider({ eth_getTransactionReceipt: () => late.promise });
    Object.defineProperty(window, 'ethereum', { configurable: true, value: fixture.provider });
    render(); await settle();
    const newer = savedDeposit({ hash: `0x${'ef'.repeat(32)}`, createdAt: 20 });
    localStorage.setItem(depositKey(), newer);
    late.resolve(receipt()); await settle();
    expect(localStorage.getItem(depositKey())).toBe(newer);
    expect(localStorage.getItem(`${depositKey()}:confirmed`)).toBeNull();
  });

  it('invalidates receipt observations after same-owner ICP session replacement', async () => {
    const late = deferred<any>();
    localStorage.setItem(depositKey(), savedDeposit());
    const fixture = makeProvider({ eth_getTransactionReceipt: () => late.promise });
    Object.defineProperty(window, 'ethereum', { configurable: true, value: fixture.provider });
    render(); await settle();
    mocks.generation.set(1);
    late.resolve(receipt()); await settle();
    expect(localStorage.getItem(depositKey())).toBe(savedDeposit());
    expect(localStorage.getItem(`${depositKey()}:confirmed`)).toBeNull();
  });

  it.each(['reset', 'approval'])('does not mistake confirmed %s for a completed deposit', async (phase) => {
    localStorage.setItem(depositKey(), savedDeposit({ phase }));
    const fixture = makeProvider({ eth_getTransactionReceipt: async () => receipt() });
    Object.defineProperty(window, 'ethereum', { configurable: true, value: fixture.provider });
    render(); await settle();
    expect(localStorage.getItem(depositKey())).toBeNull();
    expect(localStorage.getItem(`${depositKey()}:confirmed`)).toBeNull();
    expect(host.textContent).toContain('allowance transaction confirmed');
    expect(host.textContent).not.toContain('awaiting ckUSDC mint');
  });

  it('retains null Ethereum receipts and resumes automatically when the page regains focus', async () => {
    let result: any = null;
    localStorage.setItem(depositKey(), savedDeposit());
    const fixture = makeProvider({ eth_getTransactionReceipt: async () => result });
    Object.defineProperty(window, 'ethereum', { configurable: true, value: fixture.provider });
    render(); await settle();
    expect(localStorage.getItem(depositKey())).toBeTruthy();
    result = receipt(); window.dispatchEvent(new Event('focus')); await settle();
    expect(localStorage.getItem(depositKey())).toBeNull();
    expect(host.textContent).toContain('awaiting ckUSDC mint');
  });

  it('restores awaiting-mint receipt when returning to the token and after remount', async () => {
    localStorage.setItem(`${depositKey()}:confirmed`, savedDeposit());
    render(); await settle();
    expect(host.textContent).toContain('awaiting ckUSDC mint');
    await chooseMintToken(CKLINK.ledgerId, CKLINK.symbol); await settle();
    expect(host.textContent).not.toContain('awaiting ckUSDC mint');
    await chooseMintToken(CKUSDC.ledgerId, CKUSDC.symbol); await settle();
    expect(host.textContent).toContain('awaiting ckUSDC mint');
    unmount(instance as any); instance = undefined; host.replaceChildren();
    render(); await settle(); expect(host.textContent).toContain('awaiting ckUSDC mint');
  });

  it.each([
    [{ Pending: null }, false, 'Ethereum payout pending'],
    [{ TxSent: { transaction_hash: HASH } }, false, 'awaiting finality'],
    [{ TxFinalized: { PendingReimbursement: { transaction_hash: HASH } } }, false, 'reimbursement pending'],
    [{ TxFinalized: { Reimbursed: { transaction_hash: HASH, reimbursed_amount: 1n, reimbursed_in_block: 5n } } }, true, 'reimbursement completed'],
    [{ TxFinalized: { Success: { transaction_hash: HASH, effective_transaction_fee: [] } } }, true, 'payout finalized'],
  ])('tracks accepted withdrawal status case %# with explicit terminal semantics', async (status, settled, message) => {
    localStorage.setItem(withdrawalKey(), savedWithdrawal()); installStatus(status);
    render(); await settle(); clickButton(/^Redeem$/); await settle();
    expect(host.textContent).toContain(message);
    expect(JSON.parse(localStorage.getItem(withdrawalKey())!).settled).toBe(settled);
    expect(mocks.approveAndWithdraw).not.toHaveBeenCalled();
    expect(host.textContent).not.toContain('Clear retry lock');
  });

  it('never resolves unknown withdrawals from empty history or mismatched accepted receipts', async () => {
    localStorage.setItem(withdrawalKey(), savedWithdrawal({ ckEthBurn: undefined, ckTokenBurn: undefined }));
    const status = installStatus({ TxFinalized: { Success: { transaction_hash: HASH } } });
    render(); await settle(); clickButton(/^Redeem$/); await settle();
    expect(status).not.toHaveBeenCalled();
    expect(host.textContent).toContain('no confirmed response');
    const known = savedWithdrawal(); localStorage.setItem(withdrawalKey(), known);
    status.mockResolvedValue([]); window.dispatchEvent(new Event('focus')); await settle();
    expect(localStorage.getItem(withdrawalKey())).toBe(known);
    status.mockResolvedValue([{ withdrawal_id: 3n, from: OWNER, from_subaccount: [], token_symbol: 'ckLINK', withdrawal_amount: 1n, recipient_address: ACCOUNT, status: { TxFinalized: { Success: { transaction_hash: HASH } } } }]);
    window.dispatchEvent(new Event('focus')); await settle(); expect(localStorage.getItem(withdrawalKey())).toBe(known);
  });

  it('estimates fees automatically and ignores a delayed quote from a previous token', async () => {
    const oldQuote = deferred<any>(); mocks.getQuote.mockImplementationOnce(() => oldQuote.promise);
    render(); await settle(); clickButton(/^Redeem$/); await settle();
    setInput('#redeem-amount', '1'); setInput('#redeem-address', ACCOUNT);
    await vi.advanceTimersByTimeAsync(400); await settle();
    expect(mocks.getQuote).toHaveBeenCalledTimes(1);
    await chooseMintToken(CKLINK.ledgerId, CKLINK.symbol); await settle();
    setInput('#redeem-amount', '1'); await vi.advanceTimersByTimeAsync(400); await settle();
    expect(host.textContent).toContain('ckLINK allowance cap');
    oldQuote.resolve({ owner: OWNER, tokenLedgerId: CKUSDC.ledgerId, amount: 1n, ckTokenFee: 1n, ckEthFee: 1n, maxTransactionFee: 999999n, ckTokenAllowance: 2n, ckEthAllowance: 999999n, ckTokenBalance: 10n, ckEthBalance: 10n, quotedAtMs: Date.now() });
    await settle(); expect(host.textContent).toContain('ckLINK allowance cap');
    expect(host.textContent).not.toContain('ckUSDC allowance cap');
    expect(host.textContent).not.toContain('Get current fee quote');
  });
});

describe('ckERC20 exact automatic mint completion', () => {
  const saved = () => savedDeposit({ atomicAmount: '2000000', erc20Address: CKUSDC.erc20Address, eventCursor: '100' });
  const accepted = () => ({ timestamp: 1n, payload: { AcceptedErc20Deposit: { transaction_hash: HASH, log_index: 7n, principal: OWNER, subaccount: [], from_address: ACCOUNT, erc20_contract_address: CKUSDC.erc20Address, value: 2000000n } } });
  const minted = () => ({ timestamp: 2n, payload: { MintedCkErc20: { event_source: { transaction_hash: HASH, log_index: 7n }, erc20_contract_address: CKUSDC.erc20Address, ckerc20_token_symbol: 'ckUSDC', mint_block_index: 55n } } });
  function eventFeed(get_events: ReturnType<typeof vi.fn>) {
    mocks.getMinter.mockResolvedValue({ get_minter_info: vi.fn().mockResolvedValue({ deposit_with_subaccount_helper_contract_address: [HELPER] }), get_events });
  }
  it('resumes a saved cursor and reaches exact mint completion across polls', async () => {
    const key = `${depositKey()}:confirmed`; localStorage.setItem(key, saved());
    const feed = vi.fn().mockResolvedValueOnce({ events: [accepted()], total_event_count: 102n }).mockResolvedValue({ events: [minted()], total_event_count: 102n }); eventFeed(feed);
    render(); await settle();
    expect(JSON.parse(localStorage.getItem(key)!).eventCursor).toBe('101');
    expect(host.textContent).toContain('awaiting ckUSDC mint');
    expect(host.querySelector<HTMLButtonElement>('.primary')?.disabled).toBe(true);
    window.dispatchEvent(new Event('focus')); await settle();
    expect(feed).toHaveBeenLastCalledWith({ start: 101n, length: 1000n });
    expect(host.textContent).toContain('mint confirmed by the minter at ledger block 55');
    expect(host.querySelector<HTMLButtonElement>('.primary')?.disabled).toBe(false);
    expect(mocks.approveAndWithdraw).not.toHaveBeenCalled();
  });
  it('does not overwrite a newer persisted mint cursor with a delayed old batch', async () => {
    const key = `${depositKey()}:confirmed`, late = deferred<any>(); localStorage.setItem(key, saved());
    const feed = vi.fn().mockImplementation(() => late.promise); eventFeed(feed);
    render(); await settle();
    const newer = savedDeposit({ atomicAmount: '2000000', erc20Address: CKUSDC.erc20Address, eventCursor: '101', eventLogIndex: '7' }); localStorage.setItem(key, newer);
    late.resolve({ events: [accepted()], total_event_count: 102n }); await settle();
    expect(localStorage.getItem(key)).toBe(newer);
    feed.mockResolvedValue({ events: [minted()], total_event_count: 102n }); window.dispatchEvent(new Event('focus')); await settle();
    expect(JSON.parse(localStorage.getItem(key)!).mintBlock).toBe('55');
  });
  it('marks legacy history as limited and never infers no mint from an empty page', async () => {
    const key = `${depositKey()}:confirmed`; localStorage.setItem(key, savedDeposit());
    const feed = vi.fn().mockImplementation(({ length }) => Promise.resolve({ events: [], total_event_count: length === 0n ? 10000n : 10000n })); eventFeed(feed);
    render(); await settle();
    expect(feed).toHaveBeenLastCalledWith({ start: 9000n, length: 1000n });
    expect(JSON.parse(localStorage.getItem(key)!).eventCursor).toBe('9000');
    expect(host.textContent).toContain('Only recent minter history is available');
    expect(host.textContent).toContain('awaiting ckUSDC mint');
  });
});

describe('ckERC20 reverted Ethereum submission finality', () => {
  it('holds a pre-final revert until its block becomes finalized and canonical', async () => {
    let finalized = '0x0f', canonicalHash = `0x${'cd'.repeat(32)}`;
    localStorage.setItem(depositKey(), savedDeposit());
    const fixture = makeProvider({ eth_getTransactionReceipt: async () => receipt('0x0'), eth_getBlockByNumber: async (params) => params?.[0] === 'finalized' ? { number: finalized } : { number: '0x10', hash: canonicalHash } });
    Object.defineProperty(window, 'ethereum', { configurable: true, value: fixture.provider });
    render(); await settle();
    expect(localStorage.getItem(depositKey())).toBeTruthy(); expect(host.textContent).toContain('waiting for Ethereum finality');
    finalized = '0x20'; canonicalHash = `0x${'ee'.repeat(32)}`; window.dispatchEvent(new Event('focus')); await settle();
    expect(localStorage.getItem(depositKey())).toBeTruthy();
    canonicalHash = `0x${'cd'.repeat(32)}`; window.dispatchEvent(new Event('focus')); await settle();
    expect(localStorage.getItem(depositKey())).toBeNull(); expect(host.textContent).toContain('Ethereum transaction reverted');
  });
  it('keeps a reverted hash held when the provider cannot read finalized blocks', async () => {
    localStorage.setItem(depositKey(), savedDeposit());
    const fixture = makeProvider({ eth_getTransactionReceipt: async () => receipt('0x0'), eth_getBlockByNumber: async () => { throw new Error('unsupported'); } });
    Object.defineProperty(window, 'ethereum', { configurable: true, value: fixture.provider });
    render(); await settle(); expect(localStorage.getItem(depositKey())).toBeTruthy();
    expect(host.textContent).toContain('waiting for Ethereum finality');
    expect(host.querySelector<HTMLButtonElement>('.primary')?.disabled).toBe(true);
  });
});

describe('ckERC20 preserves unreadable and ambiguous financial journals', () => {
  it.each([
    ['deposit', () => depositKey(), '{bad json'],
    ['deposit', () => depositKey(), '{}'],
    ['deposit', () => `rumi:ckusdc:pending-deposit:${ACCOUNT}:${OWNER.toText()}`, '{bad json'],
    ['deposit', () => `rumi:ckusdc:pending-deposit:${ACCOUNT}:${OWNER.toText()}`, '[]'],
    ['deposit', () => `${depositKey()}:confirmed`, '{bad json'],
    ['deposit', () => `${depositKey()}:confirmed`, '{}'],
    ['deposit', () => `${depositKey()}:confirmed`, savedDeposit({ mintBlock: '55' })],
    ['deposit', () => depositKey(), savedDeposit({ tokenLedgerId: CKLINK.ledgerId })],
    ['withdrawal', () => withdrawalKey(), '{bad json'],
    ['withdrawal', () => withdrawalKey(), '{}'],
    ['withdrawal', () => withdrawalKey(), savedWithdrawal({ settled: true })],
    ['withdrawal', () => withdrawalKey(), savedWithdrawal({ tokenLedgerId: 'invalid' })],
    ['withdrawal', () => `rumi:ckusdc:pending-withdrawal:${OWNER.toText()}`, '{bad json'],
    ['withdrawal', () => `rumi:ckusdc:pending-withdrawal:${OWNER.toText()}`, 'null'],
  ] as const)('holds malformed journal case %# without dispatch or overwrite', async (direction, key, raw) => {
    localStorage.setItem(key(), raw); render(); await settle();
    if (direction === 'withdrawal') { clickButton(/^Redeem$/); await settle(); setInput('#redeem-amount', '1'); setInput('#redeem-address', ACCOUNT); await vi.advanceTimersByTimeAsync(400); await settle(); }
    else setInput('#deposit-amount', '2');
    expect(host.querySelector<HTMLButtonElement>('.primary')?.disabled).toBe(true);
    expect(host.textContent).toContain('tracking could not be read safely');
    host.querySelector<HTMLButtonElement>('.primary')!.click(); window.dispatchEvent(new Event('focus')); await settle();
    expect(localStorage.getItem(key())).toBe(raw);
    expect(mocks.approveAndWithdraw).not.toHaveBeenCalled();
    expect((provider.request as any).mock.calls.some(([args]: any[]) => args.method === 'eth_sendTransaction')).toBe(false);
  });

  it.each([
    { Err: { CkEthLedgerError: { error: { TemporarilyUnavailable: 'burn reply lost' } } } },
    { Err: { TemporarilyUnavailable: 'unknown disposition' } },
    { Err: { NewUnknownError: null } },
  ])('keeps the exact pre-dispatch hold after ambiguous minter error %#', async (response) => {
    let submittedRaw: string | null = null;
    mocks.approveAndWithdraw.mockImplementation(async (params) => {
      params.onWithdrawalSubmitted(); submittedRaw = localStorage.getItem(withdrawalKey());
      params.onWithdrawalResolved(response); throw new Error('Withdrawal response needs reconciliation.');
    });
    render(); await settle(); clickButton(/^Redeem$/); await settle();
    setInput('#redeem-amount', '1'); setInput('#redeem-address', ACCOUNT); await vi.advanceTimersByTimeAsync(400); await settle();
    clickButton(/Approve and request USDC redemption/); await settle();
    expect(submittedRaw).toBeTruthy(); expect(localStorage.getItem(withdrawalKey())).toBe(submittedRaw);
    expect(host.querySelector<HTMLButtonElement>('.primary')?.disabled).toBe(true);
    host.querySelector<HTMLButtonElement>('.primary')!.click(); await settle(); expect(mocks.approveAndWithdraw).toHaveBeenCalledTimes(1);
  });

  it('preserves accepted receipt and releases page busy state if popup cleanup rejects', async () => {
    const close = vi.fn().mockRejectedValue(new Error('popup already gone'));
    mocks.isOisy.mockReturnValue(true); mocks.beginAction.mockReturnValue({ ready: Promise.resolve(), close });
    mocks.approveAndWithdraw.mockImplementation(async (params) => {
      params.onWithdrawalSubmitted(); params.onWithdrawalResolved({ Ok: { cketh_block_index: 3n, ckerc20_block_index: 4n } });
      return { ckEthBurnBlock: 3n, ckTokenBurnBlock: 4n };
    });
    render(); await settle(); clickButton(/^Redeem$/); await settle();
    setInput('#redeem-amount', '1'); setInput('#redeem-address', ACCOUNT); await vi.advanceTimersByTimeAsync(400); await settle();
    clickButton(/Approve and request USDC redemption/); await settle();
    expect(close).toHaveBeenCalledOnce(); expect(JSON.parse(localStorage.getItem(withdrawalKey())!).ckEthBurn).toBe('3');
    expect(host.querySelector<HTMLInputElement>('#redeem-address')?.disabled).toBe(false);
    expect(host.textContent).toContain('Withdrawal accepted; Ethereum payout pending');
    expect(mocks.approveAndWithdraw).toHaveBeenCalledOnce();
  });
});

it('does not show old mint completion after its journal was replaced during the query', async () => {
  const key = `${depositKey()}:confirmed`, late = deferred<any>();
  localStorage.setItem(key, savedDeposit({ atomicAmount: '2000000', erc20Address: CKUSDC.erc20Address, eventCursor: '100', eventLogIndex: '7' }));
  mocks.getMinter.mockResolvedValue({ get_minter_info: vi.fn().mockResolvedValue({ deposit_with_subaccount_helper_contract_address: [HELPER] }), get_events: vi.fn(() => late.promise) });
  render(); await settle();
  const newer = savedDeposit({ hash: `0x${'ef'.repeat(32)}`, atomicAmount: '2000000', erc20Address: CKUSDC.erc20Address, eventCursor: '101', createdAt: 20 }); localStorage.setItem(key, newer);
  late.resolve({ events: [{ timestamp: 1n, payload: { MintedCkErc20: { event_source: { transaction_hash: HASH, log_index: 7n }, erc20_contract_address: CKUSDC.erc20Address, ckerc20_token_symbol: 'ckUSDC', mint_block_index: 55n } } }], total_event_count: 102n }); await settle();
  expect(localStorage.getItem(key)).toBe(newer); expect(host.textContent).not.toContain('mint confirmed by the minter');
});
