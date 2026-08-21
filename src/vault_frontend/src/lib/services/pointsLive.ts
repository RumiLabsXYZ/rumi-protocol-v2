/**
 * pointsLive.ts — fetches the connected principal's CURRENT point-earning
 * positions from the SAME endpoints the rumi_points accrual engine snapshots
 * (`epoch.rs::fetch_raw_snapshot`):
 *
 *   backend  get_vaults(p)          → open vault debt
 *   SP       get_user_position(p)   → icUSD / 3USD stability-pool balances
 *   3pool    get_lp_balance(p)      → wallet 3USD (for the holding rule)
 *   AMM      get_pools + get_lp_balance → 3USD/ICP LP share
 *   backend  get_protocol_status    → ICP/USD rate (AMM ICP-leg valuation)
 *   3pool    virtual price          → 3USD verification valuation
 *
 * Every read is an anonymous query. Failures become `null` (NOT zero) so the
 * pure mirror in `utils/pointsBreakdown.ts` reports "couldn't check" instead
 * of a false $0 — the same Option contract the engine's fetch helpers use.
 */
import type { Principal } from '@dfinity/principal';
import { Actor, HttpAgent, AnonymousIdentity } from '@dfinity/agent';
import { idlFactory as backendIDL } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
import type { _SERVICE as BackendService } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did';
import type { PrincipalState } from '$declarations/rumi_points/rumi_points.did';
import { CANISTER_IDS, CONFIG } from '$lib/config';
import { stabilityPoolService } from './stabilityPoolService';
import { threePoolService } from './threePoolService';
import { ammService, type PoolInfo } from './ammService';
import { getThreeUsdPrice } from './threeUsdPrice';
import type { LiveInputs } from '$lib/utils/pointsBreakdown';

const E8S = 100_000_000;

let _backend: BackendService | null = null;
function backendActor(): BackendService {
  if (_backend) return _backend;
  const agent = new HttpAgent({ host: CONFIG.host, identity: new AnonymousIdentity() });
  if (CONFIG.isLocal) {
    agent.fetchRootKey().catch((e) => console.warn('[pointsLive] fetchRootKey failed', e));
  }
  _backend = Actor.createActor<BackendService>(backendIDL, {
    agent,
    canisterId: CANISTER_IDS.PROTOCOL,
  });
  return _backend;
}

/** The recorded 3pool composition lives on the points canister's
 *  PrincipalState (event-tracked there; not queryable from the 3pool). */
export function recorded3poolFromState(
  state: PrincipalState | null,
): { icusd: number; ckusdc: number; ckusdt: number } {
  const rec = { icusd: 0, ckusdc: 0, ckusdt: 0 };
  if (!state) return rec;
  for (const [key, dep] of state.active_deposits) {
    if (!('ThreePool' in key.venue)) continue;
    const usd = Number(dep.recorded_value_usd) / E8S;
    if ('IcUsd' in key.asset) rec.icusd += usd;
    else if ('CkUsdc' in key.asset) rec.ckusdc += usd;
    else if ('CkUsdt' in key.asset) rec.ckusdt += usd;
  }
  return rec;
}

/** Mirror of epoch.rs::pick_amm_pool — the 3USD/ICP pool, reserves oriented. */
function pickAmmPool(pools: PoolInfo[]): PoolInfo | null {
  return (
    pools.find((p) => {
      const pair = [p.token_a.toText(), p.token_b.toText()];
      return pair.includes(CANISTER_IDS.THREEPOOL) && pair.includes(CANISTER_IDS.ICP_LEDGER);
    }) ?? null
  );
}

async function fetchVaultDebtUsd(p: Principal): Promise<number> {
  const vaults = await backendActor().get_vaults([p]);
  return vaults.reduce((acc, v) => acc + Number(v.borrowed_icusd_amount) / E8S, 0);
}

async function fetchSpBalances(p: Principal): Promise<{ icusd: number; threeUsd: number }> {
  const pos = await stabilityPoolService.getUserPosition(p);
  if (!pos) return { icusd: 0, threeUsd: 0 };
  let icusd = 0;
  let threeUsd = 0;
  for (const [ledger, bal] of pos.stablecoin_balances) {
    const id = ledger.toText();
    if (id === CANISTER_IDS.ICUSD_LEDGER) icusd = Number(bal) / E8S;
    else if (id === CANISTER_IDS.THREEPOOL) threeUsd = Number(bal) / E8S;
  }
  return { icusd, threeUsd };
}

async function fetchAmmShares(p: Principal): Promise<{ threeUsd: number; icp: number }> {
  const pool = pickAmmPool(await ammService.getPools());
  if (!pool || pool.total_lp_shares === 0n) return { threeUsd: 0, icp: 0 };
  const userLp = await ammService.getLpBalance(pool.pool_id, p);
  if (userLp === 0n) return { threeUsd: 0, icp: 0 };
  const share = Number(userLp) / Number(pool.total_lp_shares);
  const [r3usd, rIcp] =
    pool.token_a.toText() === CANISTER_IDS.THREEPOOL
      ? [pool.reserve_a, pool.reserve_b]
      : [pool.reserve_b, pool.reserve_a];
  return { threeUsd: (Number(r3usd) / E8S) * share, icp: (Number(rIcp) / E8S) * share };
}

async function fetchIcpUsd(): Promise<number> {
  const status = await backendActor().get_protocol_status();
  const rate = Number(status.last_icp_rate);
  if (!Number.isFinite(rate) || rate <= 0) throw new Error(`bad icp rate ${rate}`);
  return rate;
}

function settled<T>(r: PromiseSettledResult<T>, what: string): T | null {
  if (r.status === 'fulfilled') return r.value;
  console.warn(`[pointsLive] ${what} read failed`, r.reason);
  return null;
}

/**
 * All live inputs for `buildLivePositions`, fetched concurrently. Individual
 * failures degrade to `null` fields; this function itself never rejects.
 */
export async function fetchLiveInputs(
  p: Principal,
  state: PrincipalState | null,
): Promise<LiveInputs> {
  const [vaultDebt, sp, wallet3usd, amm, icpUsd, virtualPrice] = await Promise.allSettled([
    fetchVaultDebtUsd(p),
    fetchSpBalances(p),
    threePoolService.getLpBalance(p).then((b) => Number(b) / E8S),
    fetchAmmShares(p),
    fetchIcpUsd(),
    getThreeUsdPrice(), // never rejects; falls back to the last known price
  ]);

  const spVal = settled(sp, 'stability pool');
  const ammVal = settled(amm, 'AMM');
  return {
    vaultDebtUsd: settled(vaultDebt, 'vault debt'),
    spIcusd: spVal ? spVal.icusd : null,
    sp3usd: spVal ? spVal.threeUsd : null,
    wallet3usd: settled(wallet3usd, 'wallet 3USD'),
    ammShare3usd: ammVal ? ammVal.threeUsd : null,
    ammShareIcp: ammVal ? ammVal.icp : null,
    recorded3pool: recorded3poolFromState(state),
    icpUsd: settled(icpUsd, 'ICP price'),
    virtualPrice: virtualPrice.status === 'fulfilled' ? virtualPrice.value : 1,
  };
}
