import type {
	RedemptionQueue as CanisterRedemptionQueue,
	RedemptionQueueEntry as CanisterRedemptionQueueEntry,
	RedemptionQuote as CanisterRedemptionQuote,
	RedeemQuotedRequest as CanisterRedeemQuotedRequest,
	RedemptionResult as CanisterRedemptionResult,
	_SERVICE,
} from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did';

export type RedemptionQueue = CanisterRedemptionQueue;
export type RedemptionQueueEntry = CanisterRedemptionQueueEntry;
export type RedemptionQuote = CanisterRedemptionQuote;
export type RedemptionQuotedRequest = CanisterRedeemQuotedRequest;
export type RedemptionResult = CanisterRedemptionResult;

export type RedemptionQuoteResult = Awaited<ReturnType<_SERVICE['get_redemption_quote']>>;
export type RedemptionResultVariant = Awaited<ReturnType<_SERVICE['redeem_quoted']>>;

export interface RedemptionPreflight {
	principalText: string;
	walletType: string | null;
	sessionGeneration: number;
	ledgerId: string;
	observedAtMs: number;
	allowanceRaw: bigint;
	balanceRaw: bigint;
	feeRaw: bigint;
}

export interface RedemptionQueueEntryView {
	runIndex: number;
	symbol: string;
	decimals: number;
	priceUsd: number;
	priceFresh: boolean;
	minCr: number;
	liquidationCr: number;
	weakestVaultCr: number;
	healthHeadroom: number;
	vaultCount: number;
	lockedCollateralRaw: bigint;
	maxInputIcusdE8s: bigint;
	maxNetCollateralRaw: bigint;
}

export const ICUSD_E8S = 100_000_000n;

export function toHumanRawAmount(raw: bigint, decimals: number): string {
	const places = Math.max(0, Math.min(18, Math.trunc(decimals)));
	const scale = 10n ** BigInt(places);
	const whole = raw / scale;
	const remainder = raw % scale;
	if (remainder === 0n) return whole.toString();
	const fraction = remainder.toString().padStart(places, '0').replace(/0+$/, '');
	return `${whole}.${fraction}`;
}

export function toHumanIcusd(e8s: bigint): string {
	return toHumanRawAmount(e8s, 8);
}

/** Preserve the backend's globally ordered consecutive runs; never merge a repeated token. */
export function toQueueEntryViews(entries: RedemptionQueueEntry[]): RedemptionQueueEntryView[] {
	return entries.map((entry) => ({
		runIndex: Number(entry.run_index),
		symbol: entry.symbol,
		decimals: Number(entry.decimals),
		priceUsd: Number(entry.price_usd),
		priceFresh: Boolean(entry.price_fresh),
		minCr: Number(entry.min_cr),
		liquidationCr: Number(entry.liquidation_cr),
		weakestVaultCr: Number(entry.weakest_vault_cr),
		healthHeadroom: Number(entry.health_headroom),
		vaultCount: Number(entry.vault_count),
		lockedCollateralRaw: entry.eligible_collateral_raw,
		maxInputIcusdE8s: entry.max_input_icusd_e8s,
		maxNetCollateralRaw: entry.max_net_collateral_raw,
	}));
}

export function quoteIsFresh(
	quote: RedemptionQuote | null,
	queue: RedemptionQueue | null,
	nowNs: bigint = BigInt(Date.now()) * 1_000_000n,
): boolean {
	if (!quote || !queue || !quote.ranking_fresh || queue.price_freshness_window_ns <= 0n) return false;
	if (!queueCandidatePricesFresh(queue, nowNs)) return false;
	const entry = queue.entries.find((candidate) => Number(candidate.run_index) === Number(quote.run_index));
	if (!entry?.price_fresh || !quote.price_fresh) return false;
	const priceAge = nowNs - quote.price_timestamp_ns;
	const quoteAge = nowNs - quote.quoted_at_ns;
	const queueAge = nowNs - queue.observed_at_ns;
	return entry.collateral_type.toText() === quote.collateral_type.toText()
		&& entry.price_timestamp_ns === quote.price_timestamp_ns
		&& entry.price_usd === quote.price_usd
		&& priceAge >= 0n && priceAge <= queue.price_freshness_window_ns
		&& quote.quote_validity_window_ns > 0n
		&& quoteAge >= 0n && quoteAge <= quote.quote_validity_window_ns
		&& queueAge >= 0n && queueAge <= quote.quote_validity_window_ns;
}

/** The displayed global order is only trustworthy while every candidate price is fresh. */
export function queueCandidatePricesFresh(
	queue: RedemptionQueue | null,
	nowNs: bigint = BigInt(Date.now()) * 1_000_000n,
): boolean {
	if (!queue || queue.entries.length === 0 || queue.price_freshness_window_ns <= 0n) return false;
	if (!queue.ranking_fresh) return false;
	const queueAge = nowNs - queue.observed_at_ns;
	if (queueAge < 0n) return false;
	return queue.entries.every((entry) => {
		const age = nowNs - entry.price_timestamp_ns;
		return entry.price_fresh && age >= 0n && age <= queue.price_freshness_window_ns;
	});
}

export function quoteMatchesAmount(quote: RedemptionQuote | null, requestedE8s: bigint): boolean {
	return quote !== null && quote.amount_e8s === requestedE8s;
}

export function redemptionPreflightIsFresh(
	preflight: RedemptionPreflight | null,
	principalText: string | null,
	ledgerId: string,
	walletType: string | null,
	sessionGeneration: number,
	nowMs = Date.now(),
	ttlMs = 30_000,
): boolean {
	if (!preflight || !principalText || preflight.principalText !== principalText || preflight.ledgerId !== ledgerId
		|| preflight.walletType !== walletType || preflight.sessionGeneration !== sessionGeneration) return false;
	const age = nowMs - preflight.observedAtMs;
	return Number.isFinite(age) && age >= 0 && age <= ttlMs
		&& preflight.allowanceRaw >= 0n && preflight.balanceRaw >= 0n && preflight.feeRaw >= 0n;
}

/** Maximum input after preserving one transfer fee, or two when approval is needed. */
export function maxRedeemableInput(
	capacityRaw: bigint | null,
	preflight: RedemptionPreflight | null,
): bigint {
	if (!preflight || preflight.balanceRaw <= 0n || preflight.feeRaw < 0n) return 0n;
	const cap = capacityRaw === null ? preflight.balanceRaw : capacityRaw;
	if (cap <= 0n) return 0n;
	const minimum = (a: bigint, b: bigint) => a < b ? a : b;
	const spendableAfterTransferFee = preflight.balanceRaw > preflight.feeRaw
		? preflight.balanceRaw - preflight.feeRaw : 0n;
	const noApprovalLimit = preflight.allowanceRaw > preflight.feeRaw
		? preflight.allowanceRaw - preflight.feeRaw : 0n;
	const noApprovalMaximum = minimum(cap, minimum(spendableAfterTransferFee, noApprovalLimit));
	const approvalMaximum = preflight.balanceRaw > preflight.feeRaw * 2n
		? minimum(cap, preflight.balanceRaw - preflight.feeRaw * 2n) : 0n;
	return noApprovalMaximum > approvalMaximum ? noApprovalMaximum : approvalMaximum;
}

export function formatUsd(value: number): string {
	if (!Number.isFinite(value) || value < 0) return '—';
	return new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', maximumFractionDigits: 2 }).format(value);
}
