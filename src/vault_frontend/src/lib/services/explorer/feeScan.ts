/** Fee accounting for a fixed event-log prefix. No RPC or floating-point amounts. */
export const FEE_SCAN_SIZE = 2_000n;
export const FEE_SCAN_PAGES_PER_RUN = 25;
export const FEE_WINDOWS_NS = [86_400_000_000_000n, 90n * 86_400_000_000_000n] as const;
export type FeeKind = 'borrow' | 'redemption';
export interface FeeWindow {
	windowNs: bigint;
	borrow: bigint;
	redemption: bigint;
	borrowCount: bigint;
	redemptionCount: bigint;
}
export interface FeeScanState {
	backend: string;
	asOfNs: bigint;
	end: bigint | null;
	cursor: bigint;
	phase: 'idle' | 'scanning' | 'incomplete' | 'complete' | 'error' | 'cancelled';
	error: string | null;
	windows: FeeWindow[];
	undated: Record<FeeKind, { count: bigint; amount: bigint }>;
}
export function newFeeScan(backend: string, asOfNs: bigint, windows = FEE_WINDOWS_NS): FeeScanState {
	return {
		backend, asOfNs, end: null, cursor: 0n, phase: 'idle', error: null,
		windows: windows.map(windowNs => ({ windowNs, borrow: 0n, redemption: 0n, borrowCount: 0n, redemptionCount: 0n })),
		undated: { borrow: { count: 0n, amount: 0n }, redemption: { count: 0n, amount: 0n } },
	};
}
export function copyFeeScan(s: FeeScanState): FeeScanState {
	return { ...s, windows: s.windows.map(w => ({ ...w })), undated: {
		borrow: { ...s.undated.borrow }, redemption: { ...s.undated.redemption },
	} };
}
export function feeNat(value: unknown): bigint {
	if (typeof value !== 'bigint' || value < 0n || value > 0xffff_ffff_ffff_ffffn) {
		throw new Error('Invalid fee event integer');
	}
	return value;
}
export function formatFeeE8s(amount: bigint): string {
	return `${amount / 100_000_000n}.${(amount % 100_000_000n).toString().padStart(8, '0')}`;
}
export function completeFee(s: FeeScanState | null, windowIndex: number, kind: FeeKind): bigint | null {
	return s?.phase === 'complete' && s.undated[kind].count === 0n
		? s.windows[windowIndex][kind] : null;
}

/** Validate a whole page before committing any of its amounts or cursor. */
export function applyFeePage(state: FeeScanState, reply: unknown, scan: bigint): FeeScanState {
	if (state.end === null || scan <= 0n || scan > FEE_SCAN_SIZE || state.cursor + scan > state.end) {
		throw new Error('Invalid fee scan window');
	}
	const page = reply as { next_start?: unknown; reached_end?: unknown; events?: unknown } | null;
	const next = state.cursor + scan;
	if (!page || feeNat(page.next_start) !== next || typeof page.reached_end !== 'boolean'
		|| (page.reached_end && next < state.end) || !Array.isArray(page.events)) {
		throw new Error('Invalid fee scan cursor or premature tail');
	}
	const out = copyFeeScan(state);
	let previous = state.cursor - 1n;
	for (const row of page.events) {
		if (!Array.isArray(row) || row.length !== 2) throw new Error('Invalid fee event row');
		const id = feeNat(row[0]);
		if (id <= previous || id < state.cursor || id >= next) throw new Error('Duplicate or out-of-range fee event');
		previous = id;
		const event = row[1];
		if (!event || typeof event !== 'object' || Object.keys(event).length !== 1) throw new Error('Invalid fee event variant');
		const key = Object.keys(event)[0];
		if (key === 'redemption_transfered') continue; // Delivery is not another fee.
		const kind: FeeKind = key === 'borrow_from_vault' ? 'borrow'
			: key === 'redemption_on_vaults' ? 'redemption' : (() => { throw new Error('Unexpected fee event variant'); })();
		const data = event[key];
		const fee = feeNat(data?.fee_amount);
		const timestamp = data?.timestamp;
		if (!Array.isArray(timestamp) || timestamp.length > 1) throw new Error('Invalid fee event timestamp');
		if (timestamp.length === 0) {
			out.undated[kind].count += 1n;
			out.undated[kind].amount += fee;
			continue;
		}
		const ts = feeNat(timestamp[0]);
		for (const window of out.windows) {
			if (ts >= state.asOfNs - window.windowNs && ts <= state.asOfNs) {
				window[kind] += fee;
				window[kind === 'borrow' ? 'borrowCount' : 'redemptionCount'] += 1n;
			}
		}
	}
	out.cursor = next;
	out.phase = next === state.end ? 'complete' : 'scanning';
	return out;
}
