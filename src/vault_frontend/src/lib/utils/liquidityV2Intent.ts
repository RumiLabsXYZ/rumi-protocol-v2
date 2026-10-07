import type { LiquidityV2StatusView } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';

export type LiquidityV2Operation = 'Provide' | 'Withdraw' | 'ClaimReturns';
export interface LiquidityV2Intent {
  version: 1;
  owner: string;
  network: string;
  /** Ledger selected when this request was first created; null only for legacy localStorage records. */
  ledgerPrincipal: string | null;
  requestId: string;
  operation: LiquidityV2Operation;
  amountRaw: string;
  approvalAttempted: boolean;
  backendDispatchAttempted: boolean;
}
export type LiquidityV2Disposition = 'complete' | 'rejected' | 'pending';
const NAT64_MAX = 18_446_744_073_709_551_615n;

export function parseLiquidityAmountRaw(input: string): bigint | null {
  const match = /^(\d+)(?:\.(\d*))?$/.exec(input.trim());
  if (!match || (match[2] ?? '').length > 8) return null;
  const raw = BigInt(match[1]) * 100_000_000n + BigInt((match[2] ?? '').padEnd(8, '0') || '0');
  return raw > 0n && raw <= NAT64_MAX ? raw : null;
}

export function formatLiquidityAmountRaw(raw: bigint): string {
  const whole = raw / 100_000_000n;
  const fraction = (raw % 100_000_000n).toString().padStart(8, '0').replace(/0+$/, '');
  return fraction ? `${whole}.${fraction}` : whole.toString();
}

export function liquidityV2IntentKey(owner: string, network: string): string {
  return `rumi_liquidity_v2_${encodeURIComponent(network)}_${encodeURIComponent(owner)}`;
}

export function liquidityV2ActionLockName(owner: string, networkScope: string): string {
  return `rumi_liquidity_action_${encodeURIComponent(networkScope)}_${encodeURIComponent(owner)}`;
}

export function parseLiquidityV2Intent(raw: string | null): LiquidityV2Intent | null {
  if (!raw) return null;
  try {
    const value = JSON.parse(raw);
    if (value?.version !== 1 || typeof value.owner !== 'string' || typeof value.network !== 'string' ||
        !/^\d+$/.test(value.requestId) || BigInt(value.requestId) <= 0n ||
        !['Provide', 'Withdraw', 'ClaimReturns'].includes(value.operation) ||
        !/^\d+$/.test(value.amountRaw) || BigInt(value.amountRaw) <= 0n ||
        typeof value.approvalAttempted !== 'boolean' || typeof value.backendDispatchAttempted !== 'boolean') return null;
    if (value.ledgerPrincipal !== undefined && value.ledgerPrincipal !== null && typeof value.ledgerPrincipal !== 'string') return null;
    return { ...value, ledgerPrincipal: value.ledgerPrincipal ?? null } as LiquidityV2Intent;
  } catch { return null; }
}

export function liquidityV2StatusMatchesIntent(
  status: LiquidityV2StatusView,
  intent: LiquidityV2Intent,
): boolean {
  return status.owner.toText() === intent.owner && status.request_id === BigInt(intent.requestId) &&
    intent.operation in status.kind && status.amount_raw === BigInt(intent.amountRaw) &&
    !!intent.ledgerPrincipal && status.ledger.toText() === intent.ledgerPrincipal;
}

export function liquidityV2StatusHasOwner(status: LiquidityV2StatusView, owner: string): boolean {
  return status.owner.toText() === owner;
}

/** Identity check for ClaimReturns before its endpoint-pinned amount is known. */
export function liquidityV2ClaimIdentityMatches(
  status: LiquidityV2StatusView,
  intent: LiquidityV2Intent,
): boolean {
  return intent.operation === 'ClaimReturns' && !!intent.ledgerPrincipal &&
    status.owner.toText() === intent.owner && status.request_id === BigInt(intent.requestId) &&
    'ClaimReturns' in status.kind && status.ledger.toText() === intent.ledgerPrincipal;
}

export function liquidityV2MayAdoptClaimAmount(
  intent: LiquidityV2Intent,
  hasExactStatus: boolean,
  hasActiveRequest: boolean,
  nextRequestId: bigint,
): boolean {
  return intent.operation === 'ClaimReturns' && !intent.backendDispatchAttempted && !hasExactStatus &&
    !hasActiveRequest && BigInt(intent.requestId) === nextRequestId;
}

export function liquidityV2Disposition(status: LiquidityV2StatusView): LiquidityV2Disposition {
  if ('Complete' in status.phase && status.result_block_index[0] !== undefined) return 'complete';
  if ('Rejected' in status.phase) return 'rejected';
  return 'pending';
}
