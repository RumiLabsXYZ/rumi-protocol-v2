import { canisterId, createActor } from '../../../../declarations/rumi_cycle_sentinel';
import type {
	PublicAlarm,
	PublicOverview,
	PublicTargetRow,
	_SERVICE
} from '../../../../declarations/rumi_cycle_sentinel/rumi_cycle_sentinel.did';

export type SentinelActor = Pick<
	_SERVICE,
	'get_public_overview' | 'list_public_targets' | 'list_public_alarms'
>;

export type TelemetrySnapshot = {
	overview: PublicOverview;
	targets: PublicTargetRow[];
	alarms: PublicAlarm[];
	refreshedAt: Date;
};

export type TelemetryLoadState = {
	snapshot?: TelemetrySnapshot;
	loading: boolean;
	stale: boolean;
	error?: string;
};

export const PAGE_SIZE = 100;
export const OPERATOR_TELEMETRY_URL = 'https://app.rumiprotocol.com/telemetry';
// `icp deploy` supplies PUBLIC_CANISTER_ID:* rather than this legacy
// declaration's CANISTER_ID_* variable. Keep the deployed mainnet identity as
// a fallback so the public page is live across both build conventions.
export const DEPLOYED_CYCLE_SENTINEL_ID = 'joh3a-5aaaa-aaaap-quy6a-cai';

export const PUBLIC_QUERY_METHODS = [
	'get_public_overview',
	'list_public_targets',
	'list_public_alarms'
] as const;

export type TargetStateName =
	| 'Healthy'
	| 'Low'
	| 'Stopped'
	| 'Uninstalled'
	| 'Unreachable'
	| 'Unobserved';

export const TARGET_STATE_PRESENTATION: Record<
	TargetStateName,
	{ label: string; tone: 'good' | 'warn' | 'bad' | 'neutral' }
> = {
	Healthy: { label: 'Healthy', tone: 'good' },
	Low: { label: 'Low', tone: 'warn' },
	Stopped: { label: 'Stopped', tone: 'bad' },
	Uninstalled: { label: 'Uninstalled', tone: 'bad' },
	Unreachable: { label: 'Unreachable', tone: 'bad' },
	Unobserved: { label: 'Unobserved', tone: 'neutral' }
};

export function variantName(value: Record<string, unknown>): string {
	return Object.keys(value)[0] ?? 'Unknown';
}

export function targetState(row: Pick<PublicTargetRow, 'state'>): TargetStateName {
	return variantName(row.state) as TargetStateName;
}

export function alarmKind(alarm: Pick<PublicAlarm, 'kind'>): string {
	return variantName(alarm.kind);
}

export function optionalBigInt(value: [] | [bigint]): bigint | undefined {
	return value.length === 1 ? value[0] : undefined;
}

export function formatInteger(value: bigint | null | undefined): string {
	return value === null || value === undefined ? 'Not available' : value.toLocaleString('en-US');
}

function formatFixed(value: bigint, scale: bigint, decimals: number): string {
	const whole = value / scale;
	const remainder = value % scale;
	if (remainder === 0n) return whole.toLocaleString('en-US');
	const fraction = remainder.toString().padStart(decimals, '0').replace(/0+$/, '');
	return `${whole.toLocaleString('en-US')}.${fraction}`;
}

export function formatCycles(value: bigint | null | undefined): string {
	if (value === null || value === undefined) return 'Not available';
	return `${formatFixed(value, 1_000_000_000_000n, 12)} T`;
}

export function formatIcpE8s(value: bigint | null | undefined): string {
	if (value === null || value === undefined) return 'Not available';
	return `${formatFixed(value, 100_000_000n, 8)} ICP`;
}

export function formatDuration(seconds: bigint | null | undefined): string {
	if (seconds === null || seconds === undefined) return 'Not available';
	const day = 86_400n;
	const hour = 3_600n;
	if (seconds >= day) return `${seconds / day}d ${(seconds % day) / hour}h`;
	if (seconds >= hour) return `${seconds / hour}h`;
	return `${seconds / 60n}m`;
}

export function formatTimestamp(seconds: bigint | null | undefined): string {
	if (seconds === null || seconds === undefined) return 'Not available';
	if (seconds > BigInt(Number.MAX_SAFE_INTEGER / 1_000)) return `${seconds.toString()} seconds`;
	return new Date(Number(seconds) * 1_000).toLocaleString();
}

export function sentinelCanisterId(
	configuredId: string | undefined = canisterId
): string | undefined {
	return typeof configuredId === 'string' && configuredId.trim() ? configuredId.trim() : undefined;
}

function pageError(prefix: string, value: Record<string, unknown>): Error {
	return new Error(`${prefix}: ${variantName(value)}`);
}

async function collectTargets(actor: SentinelActor): Promise<PublicTargetRow[]> {
	const items: PublicTargetRow[] = [];
	const seen = new Set<string>();
	let cursor: [] | [string] = [];
	for (;;) {
		const result:
			| { Ok: { items: PublicTargetRow[]; next_cursor: [] | [string] } }
			| { Err: Record<string, unknown> } = await actor.list_public_targets(cursor, PAGE_SIZE);
		if ('Err' in result) throw pageError('Unable to read public targets', result.Err);
		items.push(...result.Ok.items);
		if (result.Ok.next_cursor.length === 0) return items;
		const next: string = result.Ok.next_cursor[0];
		if (seen.has(next)) throw new Error('Cycle Sentinel returned a repeated target cursor.');
		seen.add(next);
		cursor = [next];
	}
}

async function collectAlarms(actor: SentinelActor): Promise<PublicAlarm[]> {
	const items: PublicAlarm[] = [];
	const seen = new Set<string>();
	let cursor: [] | [string] = [];
	for (;;) {
		const result:
			| { Ok: { items: PublicAlarm[]; next_cursor: [] | [string] } }
			| { Err: Record<string, unknown> } = await actor.list_public_alarms(cursor, PAGE_SIZE);
		if ('Err' in result) throw pageError('Unable to read public alarms', result.Err);
		items.push(...result.Ok.items);
		if (result.Ok.next_cursor.length === 0) return items;
		const next: string = result.Ok.next_cursor[0];
		if (seen.has(next)) throw new Error('Cycle Sentinel returned a repeated alarm cursor.');
		seen.add(next);
		cursor = [next];
	}
}

export function createAnonymousSentinelActor(
	id = sentinelCanisterId() ?? DEPLOYED_CYCLE_SENTINEL_ID
): SentinelActor | undefined {
	return id ? (createActor(id) as SentinelActor) : undefined;
}

export async function loadTelemetry(actor?: SentinelActor): Promise<TelemetrySnapshot> {
	const resolvedActor = actor ?? createAnonymousSentinelActor();
	if (!resolvedActor) throw new Error('Cycle Sentinel is not configured for this environment.');
	const [overview, targets, alarms] = await Promise.all([
		resolvedActor.get_public_overview(),
		collectTargets(resolvedActor),
		collectAlarms(resolvedActor)
	]);
	return { overview, targets, alarms, refreshedAt: new Date() };
}

export async function refreshTelemetry(
	previous: TelemetrySnapshot | undefined,
	loader: () => Promise<TelemetrySnapshot> = () => loadTelemetry()
): Promise<TelemetryLoadState> {
	try {
		return { snapshot: await loader(), loading: false, stale: false };
	} catch (error) {
		return {
			snapshot: previous,
			loading: false,
			stale: previous !== undefined,
			error: error instanceof Error ? error.message : 'Telemetry is unavailable.'
		};
	}
}
