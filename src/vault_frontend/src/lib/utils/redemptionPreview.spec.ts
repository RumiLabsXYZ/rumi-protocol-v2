import { Principal } from '@dfinity/principal';
import { describe, expect, it } from 'vitest';
import {
	quoteIsFresh,
	quoteMatchesAmount,
	queueCandidatePricesFresh,
	maxRedeemableInput,
	redemptionPreflightIsFresh,
	toHumanIcusd,
	toHumanRawAmount,
	toQueueEntryViews,
	type RedemptionQueue,
	type RedemptionQueueEntry,
	type RedemptionQuote,
	type RedemptionPreflight,
} from './redemptionPreview';

const icp = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
const xaut = Principal.fromText('nza5v-qaaaa-aaaar-qahzq-cai');
const snapshotNs = 1_000_000_000_000n;
const preflight: RedemptionPreflight = {
	principalText: '2vxsx-fae',
	walletType: null,
	sessionGeneration: 0,
	ledgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
	observedAtMs: 10_000,
	allowanceRaw: 0n,
	balanceRaw: 1_000n,
	feeRaw: 10n,
};

function entry(runIndex: number, symbol: string, principal = icp): RedemptionQueueEntry {
	return {
		run_index: runIndex,
		collateral_type: principal,
		symbol,
		decimals: symbol === 'ckXAUT' ? 6 : 8,
		price_usd: symbol === 'ICP' ? 6 : 2_000,
		price_timestamp_ns: snapshotNs,
		price_fresh: true,
		min_cr: symbol === 'ICP' ? 1.5 : 1.18,
		liquidation_cr: symbol === 'ICP' ? 1.33 : 1.12,
		weakest_vault_cr: 1.5,
		health_headroom: 0.24,
		vault_count: 2n,
		eligible_collateral_raw: 7_000_000_000n,
		eligible_debt_e8s: 100_000_000_000n,
		max_input_icusd_e8s: 100_000_000_000n,
		max_net_collateral_raw: 1_200_000_000n,
	};
}

function quote(runIndex: number, amount = 50_000_000n): RedemptionQuote {
	return {
		run_index: runIndex,
		amount_e8s: amount,
		ranking_fresh: true,
		collateral_type: icp,
		symbol: 'ICP',
		decimals: 8,
		price_usd: 6,
		price_timestamp_ns: snapshotNs,
		price_fresh: true,
		fee_e8s: 150_000n,
		rmr: 0.97,
		effective_icusd_e8s: 48_350_000n,
		gross_collateral_raw: 805_833_333n,
		ledger_fee_raw: 10_000n,
		net_collateral_raw: 805_823_333n,
		max_input_icusd_e8s: 100_000_000n,
		quoted_at_ns: snapshotNs,
		quote_validity_window_ns: 60_000_000_000n,
	};
}

describe('redemption preview presentation helpers', () => {
	it('formats raw token units and icUSD without floating-point rounding', () => {
		expect(toHumanRawAmount(36_467_800_000n, 9)).toBe('36.4678');
		expect(toHumanRawAmount(1_234_000n, 6)).toBe('1.234');
		expect(toHumanIcusd(119_999_000_000n)).toBe('1199.99');
	});

	it('preserves consecutive same-token runs without merging a later repeated token', () => {
		const rows = toQueueEntryViews([entry(0, 'ICP'), entry(1, 'ckXAUT', xaut), entry(2, 'ICP')]);
		expect(rows.map((row) => `${row.runIndex}:${row.symbol}`)).toEqual(['0:ICP', '1:ckXAUT', '2:ICP']);
		expect(rows[0].lockedCollateralRaw).toBe(7_000_000_000n);
		expect(rows[0].maxNetCollateralRaw).toBe(1_200_000_000n);
		expect(rows[0].lockedCollateralRaw).not.toBe(rows[0].maxNetCollateralRaw);
		expect(rows[0].minCr).toBe(1.5);
		expect(rows[0].liquidationCr).toBe(1.33);
	});

	it('checks queue and price freshness before allowing a quote to be used', () => {
		const quoteValue = quote(0);
		const queue: RedemptionQueue = {
			observed_at_ns: snapshotNs,
			ranking_fresh: true,
			rmr: 0.97,
			price_freshness_window_ns: 600_000_000_000n,
			entries: [entry(0, 'ICP')],
		};
		expect(quoteIsFresh(quoteValue, queue, snapshotNs + 30_000_000_000n)).toBe(true);
		expect(quoteIsFresh(quoteValue, queue, snapshotNs + 61_000_000_000n)).toBe(false);
		expect(quoteIsFresh(quoteValue, { ...queue, entries: [{ ...entry(0, 'ICP'), price_fresh: false }] }, snapshotNs + 1n)).toBe(false);
		expect(quoteIsFresh(quoteValue, { ...queue, ranking_fresh: false }, snapshotNs + 1n)).toBe(false);
		expect(quoteIsFresh(quoteValue, { ...queue, entries: [entry(0, 'ICP'), { ...entry(1, 'ckXAUT', xaut), price_fresh: false }] }, snapshotNs + 1n)).toBe(false);
		expect(queueCandidatePricesFresh(queue, snapshotNs + 599_000_000_000n)).toBe(true);
		expect(queueCandidatePricesFresh(queue, snapshotNs + 601_000_000_000n)).toBe(false);
		expect(quoteIsFresh({ ...quoteValue, ranking_fresh: false }, queue, snapshotNs + 1n)).toBe(false);
		expect(quoteIsFresh(quoteValue, null, snapshotNs + 1n)).toBe(false);
	});

	it('requires the quote amount to exactly match the current request', () => {
		expect(quoteMatchesAmount(quote(0), 50_000_000n)).toBe(true);
		expect(quoteMatchesAmount(quote(0), 50_000_001n)).toBe(false);
		expect(quoteMatchesAmount(null, 50_000_000n)).toBe(false);
	});

	it('uses one live fee when allowance covers the amount and two when approval is required', () => {
		expect(maxRedeemableInput(null, preflight)).toBe(980n);
		expect(maxRedeemableInput(950n, preflight)).toBe(950n);
		expect(maxRedeemableInput(null, { ...preflight, allowanceRaw: 1_000n })).toBe(990n);
		expect(maxRedeemableInput(500n, { ...preflight, allowanceRaw: 1_000n })).toBe(500n);
	});

	it('binds preflight snapshots to the current wallet and 30-second freshness window', () => {
		expect(redemptionPreflightIsFresh(preflight, '2vxsx-fae', preflight.ledgerId, null, 0, 40_000)).toBe(true);
		expect(redemptionPreflightIsFresh(preflight, 'w7x7r-cok77-xa', preflight.ledgerId, null, 0, 10_001)).toBe(false);
		expect(redemptionPreflightIsFresh(preflight, preflight.principalText, 'another-ledger', null, 0, 10_001)).toBe(false);
		expect(redemptionPreflightIsFresh(preflight, preflight.principalText, preflight.ledgerId, 'plug', 0, 10_001)).toBe(false);
		expect(redemptionPreflightIsFresh(preflight, preflight.principalText, preflight.ledgerId, null, 1, 10_001)).toBe(false);
		expect(redemptionPreflightIsFresh(preflight, preflight.principalText, preflight.ledgerId, null, 0, 40_001)).toBe(false);
	});
});
