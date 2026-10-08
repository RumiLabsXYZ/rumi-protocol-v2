/**
 * pointsRules.ts — the single source of truth for Season 1 airdrop multipliers,
 * mirrored from the rumi_points canister (accrual.rs / airdrop spec v2). Pure and
 * unit-tested. Every badge, callout, and table in the UI reads from here so the
 * app and the docs can never disagree.
 *
 * All amounts here are USD-equivalent: icUSD, ckUSDC, ckUSDT and 3USD all peg to
 * ~$1, so the entered token amount is its own USD value for multiplier purposes.
 */

export const SEASON_LABEL = 'Season 1';
import {
  fiatStableMultiplier,
  LEGACY_FIAT_STABLE_POINTS_POLICY,
  type FiatStablePointsPolicy,
} from './fiatStablePointsPolicy';

/** Maximum public multiplier. The fiat-stable 4x rule is runtime-owned. */
export const MAX_MULTIPLIER = 5;

/** The repay-with-ckUSDC/ckUSDT 5x window is blocked on an upstream backend field
 *  (RepayToVault.repayment_asset). Advertise it as "coming soon" until it ships;
 *  do not imply it is live. */
export const REPAY_BOOST_COMING_SOON = true;
export const REPAY_BOOST_MULTIPLIER = 5;

export type EarnVenue = 'vault' | 'stabilityPool' | 'threePool' | 'amm';

export interface EarnRule {
  key: string;
  label: string;
  venue: EarnVenue;
  multiplier: number;
  href: string;
}

/** The public multiplier table, ordered high → low for "ways to earn" lists. */
export const EARN_RULES: EarnRule[] = [
  { key: 'ck-matched', label: 'ckUSDC + ckUSDT in the 3pool', venue: 'threePool', multiplier: 5, href: '/3usd' },
  { key: 'ck-unmatched', label: 'ckUSDC or ckUSDT in the 3pool', venue: 'threePool', multiplier: 3, href: '/3usd' },
  { key: 'sp-3usd', label: '3USD in the stability pool', venue: 'stabilityPool', multiplier: 2, href: '/stability-pool' },
  { key: 'amm', label: '3USD/ICP liquidity in the AMM', venue: 'amm', multiplier: 2, href: '/swap' },
  { key: 'vault', label: 'icUSD borrowed against a vault', venue: 'vault', multiplier: 1, href: '/' },
  { key: 'icusd-3pool', label: 'icUSD in the 3pool', venue: 'threePool', multiplier: 1, href: '/3usd' },
  { key: 'sp-icusd', label: 'icUSD in the stability pool', venue: 'stabilityPool', multiplier: 1, href: '/stability-pool' },
];

export function earnRulesForPolicy(policy: FiatStablePointsPolicy): EarnRule[] {
  const multiplier = fiatStableMultiplier(policy, 'flat');
  if (multiplier === null) return [];
  if (policy.mode === 'legacy') return EARN_RULES;
  return EARN_RULES.map((rule) =>
    rule.venue === 'threePool' && rule.key.startsWith('ck-')
      ? { ...rule, multiplier, label: 'ckUSDC or ckUSDT in the 3pool' }
      : rule,
  ).filter((rule) => rule.key !== 'ck-matched');
}

/** Stability-pool deposit multiplier by token symbol (case-insensitive). */
export function spMultiplier(symbol: string | null | undefined): number {
  if (!symbol) return 1;
  const s = symbol.toLowerCase();
  if (s === '3usd' || s === 'threeusd') return 2;
  return 1; // icUSD (and any other stable) in the SP earns 1x
}

export interface ThreePoolMultiplier {
  /** Highest active tier: legacy 5/3 or active flat 4, 1 (icUSD only), 0 (empty/unavailable). */
  headline: number;
  /** Blended effective multiplier = weighted / deposited, rounded to 1 dp. */
  effective: number;
  matchedUsd: number; // 2*min(usdc,usdt) — dollars earning the active fiat-stable rate
  unmatchedUsd: number; // |usdc-usdt| — dollars earning the active fiat-stable rate
  icusdUsd: number; // icUSD dollars earning 1x
  /** A short nudge toward a higher tier, or null. */
  nudge: string | null;
  /** False when the runtime policy response was not understood. */
  policyAvailable: boolean;
}

/**
 * Mirror of accrual.rs snapshot_weights for the 3pool. The active rate is
 * supplied by the canister policy; legacy fallback is matched @ 5x and
 * unmatched @ 3x, while the active cutover is flat @ 4x.
 *   icusd     =  icusd            @ 1x
 * Inputs are USD-equivalent token amounts (>= 0). A token-dust amount of the
 * opposite coin does NOT flip the whole position to 5x — only the matched portion
 * does (the v1 dust-gaming exploit is closed in the canister), so the blended
 * `effective` stays honest.
 */
export function compute3poolMultiplier(input: {
  icusd?: number;
  ckusdc?: number;
  ckusdt?: number;
}, policy: FiatStablePointsPolicy = LEGACY_FIAT_STABLE_POINTS_POLICY): ThreePoolMultiplier {
  const icusd = Math.max(0, input.icusd ?? 0);
  const usdc = Math.max(0, input.ckusdc ?? 0);
  const usdt = Math.max(0, input.ckusdt ?? 0);

  const matchedUsd = 2 * Math.min(usdc, usdt);
  const unmatchedUsd = Math.abs(usdc - usdt);
  const icusdUsd = icusd;

  const deposited = icusd + usdc + usdt;
  const matchedMultiplier = fiatStableMultiplier(policy, 'matched');
  const unmatchedMultiplier = fiatStableMultiplier(policy, 'unmatched');
  const hasFiatStable = matchedUsd + unmatchedUsd > 0;
  const policyAvailable = !hasFiatStable || (matchedMultiplier !== null && unmatchedMultiplier !== null);
  const matchedRate = matchedMultiplier ?? 0;
  const unmatchedRate = unmatchedMultiplier ?? 0;
  const weighted = policyAvailable
    ? icusdUsd * 1 + matchedUsd * (policy.mode === 'flat4x' ? 4 : matchedRate) + unmatchedUsd * (policy.mode === 'flat4x' ? 4 : unmatchedRate)
    : 0;

  let headline = 0;
  if (policyAvailable && matchedUsd > 0) headline = policy.mode === 'flat4x' ? 4 : 5;
  else if (policyAvailable && unmatchedUsd > 0) headline = policy.mode === 'flat4x' ? 4 : 3;
  else if (icusdUsd > 0) headline = 1;

  const effective = deposited > 0 ? Math.round((weighted / deposited) * 10) / 10 : 0;

  let nudge: string | null = null;
  if (matchedUsd === 0 && unmatchedUsd > 0) {
    const missing = usdc === 0 ? 'ckUSDC' : 'ckUSDT';
    nudge = policy.mode === 'flat4x' ? null : `Add ${missing} to reach 5×`;
  } else if (matchedUsd > 0 && unmatchedUsd > 0 && policy.mode !== 'flat4x') {
    nudge = 'Balance the pair so all of it earns 5×';
  }

  return { headline, effective, matchedUsd, unmatchedUsd, icusdUsd, nudge, policyAvailable };
}

/** Human headline for the 3pool callout given a computed multiplier. */
export function threePoolHeadline(m: ThreePoolMultiplier): string {
  if (!m.policyAvailable) return 'Current fiat-stable points rate unavailable';
  if (m.headline === 4) return 'Earning 4× on ckUSDC/ckUSDT in the 3pool';
  if (m.headline === 5 && m.unmatchedUsd === 0 && m.icusdUsd === 0) {
    return 'Earning 5× — matched ckUSDC + ckUSDT pair';
  }
  if (m.headline === 5) return `Earning up to 5× · ≈${m.effective}× blended`;
  if (m.headline === 3) return 'Earning 3× on single-sided ckUSDC/ckUSDT';
  if (m.headline === 1) return 'Earning 1× on icUSD liquidity';
  return 'Add liquidity to start earning points';
}

/** Multiplier label for a (venue, assetSymbol) active deposit on the dashboard. */
export function depositMultiplierLabel(
  venue: EarnVenue,
  assetSymbol: string,
  policy: FiatStablePointsPolicy = LEGACY_FIAT_STABLE_POINTS_POLICY,
): string {
  const s = assetSymbol.toLowerCase();
  if (venue === 'threePool') {
    if (!s.includes('ckusd')) return '1×';
    if (policy.mode === 'flat4x') return '4×';
    if (policy.mode === 'unknown') return 'Rate unavailable';
    return '3–5×';
  }
  if (venue === 'stabilityPool') return spMultiplier(assetSymbol) === 2 ? '2×' : '1×';
  if (venue === 'amm') return '2×';
  return '1×'; // vault
}

/**
 * Format LeaderboardEntry.estimated_share_bps (bps of the Season 1 pool) as a %.
 *
 * NOT used on any user-facing surface: share-of-pool is deliberately kept off
 * the /points page (it exposes how concentrated the season is and implies a
 * payout proportion nothing has committed to). Kept for operator tooling.
 */
export function formatSharePct(bps: number): string {
  if (!Number.isFinite(bps) || bps <= 0) return '—';
  return `${(bps / 100).toFixed(2)}%`;
}
