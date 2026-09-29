import { describe, expect, it } from 'vitest';
import { IDL } from '@dfinity/candid';
import { Principal } from '@dfinity/principal';
import { idlFactory } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';

/** Read method types from the generated Candid service, never a local schema copy. */
function generatedMethod(name: string): any {
	const service: any = idlFactory({ IDL });
	const entry = service._fields.find(([methodName]: [string, unknown]) => methodName === name);
	if (!entry) throw new Error(`${name} is missing from the generated rumi_protocol_backend Candid service`);
	return entry[1];
}

function candidRoundTrip(types: any[], values: unknown[]): unknown[] {
	return IDL.decode(types, IDL.encode(types, values));
}

const ICP = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
const XAUT = Principal.fromText('nza5v-qaaaa-aaaar-qahzq-cai');
const AMOUNT_E8S = 12_345_678_901n;
const LARGE_NATIVE_AMOUNT = (1n << 64n) + 123_456_789n;

function queueEntry(collateral: Principal, symbol: string, runIndex: number) {
	return {
		run_index: runIndex,
		collateral_type: collateral,
		symbol,
		decimals: symbol === 'ckXAUT' ? 6 : 8,
		price_usd: symbol === 'ICP' ? 6.25 : 2_000,
		price_timestamp_ns: 1_800_000_000_000_000_000n,
		price_fresh: true,
		min_cr: symbol === 'ICP' ? 1.5 : 1.18,
		liquidation_cr: symbol === 'ICP' ? 1.33 : 1.12,
		weakest_vault_cr: symbol === 'ICP' ? 1.5 : 1.45,
		health_headroom: 0.25,
		vault_count: 2n,
		eligible_collateral_raw: LARGE_NATIVE_AMOUNT,
		eligible_debt_e8s: 10_000_000_000n,
		max_input_icusd_e8s: 10_000_000_000n,
		max_net_collateral_raw: 99_000_000n,
	};
}

function redemptionQuote() {
	return {
		quoted_at_ns: 1_800_000_001_000_000_000n,
		quote_validity_window_ns: 60_000_000_000n,
		ranking_fresh: true,
		amount_e8s: AMOUNT_E8S,
		run_index: 0,
		collateral_type: ICP,
		symbol: 'ICP',
		decimals: 8,
		price_usd: 6.25,
		price_timestamp_ns: 1_800_000_000_000_000_000n,
		price_fresh: true,
		fee_e8s: 123_456n,
		rmr: 0.97,
		effective_icusd_e8s: 11_975_308_642n,
		gross_collateral_raw: 191_604_938n,
		ledger_fee_raw: 10_000n,
		net_collateral_raw: 191_594_938n,
		max_input_icusd_e8s: 10_000_000_000n,
	};
}

describe('generated redemption Candid bindings', () => {
	it('exposes and roundtrips the complete health-ranked queue, including exact nat collateral above u64', () => {
		const method = generatedMethod('get_redemption_queue');
		expect(method.argTypes).toHaveLength(0);
		const sample = {
			observed_at_ns: 1_800_000_001_000_000_000n,
			ranking_fresh: true,
			rmr: 0.97,
			price_freshness_window_ns: 600_000_000_000n,
			entries: [
				queueEntry(ICP, 'ICP', 0),
				queueEntry(XAUT, 'ckXAUT', 1),
				queueEntry(ICP, 'ICP', 2),
			],
		};

		const [decoded] = candidRoundTrip(method.retTypes, [sample]) as [typeof sample];
		expect(decoded.entries.map((entry) => entry.symbol)).toEqual(['ICP', 'ckXAUT', 'ICP']);
		expect(decoded.entries[0].eligible_collateral_raw).toBe(LARGE_NATIVE_AMOUNT);
		expect(decoded.entries[0].eligible_collateral_raw).toBeGreaterThan((1n << 64n) - 1n);
		expect(decoded.entries[0].min_cr).toBe(1.5);
		expect(decoded.entries[0].liquidation_cr).toBe(1.33);
	});

	it('exposes an advisory preview whose stale queue and estimate error roundtrip independently', () => {
		const method = generatedMethod('get_redemption_preview');
		expect(method.argTypes).toHaveLength(1);
		const [decodedAmount] = candidRoundTrip(method.argTypes, [AMOUNT_E8S]);
		expect(decodedAmount).toBe(AMOUNT_E8S);

		const samplePreview = {
			queue: {
				observed_at_ns: 1_800_000_001_000_000_000n,
				ranking_fresh: true,
				rmr: 0.97,
				price_freshness_window_ns: 600_000_000_000n,
				entries: [queueEntry(ICP, 'ICP', 0)],
			},
			estimate: { Err: { RedemptionQuoteUnavailable: 'cached ranking is stale; preview remains advisory' } },
		};
		const [decoded] = candidRoundTrip(method.retTypes, [samplePreview]) as [typeof samplePreview];
		expect(decoded.queue.entries[0].eligible_collateral_raw).toBe(LARGE_NATIVE_AMOUNT);
		expect(decoded.estimate).toEqual(samplePreview.estimate);
	});

	it('exposes a separate update offer call with fresh queue on inner capacity error and refresh cooldown at the outer layer', () => {
		const method = generatedMethod('prepare_redemption_offer');
		expect(method.argTypes).toHaveLength(1);
		const [decodedAmount] = candidRoundTrip(method.argTypes, [AMOUNT_E8S]);
		expect(decodedAmount).toBe(AMOUNT_E8S);

		const queue = {
			observed_at_ns: 1_800_000_001_000_000_000n,
			ranking_fresh: true,
			rmr: 0.97,
			price_freshness_window_ns: 600_000_000_000n,
			entries: [queueEntry(ICP, 'ICP', 0)],
		};
		const [prepared] = candidRoundTrip(method.retTypes, [{ Ok: {
			queue,
			quote: { Err: { RedemptionCapacityExceeded: { max_input_icusd_e8s: 7_000_000_000n } } },
		} }]) as [any];
		expect(prepared.Ok.queue.entries[0].eligible_collateral_raw).toBe(LARGE_NATIVE_AMOUNT);
		expect(prepared.Ok.quote.Err.RedemptionCapacityExceeded.max_input_icusd_e8s).toBe(7_000_000_000n);

		const [cooldown] = candidRoundTrip(method.retTypes, [{ Err: { RefreshCooldown: { retry_after_ns: 300_000_000_000n } } }]) as [any];
		expect(cooldown.Err.RefreshCooldown.retry_after_ns).toBe(300_000_000_000n);
	});

	it('exposes a nat64 quote query and roundtrips its typed Ok quote result', () => {
		const method = generatedMethod('get_redemption_quote');
		expect(method.argTypes).toHaveLength(1);
		const [decodedAmount] = candidRoundTrip(method.argTypes, [AMOUNT_E8S]);
		expect(decodedAmount).toBe(AMOUNT_E8S);

		const [decodedResult] = candidRoundTrip(method.retTypes, [{ Ok: redemptionQuote() }]) as [any];
		expect(decodedResult.Ok.amount_e8s).toBe(AMOUNT_E8S);
		expect(decodedResult.Ok.collateral_type.toText()).toBe(ICP.toText());
		expect(decodedResult.Ok.net_collateral_raw).toBe(191_594_938n);
	});

	it('roundtrips the structured redemption errors and the wrapped legacy ProtocolError', () => {
		const method = generatedMethod('get_redemption_quote');
		const errors = [
			{ RedemptionQuoteUnavailable: 'candidate ranking is incomplete' },
			{ RedemptionCapacityExceeded: { max_input_icusd_e8s: AMOUNT_E8S } },
			{ RedemptionPriorityChanged: { expected: ICP, actual: XAUT } },
			{ RedemptionMinimumNotMet: { minimum_net_raw: 99n, actual_net_raw: 98n } },
			{ Protocol: { TemporarilyUnavailable: 'protocol is in read-only mode' } },
		];

		for (const error of errors) {
			const [decoded] = candidRoundTrip(method.retTypes, [{ Err: error }]) as [any];
			expect(Object.keys(decoded.Err)).toEqual(Object.keys(error));
		}

		const [capacity] = candidRoundTrip(method.retTypes, [{ Err: errors[1] }]) as [any];
		expect(capacity.Err.RedemptionCapacityExceeded.max_input_icusd_e8s).toBe(AMOUNT_E8S);
		const [priority] = candidRoundTrip(method.retTypes, [{ Err: errors[2] }]) as [any];
		expect(priority.Err.RedemptionPriorityChanged.expected.toText()).toBe(ICP.toText());
		expect(priority.Err.RedemptionPriorityChanged.actual.toText()).toBe(XAUT.toText());
		const [legacy] = candidRoundTrip(method.retTypes, [{ Err: errors[4] }]) as [any];
		expect(legacy.Err.Protocol).toEqual({ TemporarilyUnavailable: 'protocol is in read-only mode' });
	});

	it('exposes a typed submit request/result and roundtrips the queued collateral receipt', () => {
		const method = generatedMethod('redeem_quoted');
		expect(method.argTypes).toHaveLength(1);
		const request = {
			amount_e8s: AMOUNT_E8S,
			expected_collateral_type: ICP,
			min_net_collateral_raw: 191_594_938n,
		};
		const [decodedRequest] = candidRoundTrip(method.argTypes, [request]) as [typeof request];
		expect(decodedRequest.amount_e8s).toBe(AMOUNT_E8S);
		expect(decodedRequest.expected_collateral_type.toText()).toBe(ICP.toText());
		expect(decodedRequest.min_net_collateral_raw).toBe(request.min_net_collateral_raw);

		const sampleResult = {
			icusd_block_index: 55n,
			fee_paid_e8s: 123_456n,
			collateral_type: ICP,
			symbol: 'ICP',
			decimals: 8,
			net_collateral_raw: 191_594_938n,
			payout_status: { Queued: null },
		};
		const [decodedResult] = candidRoundTrip(method.retTypes, [{ Ok: sampleResult }]) as [any];
		expect(decodedResult.Ok.collateral_type.toText()).toBe(ICP.toText());
		expect(decodedResult.Ok.net_collateral_raw).toBe(sampleResult.net_collateral_raw);
		expect(decodedResult.Ok.payout_status).toEqual({ Queued: null });

		const [decodedError] = candidRoundTrip(method.retTypes, [{ Err: { Protocol: { AnonymousCallerNotAllowed: null } } }]) as [any];
		expect(decodedError.Err).toEqual({ Protocol: { AnonymousCallerNotAllowed: null } });
	});
});
