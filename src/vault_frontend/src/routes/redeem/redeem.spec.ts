import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';
import { Principal } from '@dfinity/principal';
import { CONFIG } from '$lib/config';
import type { RedemptionQueue, RedemptionQueueEntry, RedemptionQuote } from '$lib/utils/redemptionPreview';
import { getVaultCrTextColor } from '$lib/utils/vaultHealth';

const mocks = vi.hoisted(() => {
	let state: any = {};
	const subscribers = new Set<(value: any) => void>();
	const readable = (initial: any) => {
		let value = initial;
		const listeners = new Set<(next: any) => void>();
		return {
			subscribe(fn: (next: any) => void) { fn(value); listeners.add(fn); return () => listeners.delete(fn); },
			set(next: any) { value = next; for (const fn of listeners) fn(value); },
			get: () => value,
		};
	};
	const walletStore = {
		subscribe(fn: (value: any) => void) {
			fn(state);
			subscribers.add(fn);
			return () => subscribers.delete(fn);
		},
		setState(next: any) {
			state = next;
			for (const fn of subscribers) fn(state);
		},
		getState: () => state,
		refreshBalance: vi.fn().mockResolvedValue(undefined),
	};
	return {
		walletStore,
		currentWalletType: readable(null),
		walletSessionGeneration: readable(0),
    getRedemptionPreview: vi.fn(),
    prepareRedemptionOffer: vi.fn(),
		getRedemptionPreflight: vi.fn(),
		redeemQuoted: vi.fn(),
		resolveRoute: vi.fn().mockResolvedValue({ estimatedOutput: 0n }),
		formatProtocolError: vi.fn((error: unknown) => String(error)),
	};
});

vi.mock('$lib/stores/wallet', () => ({ walletStore: mocks.walletStore }));
vi.mock('$lib/services/auth', () => ({
	currentWalletType: mocks.currentWalletType,
	walletSessionGeneration: mocks.walletSessionGeneration,
}));
vi.mock('$lib/services/protocol', () => ({
    protocolService: {
      getRedemptionPreview: mocks.getRedemptionPreview,
      prepareRedemptionOffer: mocks.prepareRedemptionOffer,
		getRedemptionPreflight: mocks.getRedemptionPreflight,
		redeemQuoted: mocks.redeemQuoted,
	},
}));
vi.mock('$lib/services/protocol/apiClient', () => ({ ApiClient: { formatProtocolError: mocks.formatProtocolError } }));
vi.mock('$lib/components/dashboard/ProtocolStats.svelte', async () => ({
	default: (await vi.importActual<typeof import('./ProtocolStatsStub.svelte')>('./ProtocolStatsStub.svelte')).default,
}));
vi.mock('$lib/services/swapRouter', () => ({ resolveRoute: mocks.resolveRoute }));
vi.mock('$lib/services/ammService', () => ({
	AMM_TOKENS: ['icUSD', 'ckUSDT', 'ckUSDC', 'ICP'].map((symbol) => ({ symbol })),
}));

import Page from './+page.svelte';

const ICP = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
const XAUT = Principal.fromText('nza5v-qaaaa-aaaar-qahzq-cai');

function makeEntry(run: number, symbol: string, collateral: Principal, nowNs: bigint): RedemptionQueueEntry {
	return {
		run_index: run,
		collateral_type: collateral,
		symbol,
		decimals: symbol === 'ckXAUT' ? 6 : 8,
		price_usd: symbol === 'ICP' ? 6 : 2_000,
		price_timestamp_ns: nowNs,
		price_fresh: true,
		min_cr: symbol === 'ICP' ? 1.5 : 1.18,
		liquidation_cr: symbol === 'ICP' ? 1.33 : 1.12,
		weakest_vault_cr: symbol === 'ICP' ? 1.5 : 1.45,
		health_headroom: 0.25,
		vault_count: 2n,
		eligible_collateral_raw: 7_000_000_000n,
		eligible_debt_e8s: 100_000_000_000n,
		max_input_icusd_e8s: 100_000_000_000n,
		max_net_collateral_raw: 1_200_000_000n,
	};
}

function makeQueue(nowNs = BigInt(Date.now()) * 1_000_000n, rankingFresh = true): RedemptionQueue {
	return {
		observed_at_ns: nowNs,
		ranking_fresh: rankingFresh,
		rmr: 0.97,
		price_freshness_window_ns: 600_000_000_000n,
		entries: [makeEntry(0, 'ICP', ICP, nowNs), makeEntry(1, 'ckXAUT', XAUT, nowNs), makeEntry(2, 'ICP', ICP, nowNs)],
	};
}

function makeQuote(amount: bigint, nowNs = BigInt(Date.now()) * 1_000_000n, symbol = 'ICP', collateral = ICP): RedemptionQuote {
	return {
		run_index: 0,
		amount_e8s: amount,
		ranking_fresh: true,
		collateral_type: collateral,
		symbol,
		decimals: symbol === 'ckXAUT' ? 6 : 8,
		price_usd: symbol === 'ckXAUT' ? 2_000 : 6,
		price_timestamp_ns: nowNs,
		price_fresh: true,
		fee_e8s: amount / 100n,
		rmr: 0.97,
		effective_icusd_e8s: amount * 97n / 100n,
		gross_collateral_raw: 805_833_333n,
		ledger_fee_raw: 10_000n,
		net_collateral_raw: 805_823_333n,
		max_input_icusd_e8s: 100_000_000_000n,
		quoted_at_ns: nowNs,
		quote_validity_window_ns: 60_000_000_000n,
	};
}

function makePreview(amount: bigint, queue = makeQueue(nowNs), estimate: { Ok: RedemptionQuote } | { Err: unknown } = { Ok: makeQuote(amount, nowNs, queue.entries[0]?.symbol, queue.entries[0]?.collateral_type) }) {
  return { queue, estimate };
}

function makePreparedOffer(amount: bigint, queue = makeQueue(nowNs), quote: { Ok: RedemptionQuote } | { Err: unknown } = { Ok: makeQuote(amount, nowNs, queue.entries[0]?.symbol, queue.entries[0]?.collateral_type) }) {
  return { Ok: { queue, quote } };
}

function makePreflight(principalText = '2vxsx-fae') {
	return {
		principalText,
		walletType: null,
		sessionGeneration: 0,
		ledgerId: CONFIG.currentIcusdLedgerId,
		observedAtMs: Date.now(),
		allowanceRaw: 0n,
		balanceRaw: 10_000_000_000n,
		feeRaw: 100_000n,
	};
}

function connectedWallet(principalText = '2vxsx-fae') {
	return {
		isConnected: true,
		principal: { toText: () => principalText, toString: () => principalText },
		tokenBalances: { ICUSD: { formatted: '100', raw: 10_000_000_000n } },
	};
}

async function settle(rounds = 10) {
	for (let i = 0; i < rounds; i++) {
		await Promise.resolve();
		await tick();
		flushSync();
	}
}

function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((done) => { resolve = done; });
	return { promise, resolve };
}

let host: HTMLDivElement;
let instance: unknown;
let nowNs: bigint;

function render() {
	instance = mount(Page, { target: host });
	flushSync();
}

async function setAmount(amount: string) {
	const input = host.querySelector<HTMLInputElement>('#icusd-amount')!;
	input.value = amount;
	input.dispatchEvent(new Event('input', { bubbles: true }));
	flushSync();
	await vi.advanceTimersByTimeAsync(350);
	await settle();
}

async function checkLiveOffer() {
  const button = host.querySelector<HTMLButtonElement>('#check-live-offer')!;
  button.click();
  await settle();
}

beforeEach(() => {
	vi.useFakeTimers();
	nowNs = BigInt(Date.now()) * 1_000_000n;
	host = document.createElement('div');
	document.body.appendChild(host);
	mocks.walletStore.setState(connectedWallet());
	mocks.currentWalletType.set(null);
	mocks.walletSessionGeneration.set(0);
	mocks.walletStore.refreshBalance.mockReset().mockResolvedValue(undefined);
  mocks.getRedemptionPreview.mockReset().mockImplementation(async (amount: bigint) => makePreview(amount));
  mocks.prepareRedemptionOffer.mockReset().mockImplementation(async (amount: bigint) => makePreparedOffer(amount));
	mocks.getRedemptionPreflight.mockReset().mockImplementation(async () => makePreflight(mocks.walletStore.getState().principal.toText()));
	mocks.redeemQuoted.mockReset().mockResolvedValue({
		success: true,
		blockIndex: 321,
		redemption: { collateralType: ICP.toText(), symbol: 'ICP', decimals: 8, netCollateralRaw: 805_823_333n, payoutStatus: { Queued: null } },
	});
	flushSync();
});

afterEach(() => {
	if (instance) unmount(instance as any);
	host.remove();
	vi.useRealTimers();
});

describe('redemption route quote and queue safety', () => {
	it('renders consecutive repeated-token runs below How Redemption Works with per-asset CR tint thresholds', async () => {
		render();
		await settle();
		const how = host.querySelector('.how-it-works')!;
		const queue = host.querySelector('.redemption-queue')!;
		expect(how.compareDocumentPosition(queue) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
		expect(queue.querySelectorAll('.queue-entry')).toHaveLength(3);
		expect(Array.from(queue.querySelectorAll('.queue-token strong')).map((node) => node.textContent)).toEqual(['ICP', 'ckXAUT', 'ICP']);
		expect(queue.textContent).toContain('later run');
		const health = queue.querySelectorAll<HTMLElement>('.queue-health span[style]');
		const expectedIcp = document.createElement('span');
		expectedIcp.style.color = getVaultCrTextColor(1.5, 1.5, 1.33);
		const expectedXaut = document.createElement('span');
		expectedXaut.style.color = getVaultCrTextColor(1.45, 1.18, 1.12);
		expect(health[1].style.color).toBe(expectedIcp.style.color);
		expect(health[3].style.color).toBe(expectedXaut.style.color);
		expect(health[1].style.color).not.toBe(health[3].style.color);
	});

	it('keeps cached estimates advisory and Checking a live offer never submits', async () => {
		render();
		await settle();
		await setAmount('1');
		expect(host.textContent).toContain('Indicative estimate');
		expect(host.textContent).toContain('not an accepted offer');
		await checkLiveOffer();
		expect(mocks.prepareRedemptionOffer).toHaveBeenCalledWith(100_000_000n);
		expect(mocks.redeemQuoted).not.toHaveBeenCalled();
		expect(host.textContent).toContain('Live offer · not yet accepted');
		expect(host.textContent).toContain('Accept and redeem');
	});

	it('labels cached estimates with source-price age, not quote calculation time', async () => {
		const stalePriceTimestamp = nowNs - 15n * 60n * 1_000_000_000n;
		const queue = makeQueue(nowNs);
		queue.ranking_fresh = false;
		queue.entries[0] = { ...queue.entries[0], price_timestamp_ns: stalePriceTimestamp, price_fresh: false };
		const quote = makeQuote(100_000_000n, nowNs, queue.entries[0].symbol, queue.entries[0].collateral_type);
		quote.price_timestamp_ns = stalePriceTimestamp;
		quote.price_fresh = false;
		mocks.getRedemptionPreview.mockResolvedValue({ queue, estimate: { Ok: quote } });

		render();
		await settle();
		await setAmount('1');

		expect(host.textContent).toContain('Price data from 15m ago');
		expect(host.textContent).not.toContain('Prices from 1s ago');
		expect(host.textContent).toContain('not an accepted offer');
	});

	it('ignores a slower cached preview after the amount changes', async () => {
		const oldPreview = deferred<ReturnType<typeof makePreview>>();
		mocks.getRedemptionPreview.mockImplementation(async (amount: bigint) => amount === 1_000_000_000n ? oldPreview.promise : makePreview(amount));
		render();
		await settle();
		await setAmount('10');
		await setAmount('20');
		expect(host.textContent).toContain('19.4 icUSD value');
		oldPreview.resolve(makePreview(1_000_000_000n));
		await settle();
		expect(host.textContent).toContain('19.4 icUSD value');
		expect(host.textContent).not.toContain('9.7 icUSD value');
	});

	it('invalidates the prepared offer when the wallet principal changes', async () => {
		render();
		await settle();
		await setAmount('10');
		await checkLiveOffer();
		expect(host.textContent).toContain('Accept and redeem');
		mocks.walletStore.setState(connectedWallet('w7x7r-cok77-xa'));
		flushSync();
		expect(host.textContent).not.toContain('Accept and redeem');
		expect(host.textContent).toContain('Wallet changed. Check a live offer again');
		expect(mocks.redeemQuoted).not.toHaveBeenCalled();
	});

	it('invalidates wallet checks after a same-principal reconnect generation change', async () => {
		render();
		await settle();
		await setAmount('10');
		await checkLiveOffer();
		expect(host.textContent).toContain('Accept and redeem');
		mocks.walletSessionGeneration.set(1);
		flushSync();
		expect(host.textContent).not.toContain('Accept and redeem');
		expect(host.textContent).toContain('Wallet checks expired. Refresh them before redeeming.');
		expect(mocks.redeemQuoted).not.toHaveBeenCalled();
	});

	it('declining retains the fresh queue snapshot and never calls redemption', async () => {
		const refreshedQueue = { ...makeQueue(nowNs), entries: [makeEntry(0, 'ckXAUT', XAUT, nowNs)] };
		mocks.prepareRedemptionOffer.mockResolvedValue(makePreparedOffer(1_000_000_000n, refreshedQueue));
		render();
		await settle();
		await setAmount('10');
		await checkLiveOffer();
		expect(Array.from(host.querySelectorAll('.queue-token strong')).map((node) => node.textContent)).toEqual(['ckXAUT']);
		host.querySelector<HTMLButtonElement>('.decline-offer')!.click();
		flushSync();
		expect(host.textContent).toContain('Offer declined. This snapshot remains visible for reference');
		expect(host.textContent).toContain('Check a new live offer');
		expect(mocks.redeemQuoted).not.toHaveBeenCalled();
	});

	it('amount edits invalidate a prepared offer before any acceptance', async () => {
		render();
		await settle();
		await setAmount('10');
		await checkLiveOffer();
		expect(host.textContent).toContain('Live offer · not yet accepted');
		await setAmount('11');
		expect(host.textContent).not.toContain('Accept and redeem');
		expect(host.textContent).toContain('Indicative estimate');
		expect(mocks.redeemQuoted).not.toHaveBeenCalled();
	});

	it('discards a late live-offer response after the amount changes', async () => {
		const pending = deferred<ReturnType<typeof makePreparedOffer>>();
		mocks.prepareRedemptionOffer.mockImplementationOnce(async () => pending.promise);
		render();
		await settle();
		await setAmount('10');
		host.querySelector<HTMLButtonElement>('#check-live-offer')!.click();
		await settle();
		await setAmount('11');
		pending.resolve(makePreparedOffer(1_000_000_000n));
		await settle();
		expect(host.textContent).toContain('Indicative estimate');
		expect(host.textContent).not.toContain('Live offer · not yet accepted');
		expect(host.textContent).not.toContain('Accept and redeem');
		expect(host.querySelector<HTMLButtonElement>('#check-live-offer')?.disabled).toBe(false);
		expect(mocks.redeemQuoted).not.toHaveBeenCalled();
	});

	it('keeps the fresh queue and capacity error when the prepared inner quote is an error', async () => {
		const refreshedQueue = { ...makeQueue(nowNs), entries: [makeEntry(0, 'ckXAUT', XAUT, nowNs)] };
		mocks.prepareRedemptionOffer.mockResolvedValue(makePreparedOffer(1_000_000n, refreshedQueue, { Err: { RedemptionCapacityExceeded: { max_input_icusd_e8s: 250_000_000n } } }));
		render();
		await settle();
		await setAmount('0.01');
		await checkLiveOffer();
		expect(Array.from(host.querySelectorAll('.queue-token strong')).map((node) => node.textContent)).toEqual(['ckXAUT']);
		expect(host.textContent).toContain('2.5 icUSD max');
		expect(host.textContent).not.toContain('Accept and redeem');
		expect(mocks.redeemQuoted).not.toHaveBeenCalled();
	});

	it('keeps the estimate when live preparation is rate limited', async () => {
		mocks.prepareRedemptionOffer.mockResolvedValue({ Err: { RefreshCooldown: { retry_after_ns: 3_000_000_000n } } });
		render();
		await settle();
		await setAmount('10');
		await checkLiveOffer();
		expect(host.textContent).toContain('Indicative estimate');
		expect(host.textContent).toContain('Try again in about 3 seconds');
		expect(mocks.redeemQuoted).not.toHaveBeenCalled();
	});

	it('expires a prepared offer and requires a new live check before acceptance', async () => {
		render();
		await settle();
		await setAmount('10');
		await checkLiveOffer();
		await vi.advanceTimersByTimeAsync(61_000);
		await settle();
		expect(host.textContent).toContain('Expired live offer');
		expect(host.textContent).toContain('Check live offer');
		expect(mocks.redeemQuoted).not.toHaveBeenCalled();
	});

	it('binds the exact quoted token, amount and minimum payout; typed Queued is not shown as delivered', async () => {
		render();
		await settle();
		await setAmount('10');
		await checkLiveOffer();
		host.querySelector<HTMLButtonElement>('.submit-btn')!.click();
		await settle();
		expect(mocks.redeemQuoted.mock.calls[0][0]).toEqual({
			amount_e8s: 1_000_000_000n,
			expected_collateral_type: ICP,
			min_net_collateral_raw: 805_823_333n,
		});
		expect(mocks.redeemQuoted.mock.calls[0][1]).toMatchObject({
			principalText: '2vxsx-fae',
			ledgerId: CONFIG.currentIcusdLedgerId,
			allowanceRaw: 0n,
			balanceRaw: 10_000_000_000n,
			feeRaw: 100_000n,
		});
		expect(mocks.redeemQuoted.mock.calls[0][2]).toMatchObject({
			amountE8s: 1_000_000_000n,
			collateralTypeText: ICP.toText(),
			minimumNetCollateralRaw: 805_823_333n,
			validUntilNs: nowNs + 60_000_000_000n,
		});
		expect(host.textContent).toContain('Queued: 8.05823333 ICP queued for delivery. The payout has not been credited yet.');
	});

	it('keeps a reply-lost submission ambiguous and requires deliberate refresh before retry', async () => {
		mocks.redeemQuoted.mockResolvedValue({ success: false, ambiguous: true, ambiguityStage: 'submission', error: 'Payout result is unknown.' });
		render();
		await settle();
		await setAmount('10');
		await checkLiveOffer();
		host.querySelector<HTMLButtonElement>('.submit-btn')!.click();
		await settle();
		expect(host.textContent).toContain('Payout result is unknown.');
		expect(host.textContent).not.toContain('queued for delivery');
		expect(host.querySelector('.submit-btn')!.hasAttribute('disabled')).toBe(true);
		expect(host.textContent).toContain('Refresh balances and quote');
	});

	it('keeps a typed queued receipt tied to the previous wallet session after reconnect', async () => {
		mocks.redeemQuoted.mockResolvedValue({
			success: true,
			blockIndex: 322,
			redemption: { collateralType: ICP.toText(), symbol: 'ICP', decimals: 8, netCollateralRaw: 805_823_333n, payoutStatus: { Queued: null } },
			sessionChangedAfterSubmission: true,
			message: 'Check the previous wallet session queue.',
		});
		render();
		await settle();
		await setAmount('10');
		await checkLiveOffer();
		host.querySelector<HTMLButtonElement>('.submit-btn')!.click();
		await settle();
		expect(host.textContent).toContain('Queued: 8.05823333 ICP queued for delivery. The payout has not been credited yet.');
		expect(host.textContent).toContain('Check the previous wallet session queue.');
		expect(host.querySelector<HTMLInputElement>('#icusd-amount')!.value).toBe('10');
		expect(mocks.walletStore.refreshBalance).not.toHaveBeenCalled();
	});

	it('does not describe a lost approval reply as a submitted redemption', async () => {
		mocks.redeemQuoted.mockResolvedValue({ success: false, ambiguous: true, ambiguityStage: 'approval', error: 'Approval response lost.' });
		render();
		await settle();
		await setAmount('10');
		await checkLiveOffer();
		host.querySelector<HTMLButtonElement>('.submit-btn')!.click();
		await settle();
		expect(host.textContent).toContain('The approval response was lost. The redemption call was not sent.');
		expect(host.textContent).not.toContain('The payout is unconfirmed.');
		expect(host.querySelector('.submit-btn')!.hasAttribute('disabled')).toBe(true);
	});

	it('unwraps a legacy ProtocolError carried by the redemption-specific error variant', async () => {
		const protocolError = { TemporarilyUnavailable: 'protocol is in read-only mode' };
		mocks.formatProtocolError.mockClear();
		mocks.getRedemptionPreview.mockResolvedValue(makePreview(1_000_000_000n, makeQueue(nowNs), { Err: { Protocol: protocolError } }));
		render();
		await settle();
		await setAmount('10');
		expect(mocks.formatProtocolError).toHaveBeenCalledWith(protocolError);
		expect(host.textContent).toContain('[object Object]');
	});

	it('fails closed when the backend marks the global ranking incomplete', async () => {
		mocks.getRedemptionPreview.mockImplementation(async (amount: bigint) => makePreview(amount, makeQueue(nowNs, false)));
		render();
		await settle();
		await setAmount('10');
		mocks.prepareRedemptionOffer.mockResolvedValue({ Err: { RefreshUnavailable: { message: 'Ranking remains incomplete', retry_after_ns: 0n } } });
		await checkLiveOffer();
		expect(host.textContent).toContain('Indicative estimate');
		expect(host.textContent).toContain('Ranking remains incomplete');
		expect(host.textContent).not.toContain('Accept and redeem');
		expect(mocks.redeemQuoted).not.toHaveBeenCalled();
	});
});
