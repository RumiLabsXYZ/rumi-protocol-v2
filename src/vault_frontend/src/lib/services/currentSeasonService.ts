/** Current Season 2 reads are isolated from the historical Season 1 points service. */
import { Actor, HttpAgent } from '@dfinity/agent';
import { CANISTER_IDS, CONFIG } from '$lib/config';
import { LOCAL_IC_HOST } from '$lib/localNetwork';
import { idlFactory } from '$declarations/rumi_points_season2/rumi_points_season2.did.js';
import type { _SERVICE, PublicEpochStatus, PointsConfig, IngestStatus, ForwardStatus } from '$declarations/rumi_points_season2/rumi_points_season2.did';

export interface CurrentSeason {
  status: PublicEpochStatus;
  config: PointsConfig;
  ingest: IngestStatus;
  highestPoints: bigint;
  forward: ForwardStatus | null;
  observedAt: number;
}
let actor: _SERVICE | null = null;
let pending: Promise<CurrentSeason> | null = null;
let cached: CurrentSeason | null = null;

export async function getCurrentSeason(refresh = false): Promise<CurrentSeason> {
  if (pending) return pending;
  if (!refresh && cached && Date.now() - cached.observedAt < 60_000) return cached;
  if (!actor) {
    const agent = new HttpAgent({ host: CONFIG.isLocal ? LOCAL_IC_HOST : 'https://icp0.io' });
    if (CONFIG.isLocal) await agent.fetchRootKey();
    actor = Actor.createActor<_SERVICE>(idlFactory, { agent, canisterId: CANISTER_IDS.RUMI_POINTS_SEASON2 });
  }
  const source = actor;
  pending = Promise.all([
    source.get_epoch_status(), source.get_points_config(), source.get_ingest_status(), source.get_leaderboard(0, 1), readForwardStatus(source),
  ]).then(([status, config, ingest, leaders, forward]) => {
    cached = { status, config, ingest, forward, highestPoints: leaders[0]?.total_points ?? 0n, observedAt: Date.now() };
    return cached;
  });
  try { return await pending; } finally { pending = null; }
}

// UI may publish before the additive scorer upgrade. Only confirmed method
// absence is compatible; other failures leave current status unconfirmed.
async function readForwardStatus(source: _SERVICE): Promise<ForwardStatus | null> {
  try { return (await source.get_season2_forward_status())[0] ?? null; }
  catch (error) {
    const message = String(error);
    if (message.includes('no query method') || message.includes('IC0536')) return null;
    throw error;
  }
}
