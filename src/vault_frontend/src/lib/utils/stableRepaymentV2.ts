import type { StableRepaymentV2StatusView } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';

export type StableRepaymentV2Outcome = 'complete' | 'rejected' | 'pending';
export type StableRepaymentV2TransportOutcome = 'dispatched_ok' | 'dispatched_err' | 'ambiguous_transport';

/** The generated Candid method returns an option directly, unlike request state which is Result. */
export function unwrapStableRepaymentV2Status(
  response: [] | [StableRepaymentV2StatusView],
): StableRepaymentV2StatusView | null {
  return response[0] ?? null;
}

export function stableRepaymentV2Outcome(status: StableRepaymentV2StatusView): StableRepaymentV2Outcome {
  if ('Complete' in status.phase && status.result[0]) return 'complete';
  if ('Rejected' in status.phase) return 'rejected';
  return 'pending';
}

export function stableRepaymentV2TransportOutcome(status: StableRepaymentV2StatusView): StableRepaymentV2TransportOutcome {
  const state = stableRepaymentV2Outcome(status);
  return state === 'complete' ? 'dispatched_ok' : state === 'rejected' ? 'dispatched_err' : 'ambiguous_transport';
}

/** Bound approval to the entered principal, current surcharge, and two ledger fees. */
export function stableRepaymentV2RequiredAllowance(amountRawE6: bigint, feeRate: number, ledgerFeeE6 = 10_000n): bigint {
  const scale = 100_000_000n;
  const scaledRate = Number.isFinite(feeRate) && feeRate > 0 ? BigInt(Math.ceil(feeRate * Number(scale))) : 0n;
  const surcharge = (amountRawE6 * scaledRate + scale - 1n) / scale;
  return amountRawE6 + surcharge + ledgerFeeE6 * 2n;
}
