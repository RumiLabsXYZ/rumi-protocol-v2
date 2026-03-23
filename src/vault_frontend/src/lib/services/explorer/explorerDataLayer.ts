import { QueryOperations } from '$lib/services/protocol/queryOperations';
import type { ProtocolStatusDTO, CollateralInfo } from '$lib/services/types';

// ── TTL constants ─────────────────────────────────────────────────────────────

const PROTOCOL_STATUS_TTL_MS = 15_000;   // 15 seconds
const COLLATERAL_CONFIGS_TTL_MS = 30_000; // 30 seconds

// ── Generic cache entry ───────────────────────────────────────────────────────

interface CacheEntry<T> {
	value: T;
	fetchedAt: number; // Date.now()
}

// ── Module-level cache store ──────────────────────────────────────────────────

let protocolStatusCache: CacheEntry<ProtocolStatusDTO> | null = null;
let collateralConfigsCache: CacheEntry<CollateralInfo[]> | null = null;

// ── Public API ────────────────────────────────────────────────────────────────

/**
 * Fetch the current protocol status, using a 15-second TTL cache.
 * Multiple callers within the TTL window receive the same cached value.
 */
export async function fetchProtocolStatus(): Promise<ProtocolStatusDTO> {
	const now = Date.now();
	if (protocolStatusCache && now - protocolStatusCache.fetchedAt < PROTOCOL_STATUS_TTL_MS) {
		return protocolStatusCache.value;
	}
	const value = await QueryOperations.getProtocolStatus();
	protocolStatusCache = { value, fetchedAt: Date.now() };
	return value;
}

/**
 * Fetch configs for all supported collateral types, using a 30-second TTL cache.
 * Fetches the list of supported types first, then fetches each config in parallel.
 * Collateral types whose config cannot be fetched are omitted from the result.
 */
export async function fetchAllCollateralConfigs(): Promise<CollateralInfo[]> {
	const now = Date.now();
	if (collateralConfigsCache && now - collateralConfigsCache.fetchedAt < COLLATERAL_CONFIGS_TTL_MS) {
		return collateralConfigsCache.value;
	}

	const supportedTypes = await QueryOperations.getSupportedCollateralTypes();
	const configResults = await Promise.all(
		supportedTypes.map(({ principal }) => QueryOperations.getCollateralConfig(principal))
	);

	const value: CollateralInfo[] = configResults.filter(
		(config): config is CollateralInfo => config !== null
	);

	collateralConfigsCache = { value, fetchedAt: Date.now() };
	return value;
}

/**
 * Invalidate all explorer data caches.
 * Call this after write operations that may change protocol or collateral state.
 */
export function invalidateCache(): void {
	protocolStatusCache = null;
	collateralConfigsCache = null;
}

/**
 * Wrap an async function with retry logic and an optional per-attempt timeout.
 *
 * @param fn       The async function to call. Receives the attempt index (0-based).
 * @param retries  Total number of attempts (default: 3).
 * @param timeout  Per-attempt timeout in milliseconds. No timeout if omitted.
 */
export async function fetchWithRetry<T>(
	fn: (attempt: number) => Promise<T>,
	retries: number = 3,
	timeout?: number
): Promise<T> {
	let lastError: unknown;

	for (let attempt = 0; attempt < retries; attempt++) {
		try {
			if (timeout !== undefined) {
				const result = await Promise.race([
					fn(attempt),
					new Promise<never>((_, reject) =>
						setTimeout(() => reject(new Error(`fetchWithRetry: attempt ${attempt} timed out after ${timeout}ms`)), timeout)
					),
				]);
				return result;
			} else {
				return await fn(attempt);
			}
		} catch (err) {
			lastError = err;
			// Don't delay before the next retry — let the caller decide pacing if needed.
		}
	}

	throw lastError;
}
