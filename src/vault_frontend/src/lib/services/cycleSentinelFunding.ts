import { AccountIdentifier } from '@dfinity/ledger-icp';
import { Principal } from '@dfinity/principal';
import type { PublicOverview, PublicTargetRow } from '$declarations/rumi_cycle_sentinel/rumi_cycle_sentinel.did';

/** Mainnet ICRC Cycles Ledger used by Cycle Sentinel's primary funding rail. */
export const CYCLES_LEDGER_PRINCIPAL = 'um5iw-rqaaa-aaaaq-qaaba-cai';
/** Mainnet ICP Ledger used by the CMC fallback rail. */
export const ICP_LEDGER_PRINCIPAL = 'ryjl3-tyaaa-aaaaa-aaaba-cai';

export type FundingOverview = PublicOverview & {
	'funding_account_owner'?: Principal;
	'cycles_ledger_balance_cycles'?: [] | [bigint];
	'cycles_ledger_balance_as_of_secs'?: [] | [bigint];
	'icp_ledger_balance_e8s'?: [] | [bigint];
	'icp_ledger_balance_as_of_secs'?: [] | [bigint];
	'min_icp_reserve_e8s'?: [] | [bigint];
	'shared_reserve_conversion_status'?: Record<string, null>;
};

export type FundingTargetRow = PublicTargetRow & {
	'tags'?: string[];
	'burn_anomaly_limit_cycles_per_day'?: [] | [bigint];
	'enabled'?: boolean;
	'auto_topup'?: boolean;
	'paused'?: boolean;
	'daily_cap_cycles'?: bigint;
	'cooldown_secs'?: bigint;
};

export const optionalBigInt = (value: [] | [bigint] | undefined): bigint | undefined => value?.[0];

export function fundingOverview(overview: PublicOverview): FundingOverview {
	return overview as FundingOverview;
}

export function fundingTarget(row: PublicTargetRow): FundingTargetRow {
	return row as FundingTargetRow;
}

export type TargetFundingFields = {
	lowThreshold: string;
	refill: string;
	dailyCap: string;
	cooldown: string;
	burnAnomalyLimit: string;
};

export function targetFundingPolicyChanged(previous: TargetFundingFields | null, current: TargetFundingFields): boolean {
	return previous === null
		|| previous.lowThreshold !== current.lowThreshold
		|| previous.refill !== current.refill
		|| previous.dailyCap !== current.dailyCap
		|| previous.cooldown !== current.cooldown
		|| previous.burnAnomalyLimit !== current.burnAnomalyLimit;
}

export function fundingOwner(overview: PublicOverview, fallbackCanisterId: string): Principal {
	return fundingOverview(overview).funding_account_owner ?? Principal.fromText(fallbackCanisterId);
}

/** The legacy ICP Ledger account ID for the owner's default subaccount. */
export function legacyIcpAccountIdentifier(owner: Principal): string {
	return AccountIdentifier.fromPrincipal({ principal: owner }).toHex();
}

export function parseTCycles(value: string, label: string): bigint {
	const text = value.trim();
	if (!/^\d+(?:\.\d{1,12})?$/.test(text)) {
		throw new Error(`${label} must be a non-negative decimal in T-cycles (up to 12 decimal places).`);
	}
	const [whole, fraction = ''] = text.split('.');
	return BigInt(whole) * 1_000_000_000_000n + BigInt((fraction + '0'.repeat(12)).slice(0, 12));
}

export function formatTCycles(value: bigint | undefined): string {
	if (value === undefined) return 'Unavailable';
	const whole = value / 1_000_000_000_000n;
	const fraction = (value % 1_000_000_000_000n).toString().padStart(12, '0').replace(/0+$/, '');
	return fraction ? `${whole.toString()}.${fraction}` : whole.toString();
}

export function formatIcp(value: bigint | undefined): string {
	if (value === undefined) return 'Unavailable';
	const whole = value / 100_000_000n;
	const fraction = (value % 100_000_000n).toString().padStart(8, '0').replace(/0+$/, '');
	return fraction ? `${whole.toString()}.${fraction}` : whole.toString();
}

export function variantLabel(value: Record<string, unknown> | undefined): string {
	return value ? Object.keys(value)[0] ?? 'Unknown' : 'Unavailable';
}

export function ageLabel(asOfSecs: bigint | undefined, nowSecs = BigInt(Math.floor(Date.now() / 1000))): string {
	if (asOfSecs === undefined) return 'Unavailable';
	if (asOfSecs > nowSecs) return 'just now';
	const age = Number(nowSecs - asOfSecs);
	if (age < 60) return `${age}s ago`;
	if (age < 3600) return `${Math.floor(age / 60)}m ago`;
	return `${Math.floor(age / 3600)}h ago`;
}

export function isStale(asOfSecs: bigint | undefined, nowSecs = BigInt(Math.floor(Date.now() / 1000)), maxAgeSecs = 900n): boolean {
	return asOfSecs === undefined || asOfSecs > nowSecs || nowSecs - asOfSecs > maxAgeSecs;
}
