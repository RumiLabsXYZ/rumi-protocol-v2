import { describe, expect, it } from 'vitest';
import { Principal } from '@dfinity/principal';
import {
	formatIcp,
	formatTCycles,
	fundingOwner,
	fundingTarget,
	legacyIcpAccountIdentifier,
	optionalBigInt,
	parseTCycles,
	targetFundingPolicyChanged,
	variantLabel,
} from './cycleSentinelFunding';
import type { PublicOverview } from '$declarations/rumi_cycle_sentinel/rumi_cycle_sentinel.did';

const overview = (overrides: Record<string, unknown> = {}) => ({
	...({} as PublicOverview),
	...overrides,
});

describe('cycleSentinelFunding', () => {
	it('parses and formats T-cycles without Number precision loss', () => {
		expect(parseTCycles('2', 'threshold')).toBe(2_000_000_000_000n);
		expect(parseTCycles('2.000000000001', 'threshold')).toBe(2_000_000_000_001n);
		expect(formatTCycles(6_000_000_000_001n)).toBe('6.000000000001');
		expect(() => parseTCycles('1.1234567890123', 'threshold')).toThrow('12 decimal places');
	});

	it('formats zero as zero and missing values as unavailable', () => {
		expect(formatTCycles(0n)).toBe('0');
		expect(formatIcp(0n)).toBe('0');
		expect(formatTCycles(undefined)).toBe('Unavailable');
		expect(formatIcp(undefined)).toBe('Unavailable');
	});

	it('keeps an explicit no-limit option distinct from an unavailable wire field', () => {
		const noLimit = fundingTarget({ burn_anomaly_limit_cycles_per_day: [] } as never);
		const limit = fundingTarget({ burn_anomaly_limit_cycles_per_day: [100n] } as never);
		expect('burn_anomaly_limit_cycles_per_day' in noLimit).toBe(true);
		expect('burn_anomaly_limit_cycles_per_day' in ({} as Record<string, unknown>)).toBe(false);
		expect(optionalBigInt(noLimit.burn_anomaly_limit_cycles_per_day)).toBeUndefined();
		expect(optionalBigInt(limit.burn_anomaly_limit_cycles_per_day)).toBe(100n);
		expect(optionalBigInt(({} as { burn_anomaly_limit_cycles_per_day?: [] | [bigint] }).burn_anomaly_limit_cycles_per_day)).toBeUndefined();
	});

	it('uses the published owner and falls back to the configured Sentinel principal', () => {
		const owner = Principal.fromText('2ibo7-dia');
		expect(fundingOwner(overview({ funding_account_owner: owner }) as PublicOverview, 'aaaaa-aa')).toBe(owner);
		expect(fundingOwner(overview() as PublicOverview, '2ibo7-dia')).toEqual(owner);
	});

	it('derives the legacy default ICP account identifier from the owner principal', () => {
		expect(legacyIcpAccountIdentifier(Principal.fromText('joh3a-5aaaa-aaaap-quy6a-cai'))).toBe(
			'a907060d486046ae0a21bcca2a4a2cbd8c48a9c4a7ab31ffbd06a4b5a20a1e2e',
		);
	});

	it('renders conversion status variants without treating missing data as ready', () => {
		expect(variantLabel({ Ready: null })).toBe('Ready');
		expect(variantLabel({ Blocked: null })).toBe('Blocked');
		expect(variantLabel(undefined)).toBe('Unavailable');
	});

	it('detects a funding edit without requiring unrelated target fields to be rewritten', () => {
		const current = { lowThreshold: '3', refill: '2', dailyCap: '6', cooldown: '3600', burnAnomalyLimit: '4' };
		expect(targetFundingPolicyChanged({ ...current }, current)).toBe(false);
		expect(targetFundingPolicyChanged({ ...current }, { ...current, refill: '2.5' })).toBe(true);
		expect(targetFundingPolicyChanged({ ...current }, { ...current, burnAnomalyLimit: '5' })).toBe(true);
	});
});
