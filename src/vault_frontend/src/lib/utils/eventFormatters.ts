import { truncatePrincipal } from './principalHelpers';

// Event categories for filtering and badge coloring
export type EventCategory = 'vault' | 'liquidation' | 'stability' | 'redemption' | 'admin';

// Get the variant key from a Candid event object
export function getEventKey(event: any): string {
	return Object.keys(event)[0];
}

// Get a human-readable type label
export function getEventType(event: any): string {
	const key = getEventKey(event);
	const labels: Record<string, string> = {
		open_vault: 'Open Vault',
		close_vault: 'Close Vault',
		borrow_from_vault: 'Borrow',
		repay_to_vault: 'Repay',
		add_margin_to_vault: 'Add Collateral',
		collateral_withdrawn: 'Withdraw All',
		partial_collateral_withdrawn: 'Withdraw Collateral',
		withdraw_and_close_vault: 'Withdraw & Close',
		vault_withdrawn_and_closed: 'Withdraw & Close',
		VaultWithdrawnAndClosed: 'Withdraw & Close',
		liquidate_vault: 'Liquidation',
		partial_liquidate_vault: 'Partial Liquidation',
		redistribute_vault: 'Redistribution',
		redemption_on_vaults: 'Redemption',
		redemption_transfered: 'Redemption Transfer',
		provide_liquidity: 'Deposit to SP',
		withdraw_liquidity: 'Withdraw from SP',
		claim_liquidity_returns: 'Claim SP Returns',
		dust_forgiven: 'Dust Forgiven',
		margin_transfer: 'Margin Transfer',
		admin_mint: 'Admin Mint',
		admin_vault_correction: 'Vault Correction',
		admin_sweep_to_treasury: 'Treasury Sweep',
		reserve_redemption: 'Reserve Redemption',
		init: 'Protocol Init',
		upgrade: 'Protocol Upgrade'
	};
	return labels[key] || key.replace(/_/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase());
}

// Categorize an event for filtering and badge colors
export function getEventCategory(event: any): EventCategory {
	const key = getEventKey(event);

	const vaultOps = [
		'open_vault',
		'close_vault',
		'borrow_from_vault',
		'repay_to_vault',
		'add_margin_to_vault',
		'collateral_withdrawn',
		'partial_collateral_withdrawn',
		'withdraw_and_close_vault',
		'vault_withdrawn_and_closed',
		'VaultWithdrawnAndClosed',
		'margin_transfer',
		'dust_forgiven'
	];
	const liquidationOps = ['liquidate_vault', 'partial_liquidate_vault', 'redistribute_vault'];
	const stabilityOps = ['provide_liquidity', 'withdraw_liquidity', 'claim_liquidity_returns'];
	const redemptionOps = ['redemption_on_vaults', 'redemption_transfered', 'reserve_redemption'];

	if (vaultOps.includes(key)) return 'vault';
	if (liquidationOps.includes(key)) return 'liquidation';
	if (stabilityOps.includes(key)) return 'stability';
	if (redemptionOps.includes(key)) return 'redemption';
	return 'admin';
}

// Get CSS color variable for event category badge
export function getEventBadgeColor(event: any): string {
	const category = getEventCategory(event);
	switch (category) {
		case 'vault':
			return 'var(--rumi-safe)';
		case 'liquidation':
			return 'var(--rumi-danger)';
		case 'stability':
			return 'var(--rumi-purple-accent)';
		case 'redemption':
			return 'var(--rumi-caution)';
		case 'admin':
			return 'var(--rumi-text-muted)';
	}
}

// Format nanosecond timestamp to human-readable string
export function formatTimestamp(nanos: bigint | number): string {
	const ms = Number(BigInt(nanos) / BigInt(1_000_000));
	const date = new Date(ms);
	return date.toLocaleString('en-US', {
		month: 'short',
		day: 'numeric',
		year: 'numeric',
		hour: '2-digit',
		minute: '2-digit',
		second: '2-digit',
		hour12: false
	});
}

// Extract timestamp from event data (returns null for old events without timestamps)
export function getEventTimestamp(event: any): bigint | null {
	const key = getEventKey(event);
	const data = event[key];
	if (!data) return null;
	// Most events have timestamp as opt nat64 (comes as [bigint] or [])
	const ts = data.timestamp;
	if (Array.isArray(ts) && ts.length > 0) return BigInt(ts[0]);
	if (typeof ts === 'bigint' || typeof ts === 'number') return BigInt(ts);
	return null;
}

// Extract caller/principal from event data
export function getEventCaller(event: any): string | null {
	const key = getEventKey(event);
	const data = event[key];
	if (!data) return null;
	// Check caller (opt principal = [Principal] or [])
	const caller = data.caller;
	if (Array.isArray(caller) && caller.length > 0) return caller[0]?.toString?.() ?? null;
	if (caller?.toString) return caller.toString();
	// Check owner
	if (data.owner?.toString) return data.owner.toString();
	// Check liquidator
	const liq = data.liquidator;
	if (Array.isArray(liq) && liq.length > 0) return liq[0]?.toString?.() ?? null;
	// Check vault.owner for OpenVault
	if (data.vault?.owner?.toString) return data.vault.owner.toString();
	return null;
}

// Format e8s amount to human-readable
export function formatAmount(e8s: bigint | number, decimals: number = 8): string {
	const value = Number(BigInt(e8s)) / Math.pow(10, decimals);
	if (value === 0) return '0';
	if (value >= 1)
		return value.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 4 });
	// For small values, show more precision
	const magnitude = Math.floor(Math.log10(Math.abs(value)));
	const places = Math.abs(magnitude) + 2;
	return value.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: places });
}

// Resolve a collateral principal to a human-readable symbol.
// If a resolver function is provided, use it; otherwise try well-known principals.
const KNOWN_COLLATERAL: Record<string, string> = {
	'ryjl3-tyaaa-aaaaa-aaaba-cai': 'ICP',
	'mxzaz-hqaaa-aaaar-qaada-cai': 'ckBTC',
	'ss2fx-dyaaa-aaaar-qacoq-cai': 'ckETH',
	'o7oak-6yaaa-aaaap-qhgbq-cai': 'ckXAUT',
	'7pail-xaaaa-aaaas-aabmq-cai': 'BOB',
	'rh2pm-ryaaa-aaaan-qeniq-cai': 'EXE',
};

export function resolveCollateralSymbol(principal: any, resolver?: (p: string) => string): string {
	const text = principal?.toString?.() ?? principal?.toText?.() ?? String(principal);
	if (resolver) return resolver(text);
	return KNOWN_COLLATERAL[text] ?? text.substring(0, 5) + '…';
}

// Get a one-line human-readable summary of an event
export function getEventSummary(event: any, vaultCollateralMap?: Map<number, any>): string {
	const key = getEventKey(event);
	const data = event[key];

	// Resolve collateral symbol: check event data first, then vault lookup map, then known principals
	function collateralLabel(vaultId?: number): string {
		// 1. Check if event data has collateral_type directly
		const ct = data?.collateral_type;
		if (ct) return resolveCollateralSymbol(ct);
		// 2. Check vault struct (for open_vault events)
		const vaultCt = data?.vault?.collateral_type;
		if (vaultCt) return resolveCollateralSymbol(vaultCt);
		// 3. Look up vault's collateral type from the map
		if (vaultId !== undefined && vaultCollateralMap) {
			const mapped = vaultCollateralMap.get(vaultId);
			if (mapped) return resolveCollateralSymbol(mapped);
		}
		return 'collateral';
	}

	switch (key) {
		case 'open_vault': {
			const label = collateralLabel(Number(data.vault?.vault_id));
			return `Vault #${data.vault.vault_id} opened with ${formatAmount(data.vault.collateral_amount)} ${label}`;
		}
		case 'close_vault':
			return `Vault #${data.vault_id} closed`;
		case 'borrow_from_vault':
			return `Borrowed ${formatAmount(data.borrowed_amount)} icUSD from Vault #${data.vault_id}`;
		case 'repay_to_vault':
			return `Repaid ${formatAmount(data.repayed_amount)} icUSD to Vault #${data.vault_id}`;
		case 'add_margin_to_vault': {
			const label = collateralLabel(Number(data.vault_id));
			return `Added ${formatAmount(data.margin_added)} ${label} to Vault #${data.vault_id}`;
		}
		case 'collateral_withdrawn': {
			const label = collateralLabel(Number(data.vault_id));
			return `Withdrew all ${label} from Vault #${data.vault_id}`;
		}
		case 'partial_collateral_withdrawn': {
			const label = collateralLabel(Number(data.vault_id));
			return `Withdrew ${formatAmount(data.amount)} ${label} from Vault #${data.vault_id}`;
		}
		case 'withdraw_and_close_vault':
		case 'vault_withdrawn_and_closed':
		case 'VaultWithdrawnAndClosed':
			return `Withdrew and closed Vault #${data.vault_id}`;
		case 'liquidate_vault':
			return `Vault #${data.vault_id} fully liquidated`;
		case 'partial_liquidate_vault':
			return `Vault #${data.vault_id} partially liquidated (${formatAmount(data.liquidator_payment)} icUSD)`;
		case 'redistribute_vault':
			return `Vault #${data.vault_id} redistributed`;
		case 'redemption_on_vaults':
			return `Redeemed ${formatAmount(data.icusd_amount)} icUSD`;
		case 'provide_liquidity':
			return `Deposited ${formatAmount(data.amount)} icUSD to Stability Pool`;
		case 'withdraw_liquidity':
			return `Withdrew ${formatAmount(data.amount)} icUSD from Stability Pool`;
		case 'claim_liquidity_returns':
			return `Claimed ${formatAmount(data.amount)} from Stability Pool`;
		case 'dust_forgiven':
			return `Forgave ${formatAmount(data.amount)} dust debt on Vault #${data.vault_id}`;
		case 'admin_mint':
			return `Admin minted ${formatAmount(data.amount)} icUSD`;
		case 'reserve_redemption':
			return `Reserve redemption: ${formatAmount(data.icusd_amount)} icUSD`;
		case 'init':
			return 'Protocol initialized';
		case 'upgrade': {
			const desc = Array.isArray(data?.description) ? data.description[0] : data?.description;
			return desc ? `Protocol upgraded — ${desc}` : 'Protocol upgraded';
		}
		default:
			return getEventType(event);
	}
}

// Extract vault ID from an event (if applicable)
export function getEventVaultId(event: any): number | null {
	const key = getEventKey(event);
	const data = event[key];
	if (data?.vault_id !== undefined) return Number(data.vault_id);
	if (data?.vault?.vault_id !== undefined) return Number(data.vault.vault_id);
	return null;
}

// Check if event is a liquidation type
export function isLiquidationEvent(event: any): boolean {
	const key = getEventKey(event);
	return ['liquidate_vault', 'partial_liquidate_vault', 'redistribute_vault'].includes(key);
}
