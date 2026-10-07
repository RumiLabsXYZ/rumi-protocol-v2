import type { RepaymentV2StatusView } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';

export interface RepaymentV2Intent {
  version: 1;
  owner: string;
  network: string;
  requestId: string;
  vaultId: string;
  requestedAmountRaw: string;
  closeAfterRepay: boolean;
  approvalAttempted: boolean;
  backendDispatchAttempted: boolean;
}

export function repaymentV2IntentKey(owner: string, network: string): string {
  return `rumi_repayment_v2_intent_${encodeURIComponent(network)}_${encodeURIComponent(owner)}`;
}

export function parseRepaymentV2Intent(raw: string | null): RepaymentV2Intent | null {
  if (!raw) return null;
  try {
    const value = JSON.parse(raw);
    if (value?.version !== 1 || typeof value.owner !== 'string' || typeof value.network !== 'string' ||
        !/^\d+$/.test(value.requestId) || BigInt(value.requestId) <= 0n ||
        !/^\d+$/.test(value.vaultId) || !/^\d+$/.test(value.requestedAmountRaw) || BigInt(value.requestedAmountRaw) <= 0n ||
        typeof value.closeAfterRepay !== 'boolean' || typeof value.approvalAttempted !== 'boolean' ||
        typeof value.backendDispatchAttempted !== 'boolean') return null;
    return value as RepaymentV2Intent;
  } catch {
    return null;
  }
}

export function repaymentV2StatusMatchesIntent(
  status: RepaymentV2StatusView,
  intent: RepaymentV2Intent,
  icusdLedgerText: string,
): boolean {
  return status.owner.toText() === intent.owner && status.ledger.toText() === icusdLedgerText &&
    status.request_id === BigInt(intent.requestId) && status.vault_id === BigInt(intent.vaultId) &&
    status.requested_amount_raw === BigInt(intent.requestedAmountRaw) &&
    status.close_after_repay === intent.closeAfterRepay;
}

export type RepaymentV2Disposition = 'complete' | 'rejected' | 'needs_additional_repayment' | 'pending';
export type RepaymentV2TransportOutcomeKind = 'dispatched_ok' | 'dispatched_err' | 'ambiguous_transport';

export function repaymentV2Disposition(status: RepaymentV2StatusView): RepaymentV2Disposition {
  if ('Complete' in status.phase) return status.result[0] ? 'complete' : 'pending';
  if ('Rejected' in status.phase) return 'rejected';
  if ('CloseNeedsAdditionalRepayment' in status.phase) return status.result[0] ? 'needs_additional_repayment' : 'pending';
  return 'pending';
}

/** A matching journal row proves success only for a receipt-bearing terminal phase. */
export function repaymentV2TransportOutcome(status: RepaymentV2StatusView): RepaymentV2TransportOutcomeKind {
  if ('Rejected' in status.phase) return 'dispatched_err';
  const disposition = repaymentV2Disposition(status);
  return disposition === 'complete' || disposition === 'needs_additional_repayment'
    ? 'dispatched_ok'
    : 'ambiguous_transport';
}
