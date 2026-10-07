export type StableRepayStage = 'prepared' | 'approval_attempted' | 'backend_dispatch_attempted';

export interface LegacyStableRepayIntent {
  version: 1;
  owner: string;
  network: string;
  vaultId: string;
  token: 'CKUSDT' | 'CKUSDC';
  requestedAmountRaw: string;
  stage: StableRepayStage;
  updatedAt: number;
}

export function legacyStableRepayIntentKey(
  owner: string,
  network: string,
  vaultId: number,
  token: 'CKUSDT' | 'CKUSDC'
): string {
  return `rumi_legacy_stable_repay_${encodeURIComponent(network)}_${encodeURIComponent(owner)}_${vaultId}_${token}`;
}

export function legacyStableRepayIntentPrefix(owner: string, network: string): string {
  return `rumi_legacy_stable_repay_${encodeURIComponent(network)}_${encodeURIComponent(owner)}_`;
}

export function parseLegacyStableRepayIntent(raw: string | null): LegacyStableRepayIntent | null {
  if (!raw) return null;
  try {
    const value = JSON.parse(raw);
    if (value?.version !== 1 || typeof value.owner !== 'string' || typeof value.network !== 'string' ||
        !/^\d+$/.test(value.vaultId) || !['CKUSDT', 'CKUSDC'].includes(value.token) ||
        !/^\d+$/.test(value.requestedAmountRaw) || !['prepared', 'approval_attempted', 'backend_dispatch_attempted'].includes(value.stage) ||
        !Number.isFinite(value.updatedAt)) return null;
    return value as LegacyStableRepayIntent;
  } catch {
    return null;
  }
}
