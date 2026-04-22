/**
 * Facet metadata + filter logic for the Activity page query layer.
 *
 * - `extractFacets(de, ...)` pulls every entity/size/time reference out of a
 *   DisplayEvent (which tokens, pools, vaults, principals it touches; its
 *   $-equivalent size; its canonical type key).
 * - `Facets` is the URL-as-state shape for active filters.
 * - `parseFacetsFromUrl` / `buildFacetsQueryString` handle the round-trip.
 * - `matchesFacets(event_facets, active)` is the AND-combined predicate.
 *
 * TODO: swap to server-side once get_events_filtered accepts filter params
 * (Tier 1 gap #1 in docs/superpowers/plans/2026-04-21-explorer-ia-redesign.md).
 */

import { CANISTER_IDS } from '$lib/config';
import {
  KNOWN_TOKENS,
  resolveTokenAlias,
  getTokenDecimals,
} from '$utils/explorerHelpers';
import type { DisplayEvent } from '$utils/displayEvent';

// ─── Type facet options ───────────────────────────────────────────────

export type TypeFacetKey =
  // Vault ops
  | 'open_vault' | 'close_vault' | 'withdraw_and_close' | 'borrow' | 'repay'
  | 'add_margin' | 'withdraw_collateral' | 'partial_withdraw_collateral'
  | 'margin_transfer' | 'dust_forgiven'
  // Liquidations
  | 'full_liquidation' | 'partial_liquidation' | 'redistribute'
  | 'bot_liquidation_claimed' | 'bot_liquidation_confirmed' | 'bot_liquidation_canceled'
  // Redemptions
  | 'redemption' | 'redemption_transfer' | 'reserve_redemption'
  // Stability pool
  | 'sp_deposit' | 'sp_withdraw' | 'sp_claim_collateral'
  | 'sp_deposit_as_3usd' | 'sp_liquidation_executed' | 'sp_other'
  // 3pool
  | '3pool_swap' | '3pool_add_liquidity' | '3pool_remove_liquidity'
  | '3pool_remove_one_coin' | '3pool_donate'
  // AMM
  | 'amm_swap' | 'amm_add_liquidity' | 'amm_remove_liquidity'
  // Multi-hop
  | 'multi_hop_swap'
  // Admin / system (catch-all)
  | 'admin' | 'system';

export interface TypeFacetOption {
  key: TypeFacetKey;
  label: string;
  group: string;
}

/** Grouped type options rendered in the Type dropdown. Order = display order. */
export const TYPE_FACET_OPTIONS: TypeFacetOption[] = [
  { key: 'open_vault', label: 'Open Vault', group: 'Vault Ops' },
  { key: 'close_vault', label: 'Close Vault', group: 'Vault Ops' },
  { key: 'withdraw_and_close', label: 'Withdraw & Close', group: 'Vault Ops' },
  { key: 'borrow', label: 'Borrow', group: 'Vault Ops' },
  { key: 'repay', label: 'Repay', group: 'Vault Ops' },
  { key: 'add_margin', label: 'Add Collateral', group: 'Vault Ops' },
  { key: 'withdraw_collateral', label: 'Withdraw Collateral (all)', group: 'Vault Ops' },
  { key: 'partial_withdraw_collateral', label: 'Withdraw Collateral (partial)', group: 'Vault Ops' },
  { key: 'margin_transfer', label: 'Margin Transfer', group: 'Vault Ops' },
  { key: 'dust_forgiven', label: 'Dust Forgiven', group: 'Vault Ops' },

  { key: 'full_liquidation', label: 'Full Liquidation', group: 'Liquidations' },
  { key: 'partial_liquidation', label: 'Partial Liquidation', group: 'Liquidations' },
  { key: 'redistribute', label: 'Redistribution', group: 'Liquidations' },
  { key: 'bot_liquidation_claimed', label: 'Bot Claimed', group: 'Liquidations' },
  { key: 'bot_liquidation_confirmed', label: 'Bot Confirmed', group: 'Liquidations' },
  { key: 'bot_liquidation_canceled', label: 'Bot Canceled', group: 'Liquidations' },

  { key: 'redemption', label: 'Redemption', group: 'Redemptions' },
  { key: 'redemption_transfer', label: 'Redemption Transfer', group: 'Redemptions' },
  { key: 'reserve_redemption', label: 'Reserve Redemption', group: 'Redemptions' },

  { key: 'sp_deposit', label: 'SP Deposit', group: 'Stability Pool' },
  { key: 'sp_withdraw', label: 'SP Withdraw', group: 'Stability Pool' },
  { key: 'sp_claim_collateral', label: 'SP Claim Collateral', group: 'Stability Pool' },
  { key: 'sp_deposit_as_3usd', label: 'SP Deposit as 3USD', group: 'Stability Pool' },
  { key: 'sp_liquidation_executed', label: 'SP Liquidation Executed', group: 'Stability Pool' },
  { key: 'sp_other', label: 'SP Other', group: 'Stability Pool' },

  { key: '3pool_swap', label: '3Pool Swap', group: '3Pool' },
  { key: '3pool_add_liquidity', label: '3Pool Add Liquidity', group: '3Pool' },
  { key: '3pool_remove_liquidity', label: '3Pool Remove Liquidity', group: '3Pool' },
  { key: '3pool_remove_one_coin', label: '3Pool Remove One Coin', group: '3Pool' },
  { key: '3pool_donate', label: '3Pool Donate', group: '3Pool' },

  { key: 'amm_swap', label: 'AMM Swap', group: 'AMM' },
  { key: 'amm_add_liquidity', label: 'AMM Add Liquidity', group: 'AMM' },
  { key: 'amm_remove_liquidity', label: 'AMM Remove Liquidity', group: 'AMM' },

  { key: 'multi_hop_swap', label: 'Multi-Hop Swap', group: 'Multi-Hop' },

  { key: 'admin', label: 'Admin (all)', group: 'Admin' },
  { key: 'system', label: 'System', group: 'System' },
];

const TYPE_FACET_LABEL_BY_KEY: Record<string, string> = Object.fromEntries(
  TYPE_FACET_OPTIONS.map((o) => [o.key, o.label]),
);

/** Grouping used by the dropdown — category name → option keys in order. */
export const TYPE_FACET_GROUPS: { group: string; keys: TypeFacetKey[] }[] = (() => {
  const byGroup = new Map<string, TypeFacetKey[]>();
  for (const opt of TYPE_FACET_OPTIONS) {
    if (!byGroup.has(opt.group)) byGroup.set(opt.group, []);
    byGroup.get(opt.group)!.push(opt.key);
  }
  return [...byGroup.entries()].map(([group, keys]) => ({ group, keys }));
})();

export function typeFacetLabel(key: string): string {
  return TYPE_FACET_LABEL_BY_KEY[key] ?? key;
}

/**
 * Category-level aliases that expand to a set of TypeFacetKeys. Keeps URLs
 * short — `?type=liquidation` is more readable than the six-key enumeration.
 */
export const TYPE_CATEGORY_ALIASES: Record<string, TypeFacetKey[]> = {
  vault_ops: [
    'open_vault', 'close_vault', 'withdraw_and_close', 'borrow', 'repay',
    'add_margin', 'withdraw_collateral', 'partial_withdraw_collateral',
    'margin_transfer', 'dust_forgiven',
  ],
  liquidation: [
    'full_liquidation', 'partial_liquidation', 'redistribute',
    'bot_liquidation_claimed', 'bot_liquidation_confirmed', 'bot_liquidation_canceled',
  ],
  liquidations: [
    'full_liquidation', 'partial_liquidation', 'redistribute',
    'bot_liquidation_claimed', 'bot_liquidation_confirmed', 'bot_liquidation_canceled',
  ],
  redemption: ['redemption', 'redemption_transfer', 'reserve_redemption'],
  redemptions: ['redemption', 'redemption_transfer', 'reserve_redemption'],
  stability_pool: [
    'sp_deposit', 'sp_withdraw', 'sp_claim_collateral',
    'sp_deposit_as_3usd', 'sp_liquidation_executed', 'sp_other',
  ],
  sp: [
    'sp_deposit', 'sp_withdraw', 'sp_claim_collateral',
    'sp_deposit_as_3usd', 'sp_liquidation_executed', 'sp_other',
  ],
  threepool: [
    '3pool_swap', '3pool_add_liquidity', '3pool_remove_liquidity',
    '3pool_remove_one_coin', '3pool_donate',
  ],
  amm: ['amm_swap', 'amm_add_liquidity', 'amm_remove_liquidity'],
  swap: ['3pool_swap', 'amm_swap', 'multi_hop_swap'],
  swaps: ['3pool_swap', 'amm_swap', 'multi_hop_swap'],
  dex: [
    '3pool_swap', 'amm_swap', 'multi_hop_swap',
    '3pool_add_liquidity', '3pool_remove_liquidity', '3pool_remove_one_coin', '3pool_donate',
    'amm_add_liquidity', 'amm_remove_liquidity',
  ],
};

// ─── Time preset options ──────────────────────────────────────────────

export type TimePresetKey = 'all' | '1h' | '24h' | '7d' | '30d' | 'custom';

export const TIME_PRESETS: { key: TimePresetKey; label: string; durationMs: number | null }[] = [
  { key: '1h', label: '1h', durationMs: 60 * 60 * 1000 },
  { key: '24h', label: '24h', durationMs: 24 * 60 * 60 * 1000 },
  { key: '7d', label: '7d', durationMs: 7 * 24 * 60 * 60 * 1000 },
  { key: '30d', label: '30d', durationMs: 30 * 24 * 60 * 60 * 1000 },
  { key: 'all', label: 'All', durationMs: null },
];

// ─── Size preset options ──────────────────────────────────────────────

export const SIZE_PRESETS: { key: string; label: string; min: number }[] = [
  { key: '1k', label: '> $1k', min: 1_000 },
  { key: '10k', label: '> $10k', min: 10_000 },
  { key: '100k', label: '> $100k', min: 100_000 },
];

// ─── Type classification ──────────────────────────────────────────────

function sourceLabelFromAction(action: string): string {
  return action || '?';
}

/** Resolve a canonical `TypeFacetKey` for a DisplayEvent. */
export function classifyEventType(de: DisplayEvent): TypeFacetKey {
  if (de.source !== 'backend') {
    switch (de.source) {
      case '3pool_swap': return '3pool_swap';
      case 'amm_swap': return 'amm_swap';
      case 'multi_hop_swap': return 'multi_hop_swap';
      case 'amm_liquidity': {
        const a = sourceLabelFromAction(de.event?.action ? Object.keys(de.event.action)[0] : '');
        return a === 'AddLiquidity' ? 'amm_add_liquidity' : 'amm_remove_liquidity';
      }
      case '3pool_liquidity': {
        const a = sourceLabelFromAction(de.event?.action ? Object.keys(de.event.action)[0] : '');
        if (a === 'AddLiquidity') return '3pool_add_liquidity';
        if (a === 'RemoveLiquidity') return '3pool_remove_liquidity';
        if (a === 'RemoveOneCoin') return '3pool_remove_one_coin';
        if (a === 'Donate') return '3pool_donate';
        return '3pool_add_liquidity';
      }
      case 'amm_admin':
      case '3pool_admin':
        return 'admin';
      case 'stability_pool': {
        const et = de.event?.event_type ?? {};
        const k = Object.keys(et)[0] ?? '';
        if (k === 'Deposit') return 'sp_deposit';
        if (k === 'Withdraw') return 'sp_withdraw';
        if (k === 'ClaimCollateral') return 'sp_claim_collateral';
        if (k === 'DepositAs3USD') return 'sp_deposit_as_3usd';
        if (k === 'LiquidationExecuted') return 'sp_liquidation_executed';
        return 'sp_other';
      }
    }
  }

  const variantKey = Object.keys(de.event?.event_type ?? de.event ?? {})[0] ?? '';
  switch (variantKey) {
    case 'open_vault': return 'open_vault';
    case 'close_vault': return 'close_vault';
    case 'withdraw_and_close_vault':
    case 'vault_withdrawn_and_closed':
    case 'VaultWithdrawnAndClosed':
      return 'withdraw_and_close';
    case 'borrow_from_vault': return 'borrow';
    case 'repay_to_vault': return 'repay';
    case 'add_margin_to_vault': return 'add_margin';
    case 'collateral_withdrawn': return 'withdraw_collateral';
    case 'partial_collateral_withdrawn': return 'partial_withdraw_collateral';
    case 'margin_transfer': return 'margin_transfer';
    case 'dust_forgiven': return 'dust_forgiven';

    case 'liquidate_vault': return 'full_liquidation';
    case 'partial_liquidate_vault': return 'partial_liquidation';
    case 'redistribute_vault': return 'redistribute';
    case 'bot_liquidation_claimed': return 'bot_liquidation_claimed';
    case 'bot_liquidation_confirmed': return 'bot_liquidation_confirmed';
    case 'bot_liquidation_canceled': return 'bot_liquidation_canceled';

    case 'redemption_on_vaults': return 'redemption';
    case 'redemption_transfered': return 'redemption_transfer';
    case 'reserve_redemption': return 'reserve_redemption';

    case 'init':
    case 'upgrade':
    case 'accrue_interest':
      return 'system';

    default:
      return 'admin';
  }
}

// ─── Facet extraction ─────────────────────────────────────────────────

export interface EventFacets {
  typeKey: TypeFacetKey;
  tokens: string[];   // token principal strings
  pools: string[];    // '3pool' or AMM pool_id string
  vaultIds: number[]; // numeric vault ids
  principals: string[];
  canisters: string[];
  /** $-equivalent magnitude where derivable; null means "no computable size". */
  sizeUsd: number | null;
  /** Nanoseconds. 0 means "no timestamp". */
  timestampNs: number;
}

function principalText(p: any): string | null {
  if (!p) return null;
  if (typeof p === 'object' && typeof p.toText === 'function') return p.toText();
  if (typeof p === 'string' && p.length > 10) return p;
  return null;
}

function optPrincipalText(p: any): string | null {
  if (Array.isArray(p)) {
    return p.length > 0 ? principalText(p[0]) : null;
  }
  return principalText(p);
}

function pushUnique<T>(arr: T[], v: T | null | undefined): void {
  if (v == null) return;
  if (arr.includes(v)) return;
  arr.push(v);
}

function toUsdFromStablecoin(amount: any, decimals: number): number {
  if (amount == null) return 0;
  const n = Number(amount);
  if (!Number.isFinite(n)) return 0;
  return n / 10 ** decimals;
}

function icpAmountToUsd(amount: any, icpPrice: number | undefined): number | null {
  if (icpPrice == null || !Number.isFinite(icpPrice) || icpPrice <= 0) return null;
  const n = Number(amount);
  if (!Number.isFinite(n)) return null;
  return (n / 1e8) * icpPrice;
}

function collateralAmountToUsd(
  amount: any,
  collateralPrincipal: string | null | undefined,
  priceMap: Map<string, number> | undefined,
): number | null {
  if (!collateralPrincipal) return null;
  const price = priceMap?.get(collateralPrincipal);
  if (price == null || !Number.isFinite(price) || price <= 0) return null;
  const decimals = getTokenDecimals(collateralPrincipal);
  const n = Number(amount);
  if (!Number.isFinite(n)) return null;
  return (n / 10 ** decimals) * price;
}

/**
 * Derive a $-equivalent size for an event when we can.
 * Returns null if size can't be computed (intentionally — those events only
 * get filtered out when the Size facet is active).
 */
function computeSizeUsd(
  de: DisplayEvent,
  priceMap: Map<string, number> | undefined,
  vaultCollateralMap: Map<number, string> | undefined,
): number | null {
  const icpPrice = priceMap?.get(CANISTER_IDS.ICP_LEDGER);

  if (de.source === 'backend') {
    const et = de.event?.event_type ?? de.event;
    const key = Object.keys(et ?? {})[0] ?? '';
    const d = et?.[key] ?? {};

    // icUSD-denominated fields (e8s)
    const icusdField =
      d.borrowed_amount ?? d.repayed_amount ?? d.icusd_amount ?? d.liquidator_payment ?? null;
    if (icusdField != null) {
      return toUsdFromStablecoin(icusdField, 8);
    }

    // Open vault — use icUSD debt at minimum, optionally upgrade with collateral price
    if (key === 'open_vault' && d.vault) {
      const debt = toUsdFromStablecoin(d.vault.borrowed_icusd_amount, 8);
      if (debt > 0) return debt;
      const collUsd = collateralAmountToUsd(d.vault.collateral_amount, principalText(d.vault.collateral_type), priceMap);
      if (collUsd != null) return collUsd;
      return null;
    }

    // Redemption transfer has no amount, falls through to null

    // Margin (add/withdraw) — need collateral price
    if (key === 'add_margin_to_vault') {
      const coll = principalText(d.collateral_type) ?? vaultCollateralMap?.get(Number(d.vault_id)) ?? null;
      return collateralAmountToUsd(d.margin_added, coll, priceMap);
    }
    if (key === 'collateral_withdrawn' || key === 'partial_collateral_withdrawn') {
      const coll = vaultCollateralMap?.get(Number(d.vault_id)) ?? null;
      return collateralAmountToUsd(d.amount, coll, priceMap);
    }

    // SP provide/withdraw/claim
    if (key === 'provide_liquidity' || key === 'withdraw_liquidity') {
      return toUsdFromStablecoin(d.amount, 8);
    }
    if (key === 'claim_liquidity_returns') {
      return icpAmountToUsd(d.amount, icpPrice);
    }

    // Admin/system events — no size
    return null;
  }

  // Non-backend sources
  if (de.source === 'multi_hop_swap') {
    const raw = String(de.event?.stablecoinAmount ?? '').replace(/,/g, '');
    const n = parseFloat(raw);
    return Number.isFinite(n) ? n : null;
  }

  if (de.source === '3pool_swap') {
    const tIn = Number(de.event?.token_in);
    const tOut = Number(de.event?.token_out);
    // 3pool tokens: 0=icUSD (8d), 1=ckUSDT (6d), 2=ckUSDC (6d) — all ~$1
    if (tIn === 0) return toUsdFromStablecoin(de.event.amount_in, 8);
    if (tIn === 1 || tIn === 2) return toUsdFromStablecoin(de.event.amount_in, 6);
    if (tOut === 0) return toUsdFromStablecoin(de.event.amount_out, 8);
    if (tOut === 1 || tOut === 2) return toUsdFromStablecoin(de.event.amount_out, 6);
    return null;
  }

  if (de.source === 'amm_swap') {
    const tokenIn = principalText(de.event?.token_in);
    const tokenOut = principalText(de.event?.token_out);
    const threePool = CANISTER_IDS.THREEPOOL;
    // 3USD LP ≈ $1; use it if present
    if (tokenIn === threePool) return toUsdFromStablecoin(de.event.amount_in, 8);
    if (tokenOut === threePool) return toUsdFromStablecoin(de.event.amount_out, 8);
    // ICP leg — use ICP price if we have one
    if (tokenIn === CANISTER_IDS.ICP_LEDGER) return icpAmountToUsd(de.event.amount_in, icpPrice);
    if (tokenOut === CANISTER_IDS.ICP_LEDGER) return icpAmountToUsd(de.event.amount_out, icpPrice);
    // Other known token with a price
    const priceIn = tokenIn ? priceMap?.get(tokenIn) : null;
    if (priceIn != null && priceIn > 0 && tokenIn) {
      const dec = getTokenDecimals(tokenIn);
      return (Number(de.event.amount_in) / 10 ** dec) * priceIn;
    }
    return null;
  }

  if (de.source === 'amm_liquidity') {
    // Value the LP shares via the 3USD-LP side when possible (~$1/unit).
    const tokenA = principalText(de.event?.token_a);
    const tokenB = principalText(de.event?.token_b);
    const threePool = CANISTER_IDS.THREEPOOL;
    if (tokenA === threePool) return toUsdFromStablecoin(de.event.amount_a, 8);
    if (tokenB === threePool) return toUsdFromStablecoin(de.event.amount_b, 8);
    if (tokenA === CANISTER_IDS.ICP_LEDGER) return icpAmountToUsd(de.event.amount_a, icpPrice);
    if (tokenB === CANISTER_IDS.ICP_LEDGER) return icpAmountToUsd(de.event.amount_b, icpPrice);
    return null;
  }

  if (de.source === '3pool_liquidity') {
    const amounts: any[] = de.event?.amounts ?? [];
    if (!amounts.length) return null;
    const decimalsByIdx = [8, 6, 6];
    let total = 0;
    for (let i = 0; i < amounts.length; i++) {
      const raw = Number(amounts[i] ?? 0);
      if (!Number.isFinite(raw)) continue;
      total += raw / 10 ** (decimalsByIdx[i] ?? 8);
    }
    return total > 0 ? total : null;
  }

  if (de.source === 'stability_pool') {
    const et = de.event?.event_type ?? {};
    const k = Object.keys(et)[0] ?? '';
    const data = et[k] ?? {};
    const ledger = principalText(data.token_ledger) ?? principalText(data.collateral_ledger);
    const decimals = ledger ? getTokenDecimals(ledger) : 8;
    if (k === 'Deposit' || k === 'Withdraw') return toUsdFromStablecoin(data.amount, decimals);
    if (k === 'ClaimCollateral') {
      if (ledger === CANISTER_IDS.ICP_LEDGER) return icpAmountToUsd(data.amount, icpPrice);
      if (ledger) return collateralAmountToUsd(data.amount, ledger, priceMap);
      return null;
    }
    if (k === 'DepositAs3USD') return toUsdFromStablecoin(data.amount_in, decimals);
    if (k === 'LiquidationExecuted') return toUsdFromStablecoin(data.stables_consumed_e8s, 8);
    return null;
  }

  return null;
}

/**
 * Extract every facet-relevant value from a DisplayEvent.
 * The Activity page calls this once per event (for the in-memory array)
 * and then filters against `Facets` using `matchesFacets`.
 */
export function extractFacets(
  de: DisplayEvent,
  priceMap?: Map<string, number>,
  vaultCollateralMap?: Map<number, string>,
  vaultOwnerMap?: Map<number, string>,
): EventFacets {
  const tokens: string[] = [];
  const pools: string[] = [];
  const vaultIds: number[] = [];
  const principals: string[] = [];
  const canisters: string[] = [];

  const typeKey = classifyEventType(de);
  const timestampNs = de.timestamp || 0;

  if (de.source === 'backend') {
    const et = de.event?.event_type ?? de.event;
    const key = Object.keys(et ?? {})[0] ?? '';
    const d = et?.[key] ?? {};

    // Vault ID
    const vaultId =
      d.vault_id != null
        ? Number(d.vault_id)
        : d.vault?.vault_id != null
        ? Number(d.vault.vault_id)
        : null;
    if (vaultId != null && Number.isFinite(vaultId)) {
      vaultIds.push(vaultId);
      // Also attach the owner if we know it
      const owner = vaultOwnerMap?.get(vaultId);
      if (owner) pushUnique(principals, owner);
    }

    // Collateral type
    const collFromEvent = principalText(d.collateral_type);
    if (collFromEvent) pushUnique(tokens, collFromEvent);
    if (d.vault?.collateral_type) pushUnique(tokens, principalText(d.vault.collateral_type));
    if (vaultId != null && vaultCollateralMap?.has(vaultId)) {
      pushUnique(tokens, vaultCollateralMap.get(vaultId)!);
    }

    // Backend events always denominate debt in icUSD
    if (
      key === 'open_vault' || key === 'borrow_from_vault' || key === 'repay_to_vault' ||
      key === 'redemption_on_vaults' || key === 'reserve_redemption' ||
      key === 'provide_liquidity' || key === 'withdraw_liquidity' ||
      key === 'liquidate_vault' || key === 'partial_liquidate_vault' ||
      key === 'dust_forgiven' || key === 'admin_mint' || key === 'admin_sweep_to_treasury'
    ) {
      pushUnique(tokens, CANISTER_IDS.ICUSD_LEDGER);
    }
    if (key === 'reserve_redemption') pushUnique(tokens, principalText(d.stable_token_ledger));

    // Principals (caller / owner / liquidator / redeemer / from / to / developer)
    for (const field of ['caller', 'owner', 'liquidator', 'redeemer', 'from', 'to', 'bot', 'developer_principal', 'treasury']) {
      const p = optPrincipalText(d[field]);
      if (p) pushUnique(principals, p);
    }
    if (d.vault?.owner) pushUnique(principals, principalText(d.vault.owner));

    // Admin events referencing canisters
    for (const field of ['principal', 'canister']) {
      const p = optPrincipalText(d[field]);
      if (p) pushUnique(canisters, p);
    }
  } else if (de.source === '3pool_swap') {
    pools.push('3pool');
    const t = Number(de.event?.token_in);
    const o = Number(de.event?.token_out);
    if (t === 0) pushUnique(tokens, CANISTER_IDS.ICUSD_LEDGER);
    if (t === 1) pushUnique(tokens, CANISTER_IDS.CKUSDT_LEDGER);
    if (t === 2) pushUnique(tokens, CANISTER_IDS.CKUSDC_LEDGER);
    if (o === 0) pushUnique(tokens, CANISTER_IDS.ICUSD_LEDGER);
    if (o === 1) pushUnique(tokens, CANISTER_IDS.CKUSDT_LEDGER);
    if (o === 2) pushUnique(tokens, CANISTER_IDS.CKUSDC_LEDGER);
    pushUnique(principals, principalText(de.event?.caller));
  } else if (de.source === '3pool_liquidity' || de.source === '3pool_admin') {
    pools.push('3pool');
    if (de.source === '3pool_liquidity') {
      // All three stablecoins involved (3pool is a 3-asset curve)
      pushUnique(tokens, CANISTER_IDS.ICUSD_LEDGER);
      pushUnique(tokens, CANISTER_IDS.CKUSDT_LEDGER);
      pushUnique(tokens, CANISTER_IDS.CKUSDC_LEDGER);
    }
    pushUnique(principals, principalText(de.event?.caller));
  } else if (de.source === 'amm_swap') {
    const poolId = de.event?.pool_id;
    if (poolId) pools.push(String(poolId));
    pushUnique(tokens, principalText(de.event?.token_in));
    pushUnique(tokens, principalText(de.event?.token_out));
    pushUnique(principals, principalText(de.event?.caller));
  } else if (de.source === 'amm_liquidity') {
    const poolId = de.event?.pool_id;
    if (poolId) pools.push(String(poolId));
    pushUnique(tokens, principalText(de.event?.token_a));
    pushUnique(tokens, principalText(de.event?.token_b));
    pushUnique(principals, principalText(de.event?.caller));
  } else if (de.source === 'amm_admin') {
    const data = de.event?.action ? Object.values(de.event.action)[0] : null;
    const poolId = (data as any)?.pool_id ?? de.event?.pool_id;
    if (poolId) pools.push(String(poolId));
    pushUnique(principals, principalText(de.event?.caller));
  } else if (de.source === 'stability_pool') {
    const et = de.event?.event_type ?? {};
    const k = Object.keys(et)[0] ?? '';
    const data = et[k] ?? {};
    pushUnique(tokens, principalText(data.token_ledger));
    pushUnique(tokens, principalText(data.collateral_ledger));
    pushUnique(tokens, principalText(data.collateral_type));
    if (data.vault_id != null) {
      const v = Number(data.vault_id);
      if (Number.isFinite(v)) vaultIds.push(v);
    }
    pushUnique(principals, principalText(data.user));
    pushUnique(principals, principalText(de.event?.caller));
  } else if (de.source === 'multi_hop_swap') {
    pools.push('3pool');
    const amm = de.event?.ammEvent;
    if (amm?.pool_id) pools.push(String(amm.pool_id));
    pushUnique(tokens, principalText(amm?.token_in));
    pushUnique(tokens, principalText(amm?.token_out));
    // Stablecoin leg inferred from 3pool event
    const liq = de.event?.liqEvent;
    if (liq) {
      pushUnique(tokens, CANISTER_IDS.ICUSD_LEDGER);
      pushUnique(tokens, CANISTER_IDS.CKUSDT_LEDGER);
      pushUnique(tokens, CANISTER_IDS.CKUSDC_LEDGER);
    }
    pushUnique(principals, principalText(amm?.caller));
    pushUnique(principals, principalText(liq?.caller));
  }

  return {
    typeKey,
    tokens,
    pools,
    vaultIds,
    principals,
    canisters,
    sizeUsd: computeSizeUsd(de, priceMap, vaultCollateralMap),
    timestampNs,
  };
}

// ─── Facet state + URL (de)serialization ─────────────────────────────

export interface Facets {
  types: TypeFacetKey[];        // OR within facet, AND across facets
  tokens: string[];             // principal strings
  pools: string[];              // '3pool' or AMM pool_id
  vaultIds: number[];
  principals: string[];
  time: { preset: TimePresetKey; fromMs?: number; toMs?: number };
  minSizeUsd: number | null;
}

export function emptyFacets(): Facets {
  return {
    types: [],
    tokens: [],
    pools: [],
    vaultIds: [],
    principals: [],
    time: { preset: 'all' },
    minSizeUsd: null,
  };
}

export function hasAnyFacet(f: Facets): boolean {
  return (
    f.types.length > 0 ||
    f.tokens.length > 0 ||
    f.pools.length > 0 ||
    f.vaultIds.length > 0 ||
    f.principals.length > 0 ||
    f.minSizeUsd != null ||
    f.time.preset !== 'all' ||
    f.time.fromMs != null ||
    f.time.toMs != null
  );
}

function parseCsv(raw: string | null | undefined): string[] {
  if (!raw) return [];
  return raw.split(',').map((s) => s.trim()).filter(Boolean);
}

function resolveTokenIdentifier(raw: string): string {
  // Accept aliases like "icp", "icusd", "3usd", or raw principals.
  const alias = resolveTokenAlias(raw);
  if (alias) return alias;
  if (raw in KNOWN_TOKENS) return raw;
  return raw;
}

function parseEntityFacet(raw: string, out: Facets): void {
  // Supported forms:
  //   "vault:42"
  //   "principal:<text>"
  //   "<principal-looking-text>" (fallback to principal)
  //   "42" (fallback to vault id)
  const [kindRaw, ...rest] = raw.split(':');
  const value = rest.length > 0 ? rest.join(':') : kindRaw;
  const kind = rest.length > 0 ? kindRaw.toLowerCase() : null;

  if (kind === 'vault' || kind === 'v') {
    const n = Number(value);
    if (Number.isFinite(n) && n >= 0) out.vaultIds.push(n);
    return;
  }
  if (kind === 'principal' || kind === 'p' || kind === 'addr') {
    if (value) out.principals.push(value);
    return;
  }
  // Fallback: digits → vault, dashed string → principal
  if (/^\d+$/.test(value)) {
    out.vaultIds.push(Number(value));
    return;
  }
  if (value.includes('-') && value.length > 10) {
    out.principals.push(value);
  }
}

/** Parse Facets from a URL. Null time-range values mean "preset-controlled". */
export function parseFacetsFromUrl(url: URL): Facets {
  const f = emptyFacets();
  const params = url.searchParams;

  const types = parseCsv(params.get('type'));
  for (const t of types) {
    if (TYPE_FACET_LABEL_BY_KEY[t]) {
      f.types.push(t as TypeFacetKey);
      continue;
    }
    const expanded = TYPE_CATEGORY_ALIASES[t.toLowerCase()];
    if (expanded) {
      for (const k of expanded) if (!f.types.includes(k)) f.types.push(k);
    }
  }

  for (const t of parseCsv(params.get('token'))) {
    f.tokens.push(resolveTokenIdentifier(t));
  }

  for (const p of parseCsv(params.get('pool'))) {
    f.pools.push(p);
  }

  for (const raw of parseCsv(params.get('entity'))) {
    parseEntityFacet(raw, f);
  }
  // Also support direct vault= and principal= for convenience
  for (const v of parseCsv(params.get('vault'))) {
    const n = Number(v);
    if (Number.isFinite(n)) f.vaultIds.push(n);
  }
  for (const p of parseCsv(params.get('principal'))) {
    if (p) f.principals.push(p);
  }

  const sizeRaw = params.get('size');
  if (sizeRaw) {
    const n = Number(sizeRaw);
    if (Number.isFinite(n) && n > 0) f.minSizeUsd = n;
  }

  const time = params.get('time');
  const validPresets: TimePresetKey[] = ['all', '1h', '24h', '7d', '30d', 'custom'];
  if (time && (validPresets as string[]).includes(time)) {
    f.time.preset = time as TimePresetKey;
  }
  const from = params.get('from');
  const to = params.get('to');
  if (from) {
    const ms = Date.parse(from);
    if (Number.isFinite(ms)) f.time.fromMs = ms;
  }
  if (to) {
    const ms = Date.parse(to);
    if (Number.isFinite(ms)) f.time.toMs = ms;
  }
  if ((f.time.fromMs != null || f.time.toMs != null) && f.time.preset === 'all') {
    f.time.preset = 'custom';
  }

  // Backward-compat with the pre-Step-4 `?filter=...` param.
  const legacy = params.get('filter');
  if (legacy && !params.get('type')) {
    if (legacy === 'liquidations') f.types.push('full_liquidation', 'partial_liquidation', 'redistribute', 'bot_liquidation_confirmed');
    else if (legacy === 'vault_ops') f.types.push('open_vault', 'close_vault', 'withdraw_and_close', 'borrow', 'repay', 'add_margin', 'withdraw_collateral', 'partial_withdraw_collateral');
    else if (legacy === 'dex') f.types.push('3pool_swap', 'amm_swap', 'multi_hop_swap', '3pool_add_liquidity', '3pool_remove_liquidity', '3pool_remove_one_coin', 'amm_add_liquidity', 'amm_remove_liquidity');
    else if (legacy === 'stability_pool') f.types.push('sp_deposit', 'sp_withdraw', 'sp_claim_collateral', 'sp_deposit_as_3usd', 'sp_liquidation_executed', 'sp_other');
    else if (legacy === 'system') f.types.push('admin', 'system');
  }

  return f;
}

/** Build a querystring (leading "?") that round-trips through parseFacetsFromUrl. */
export function buildFacetsQueryString(f: Facets): string {
  const params = new URLSearchParams();
  if (f.types.length) params.set('type', f.types.join(','));
  if (f.tokens.length) params.set('token', f.tokens.join(','));
  if (f.pools.length) params.set('pool', f.pools.join(','));
  if (f.vaultIds.length) params.set('vault', f.vaultIds.join(','));
  if (f.principals.length) params.set('principal', f.principals.join(','));
  if (f.minSizeUsd != null) params.set('size', String(f.minSizeUsd));
  if (f.time.preset !== 'all' && f.time.preset !== 'custom') params.set('time', f.time.preset);
  if (f.time.fromMs != null) params.set('from', new Date(f.time.fromMs).toISOString());
  if (f.time.toMs != null) params.set('to', new Date(f.time.toMs).toISOString());
  const q = params.toString();
  return q ? `?${q}` : '';
}

/** Return the absolute pathname+search that reproduces these facets on the Activity page. */
export function activityHrefFor(f: Facets): string {
  return `/explorer/activity${buildFacetsQueryString(f)}`;
}

// ─── Matching ─────────────────────────────────────────────────────────

/** True iff `event_facets` satisfies every active facet in `active` (AND). */
export function matchesFacets(event_facets: EventFacets, active: Facets): boolean {
  if (active.types.length && !active.types.includes(event_facets.typeKey)) return false;

  if (active.tokens.length) {
    const match = active.tokens.some((t) => event_facets.tokens.includes(t));
    if (!match) return false;
  }

  if (active.pools.length) {
    const match = active.pools.some((p) => event_facets.pools.includes(p));
    if (!match) return false;
  }

  if (active.vaultIds.length) {
    const match = active.vaultIds.some((v) => event_facets.vaultIds.includes(v));
    if (!match) return false;
  }

  if (active.principals.length) {
    const match = active.principals.some((p) => event_facets.principals.includes(p));
    if (!match) return false;
  }

  if (active.minSizeUsd != null) {
    if (event_facets.sizeUsd == null) return false;
    if (event_facets.sizeUsd < active.minSizeUsd) return false;
  }

  // Time
  const presetDuration = TIME_PRESETS.find((p) => p.key === active.time.preset)?.durationMs;
  const nowMs = Date.now();
  const tsMs = event_facets.timestampNs > 0 ? event_facets.timestampNs / 1_000_000 : null;

  if (active.time.preset !== 'all') {
    if (tsMs == null) return false;
    if (active.time.preset === 'custom') {
      if (active.time.fromMs != null && tsMs < active.time.fromMs) return false;
      if (active.time.toMs != null && tsMs > active.time.toMs) return false;
    } else if (presetDuration != null) {
      if (tsMs < nowMs - presetDuration) return false;
    }
  }

  return true;
}

// ─── Mutating helpers (for chip clicks) ───────────────────────────────

export type FacetKind = 'type' | 'token' | 'pool' | 'vault' | 'principal' | 'size' | 'time';

/**
 * Add a single value to a facet, returning a new Facets object.
 * No-op if already present.
 */
export function addFacetValue(f: Facets, kind: FacetKind, value: string | number | TypeFacetKey): Facets {
  const next = structuredCloneFacets(f);
  switch (kind) {
    case 'type':
      if (!next.types.includes(value as TypeFacetKey)) next.types.push(value as TypeFacetKey);
      break;
    case 'token': {
      const v = resolveTokenIdentifier(String(value));
      if (!next.tokens.includes(v)) next.tokens.push(v);
      break;
    }
    case 'pool':
      if (!next.pools.includes(String(value))) next.pools.push(String(value));
      break;
    case 'vault': {
      const n = Number(value);
      if (Number.isFinite(n) && !next.vaultIds.includes(n)) next.vaultIds.push(n);
      break;
    }
    case 'principal': {
      const s = String(value);
      if (s && !next.principals.includes(s)) next.principals.push(s);
      break;
    }
    case 'size':
      next.minSizeUsd = Number(value);
      break;
    case 'time':
      next.time = { preset: value as TimePresetKey };
      break;
  }
  return next;
}

export function removeFacetValue(f: Facets, kind: FacetKind, value: string | number | TypeFacetKey): Facets {
  const next = structuredCloneFacets(f);
  switch (kind) {
    case 'type':
      next.types = next.types.filter((t) => t !== value);
      break;
    case 'token':
      next.tokens = next.tokens.filter((t) => t !== value);
      break;
    case 'pool':
      next.pools = next.pools.filter((p) => p !== value);
      break;
    case 'vault':
      next.vaultIds = next.vaultIds.filter((v) => v !== Number(value));
      break;
    case 'principal':
      next.principals = next.principals.filter((p) => p !== value);
      break;
    case 'size':
      next.minSizeUsd = null;
      break;
    case 'time':
      next.time = { preset: 'all' };
      break;
  }
  return next;
}

function structuredCloneFacets(f: Facets): Facets {
  return {
    types: [...f.types],
    tokens: [...f.tokens],
    pools: [...f.pools],
    vaultIds: [...f.vaultIds],
    principals: [...f.principals],
    time: { ...f.time },
    minSizeUsd: f.minSizeUsd,
  };
}
