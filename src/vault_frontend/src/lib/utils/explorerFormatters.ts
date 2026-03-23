import { getEventKey, formatAmount, resolveCollateralSymbol, formatTimestamp, getEventTimestamp } from './eventFormatters';

/**
 * A structured key-value field extracted from an event for display on the event detail page.
 * The `type` drives how the UI renders the value (EntityLink, TokenBadge, etc.).
 */
export interface EventField {
	label: string;
	value: string;
	type: 'text' | 'amount' | 'address' | 'vault' | 'token' | 'percentage' | 'timestamp';
	/** For 'vault' type: the vault ID as a number. For 'address' type: the full principal string. */
	linkId?: string | number;
}

/**
 * Extract structured display fields from a protocol event.
 * Returns an array of EventField objects that the detail page can render with
 * appropriate UI components (EntityLinks, TokenBadges, etc.).
 */
export function extractEventFields(event: any): EventField[] {
	const key = getEventKey(event);
	const data = event[key];
	if (!data) return [];

	const fields: EventField[] = [];

	// ── Helpers ────────────────────────────────────────────────────────────────

	function addVaultId(vaultId: any) {
		if (vaultId === undefined || vaultId === null) return;
		const id = Number(vaultId);
		fields.push({ label: 'Vault', value: `#${id}`, type: 'vault', linkId: id });
	}

	function addAddress(label: string, principal: any) {
		if (!principal) return;
		const text = principal?.toString?.() ?? principal?.toText?.() ?? String(principal);
		if (!text) return;
		fields.push({ label, value: text, type: 'address', linkId: text });
	}

	function addOptAddress(label: string, optPrincipal: any) {
		if (Array.isArray(optPrincipal)) {
			if (optPrincipal.length > 0) addAddress(label, optPrincipal[0]);
		} else if (optPrincipal) {
			addAddress(label, optPrincipal);
		}
	}

	function addToken(principal: any) {
		if (!principal) return;
		const text = principal?.toString?.() ?? principal?.toText?.() ?? String(principal);
		const symbol = resolveCollateralSymbol(principal);
		fields.push({ label: 'Collateral', value: symbol, type: 'token', linkId: text });
	}

	function addAmount(label: string, e8s: any, decimals: number = 8, unit: string = 'icUSD') {
		if (e8s === undefined || e8s === null) return;
		const formatted = formatAmount(e8s, decimals);
		fields.push({ label, value: `${formatted} ${unit}`, type: 'amount' });
	}

	function addAmountRaw(label: string, e8s: any, symbol: string, decimals: number = 8) {
		if (e8s === undefined || e8s === null) return;
		const formatted = formatAmount(e8s, decimals);
		fields.push({ label, value: `${formatted} ${symbol}`, type: 'amount' });
	}

	function addRate(label: string, rate: any) {
		if (rate === undefined || rate === null) return;
		fields.push({ label, value: String(rate), type: 'percentage' });
	}

	function addText(label: string, value: any) {
		if (value === undefined || value === null) return;
		fields.push({ label, value: String(value), type: 'text' });
	}

	function addTimestamp(label: string, nanos: any) {
		if (nanos === undefined || nanos === null) return;
		try {
			fields.push({ label, value: formatTimestamp(nanos), type: 'timestamp' });
		} catch {
			// ignore bad timestamps
		}
	}

	// ── Event-specific field extraction ─────────────────────────────────────────

	switch (key) {
		case 'open_vault': {
			const vault = data.vault;
			if (vault) {
				addVaultId(vault.vault_id);
				addAddress('Owner', vault.owner);
				addToken(vault.collateral_type);
				addAmountRaw('Collateral Deposited', vault.collateral_amount, resolveCollateralSymbol(vault.collateral_type));
				addAmount('Debt', vault.borrowed_icusd_amount);
			}
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'close_vault': {
			addVaultId(data.vault_id);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'borrow_from_vault': {
			addVaultId(data.vault_id);
			addAmount('Borrowed', data.borrowed_amount);
			addAmount('Borrowing Fee', data.fee);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'repay_to_vault': {
			addVaultId(data.vault_id);
			addAmount('Repaid', data.repayed_amount);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'add_margin_to_vault': {
			const symbol = resolveCollateralSymbol(data.collateral_type);
			addVaultId(data.vault_id);
			addToken(data.collateral_type);
			addAmountRaw('Collateral Added', data.margin_added, symbol);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'collateral_withdrawn': {
			addVaultId(data.vault_id);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'partial_collateral_withdrawn': {
			const symbol = resolveCollateralSymbol(data.collateral_type);
			addVaultId(data.vault_id);
			addToken(data.collateral_type);
			addAmountRaw('Amount Withdrawn', data.amount, symbol);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'withdraw_and_close_vault':
		case 'vault_withdrawn_and_closed':
		case 'VaultWithdrawnAndClosed': {
			addVaultId(data.vault_id);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'liquidate_vault': {
			addVaultId(data.vault_id);
			addAmount('Liquidator Payment', data.liquidator_payment);
			addOptAddress('Liquidator', data.liquidator);
			addToken(data.collateral_type);
			addAmountRaw('Collateral Seized', data.collateral_amount, resolveCollateralSymbol(data.collateral_type));
			break;
		}

		case 'partial_liquidate_vault': {
			addVaultId(data.vault_id);
			addAmount('Liquidator Payment', data.liquidator_payment);
			addAmount('Debt Repaid', data.debt_repaid);
			addOptAddress('Liquidator', data.liquidator);
			addToken(data.collateral_type);
			addAmountRaw('Collateral Seized', data.collateral_amount, resolveCollateralSymbol(data.collateral_type));
			break;
		}

		case 'redistribute_vault': {
			addVaultId(data.vault_id);
			addAmount('Redistributed Debt', data.redistributed_debt ?? data.amount);
			addToken(data.collateral_type);
			break;
		}

		case 'redemption_on_vaults': {
			addAmount('icUSD Redeemed', data.icusd_amount);
			addAmount('Redemption Fee', data.fee);
			addOptAddress('Caller', data.caller);
			// Vaults redeemed from (array of vault IDs)
			if (Array.isArray(data.vault_ids) && data.vault_ids.length > 0) {
				addText('Vaults Redeemed', data.vault_ids.map((id: any) => `#${id}`).join(', '));
			}
			break;
		}

		case 'redemption_transfered': {
			addAmount('icUSD Amount', data.icusd_amount);
			addAddress('To', data.to);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'provide_liquidity': {
			addAmount('Amount Deposited', data.amount);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'withdraw_liquidity': {
			addAmount('Amount Withdrawn', data.amount);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'claim_liquidity_returns': {
			addAmount('Amount Claimed', data.amount);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'dust_forgiven': {
			addVaultId(data.vault_id);
			addAmount('Dust Amount', data.amount);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'margin_transfer': {
			addVaultId(data.vault_id);
			addToken(data.collateral_type);
			const symbol = resolveCollateralSymbol(data.collateral_type);
			addAmountRaw('Amount', data.amount, symbol);
			addAddress('To', data.to);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'admin_mint': {
			addAmount('Minted', data.amount);
			addAddress('To', data.to);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'admin_vault_correction': {
			addVaultId(data.vault_id);
			addToken(data.collateral_type);
			const sym = resolveCollateralSymbol(data.collateral_type);
			addAmountRaw('Old Amount', data.old_amount, sym);
			addAmountRaw('New Amount', data.new_amount, sym);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'admin_sweep_to_treasury': {
			addAmount('Amount Swept', data.amount);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'reserve_redemption': {
			addAmount('icUSD Amount', data.icusd_amount);
			addAmount('Fee', data.fee);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'update_collateral_config': {
			addToken(data.collateral_type);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'add_collateral_type': {
			addToken(data.collateral_type);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'update_collateral_status': {
			addToken(data.collateral_type);
			if (data.status && typeof data.status === 'object') {
				const statusName = Object.keys(data.status).find((k) => data.status[k] === null) ?? 'Unknown';
				addText('New Status', statusName);
			}
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'set_recovery_parameters': {
			addToken(data.collateral_type);
			addRate('Recovery Target CR', data.recovery_target_cr);
			addRate('Liquidation CR', data.liquidation_cr ?? data.liquidation_ratio);
			addRate('Borrow Threshold CR', data.borrow_threshold_cr ?? data.borrow_threshold_ratio);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'set_recovery_target_cr': {
			addRate('New Target CR', data.rate);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'set_recovery_cr_multiplier': {
			addRate('New Multiplier', data.multiplier ?? data.buffer);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'set_max_partial_liquidation_ratio': {
			addRate('New Ratio', data.rate);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'set_stability_pool_liquidation_share': {
			addRate('New Share', data.share);
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'set_recovery_rate_curve': {
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'init': {
			addOptAddress('Caller', data.caller);
			break;
		}

		case 'upgrade': {
			const desc = Array.isArray(data?.description) ? data.description[0] : data?.description;
			if (desc) addText('Description', desc);
			addOptAddress('Caller', data.caller);
			break;
		}

		default: {
			// Generic fallback: surface any vault_id, caller, owner
			if (data.vault_id !== undefined) addVaultId(data.vault_id);
			if (data.vault?.vault_id !== undefined) addVaultId(data.vault.vault_id);
			addOptAddress('Caller', data.caller);
			if (data.owner) addAddress('Owner', data.owner);
			break;
		}
	}

	// ── Always append timestamp if present ───────────────────────────────────
	const ts = getEventTimestamp(event);
	if (ts !== null) {
		fields.push({ label: 'Timestamp', value: formatTimestamp(ts), type: 'timestamp' });
	}

	return fields;
}

/**
 * Returns a one-sentence description of what an event type does.
 * Used on the event detail page to give context to readers.
 */
export function getEventTypeDescription(eventKey: string): string {
	const descriptions: Record<string, string> = {
		open_vault: 'A new vault was opened by depositing collateral.',
		close_vault: 'An existing vault was closed after all debt was repaid.',
		borrow_from_vault: 'icUSD was minted against a vault\'s collateral.',
		repay_to_vault: 'icUSD was repaid to reduce a vault\'s debt.',
		add_margin_to_vault: 'Additional collateral was deposited into a vault.',
		collateral_withdrawn: 'All collateral was withdrawn from a vault (vault must have zero debt).',
		partial_collateral_withdrawn: 'A portion of collateral was withdrawn from a vault.',
		withdraw_and_close_vault: 'Collateral was withdrawn and the vault was simultaneously closed.',
		vault_withdrawn_and_closed: 'Collateral was withdrawn and the vault was simultaneously closed.',
		VaultWithdrawnAndClosed: 'Collateral was withdrawn and the vault was simultaneously closed.',
		liquidate_vault: 'An undercollateralised vault was fully liquidated.',
		partial_liquidate_vault: 'An undercollateralised vault was partially liquidated.',
		redistribute_vault: 'A vault\'s debt and collateral were redistributed across other vaults.',
		redemption_on_vaults: 'icUSD was redeemed for collateral at face value via vault redemption.',
		redemption_transfered: 'Collateral proceeds from a redemption were transferred to the redeemer.',
		provide_liquidity: 'icUSD was deposited into the Stability Pool.',
		withdraw_liquidity: 'icUSD was withdrawn from the Stability Pool.',
		claim_liquidity_returns: 'Collateral gains were claimed from the Stability Pool.',
		dust_forgiven: 'A small residual debt (dust) on a vault was forgiven by the protocol.',
		margin_transfer: 'Collateral was transferred out of a vault to an address.',
		admin_mint: 'The admin minted icUSD directly (emergency operation).',
		admin_vault_correction: 'The admin corrected the collateral balance recorded on a vault.',
		admin_sweep_to_treasury: 'Funds were swept to the protocol treasury by an admin.',
		reserve_redemption: 'icUSD was redeemed against the protocol\'s ckStable reserve.',
		update_collateral_config: 'The configuration parameters for a collateral type were updated.',
		add_collateral_type: 'A new collateral type was registered with the protocol.',
		update_collateral_status: 'The active/paused/frozen status of a collateral type was changed.',
		set_recovery_parameters: 'Recovery mode parameters for a collateral type were updated.',
		set_recovery_target_cr: 'The recovery mode target collateral ratio was updated.',
		set_recovery_cr_multiplier: 'The recovery mode CR multiplier was updated.',
		set_max_partial_liquidation_ratio: 'The maximum partial liquidation ratio was updated.',
		set_stability_pool_liquidation_share: 'The share of liquidations directed to the Stability Pool was updated.',
		set_recovery_rate_curve: 'The interest rate curve used during recovery mode was updated.',
		init: 'The protocol canister was initialised.',
		upgrade: 'The protocol canister was upgraded.',
	};
	return descriptions[eventKey] ?? 'A protocol event was recorded.';
}
