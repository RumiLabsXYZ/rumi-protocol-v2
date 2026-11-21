/** Current Season 2 reads are isolated from the historical Season 1 points service. */
import { Actor, HttpAgent } from '@dfinity/agent';
import { CANISTER_IDS, CONFIG } from '$lib/config';
import { LOCAL_IC_HOST } from '$lib/localNetwork';
import { idlFactory } from '$declarations/rumi_points/rumi_points.did.js';
import type { _SERVICE, PublicEpochStatus, PointsConfig, IngestStatus } from '$declarations/rumi_points/rumi_points.did';

export interface CurrentSeason {
  status: PublicEpochStatus;
  config: PointsConfig;
  ingest: IngestStatus;
  ledgerEntries: bigint;
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
    source.get_epoch_status(), source.get_points_config(), source.get_ingest_status(), source.get_point_ledger_len(),
  ]).then(([status, config, ingest, ledgerEntries]) => {
    cached = { status, config, ingest, ledgerEntries, observedAt: Date.now() };
    return cached;
  });
  try { return await pending; } finally { pending = null; }
}
