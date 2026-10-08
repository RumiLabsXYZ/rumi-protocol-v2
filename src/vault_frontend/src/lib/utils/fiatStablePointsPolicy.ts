/**
 * Runtime policy for the fiat-backed 3pool points rule.
 *
 * The points canister owns this state. The frontend keeps the legacy rule as a
 * compatibility fallback only when the new query is absent (older deployments)
 * and treats an unrecognised response as unavailable. That prevents a stale or
 * partially regenerated client from calling a policy 4x before its epoch.
 */
export type FiatStablePointsMode = 'legacy' | 'flat4x' | 'unknown';
export type FiatStablePointsSource = 'runtime' | 'legacy-fallback';

export interface FiatStableMigrationProgress {
  status: 'not_started' | 'in_progress' | 'complete' | 'unknown';
  processed: bigint | null;
  total: bigint | null;
}

export interface FiatStablePointsPolicy {
  mode: FiatStablePointsMode;
  source: FiatStablePointsSource;
  /** Epoch at which flat 4x becomes effective, if the canister has scheduled one. */
  effectiveEpoch: bigint | null;
  /** Current legacy epoch reported while the boundary is pending. */
  legacyEpoch: bigint | null;
  /** Exclusive immutable ledger prefix used for historical top-ups. */
  historicalLedgerCutoff: bigint | null;
  inlineTopupRows: bigint | null;
  /** Explicit migration flags from the points canister; null for legacy fallback. */
  historicalComplete: boolean | null;
  inlineTopupsComplete: boolean | null;
  pendingCutover: boolean;
  migration: FiatStableMigrationProgress;
}

export const LEGACY_FIAT_STABLE_POINTS_POLICY: FiatStablePointsPolicy = {
  mode: 'legacy',
  source: 'legacy-fallback',
  effectiveEpoch: null,
  legacyEpoch: null,
  historicalLedgerCutoff: null,
  inlineTopupRows: null,
  historicalComplete: null,
  inlineTopupsComplete: null,
  pendingCutover: false,
  migration: { status: 'unknown', processed: null, total: null },
};

function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === 'object' ? (value as Record<string, unknown>) : null;
}

function isNat(value: unknown): value is bigint {
  return typeof value === 'bigint' && value >= 0n;
}

function candidOptionalNat(value: unknown): { valid: boolean; value: bigint | null } {
  if (!Array.isArray(value) || value.length > 1) return { valid: false, value: null };
  if (value.length === 0) return { valid: true, value: null };
  return isNat(value[0]) ? { valid: true, value: value[0] } : { valid: false, value: null };
}

function migrationFromExactFields(
  historicalComplete: boolean,
  historicalNextOffset: bigint,
  inlineComplete: boolean,
  inlineTopupRows: bigint,
): FiatStableMigrationProgress {
  if (historicalComplete && inlineComplete) {
    return { status: 'complete', processed: historicalNextOffset, total: null };
  }
  if (historicalNextOffset > 0n || inlineTopupRows > 0n) {
    return { status: 'in_progress', processed: historicalNextOffset, total: null };
  }
  return { status: 'not_started', processed: 0n, total: null };
}

/** Normalize the exact additive Candid response without importing regenerated bindings. */
export function normalizeFiatStablePointsPolicy(raw: unknown): FiatStablePointsPolicy {
  const r = asRecord(raw);
  if (!r) return { ...LEGACY_FIAT_STABLE_POINTS_POLICY, mode: 'unknown', source: 'runtime' };

  const requiredFields = [
    'cutover_epoch',
    'legacy_epoch',
    'active_for_current_epoch',
    'historical_ledger_cutoff',
    'historical_next_offset',
    'historical_complete',
    'inline_legacy_topups_complete',
    'inline_legacy_topup_rows',
  ];
  if (requiredFields.some((name) => !(name in r))) {
    return { ...LEGACY_FIAT_STABLE_POINTS_POLICY, mode: 'unknown', source: 'runtime' };
  }
  if (typeof r.active_for_current_epoch !== 'boolean') {
    return { ...LEGACY_FIAT_STABLE_POINTS_POLICY, mode: 'unknown', source: 'runtime' };
  }
  if (typeof r.historical_complete !== 'boolean' || typeof r.inline_legacy_topups_complete !== 'boolean') {
    return { ...LEGACY_FIAT_STABLE_POINTS_POLICY, mode: 'unknown', source: 'runtime' };
  }
  const cutoverEpoch = candidOptionalNat(r.cutover_epoch);
  const legacyEpoch = candidOptionalNat(r.legacy_epoch);
  const ledgerCutoff = candidOptionalNat(r.historical_ledger_cutoff);
  if (!cutoverEpoch.valid || !legacyEpoch.valid || !ledgerCutoff.valid) {
    return { ...LEGACY_FIAT_STABLE_POINTS_POLICY, mode: 'unknown', source: 'runtime' };
  }
  if (!isNat(r.historical_next_offset) || !isNat(r.inline_legacy_topup_rows)) {
    return { ...LEGACY_FIAT_STABLE_POINTS_POLICY, mode: 'unknown', source: 'runtime' };
  }
  if (
    cutoverEpoch.value === null &&
    (legacyEpoch.value !== null ||
      r.active_for_current_epoch ||
      ledgerCutoff.value !== null ||
      r.historical_next_offset !== 0n ||
      r.historical_complete ||
      r.inline_legacy_topups_complete ||
      r.inline_legacy_topup_rows !== 0n)
  ) {
    return { ...LEGACY_FIAT_STABLE_POINTS_POLICY, mode: 'unknown', source: 'runtime' };
  }
  if (r.active_for_current_epoch && cutoverEpoch.value === null) {
    return { ...LEGACY_FIAT_STABLE_POINTS_POLICY, mode: 'unknown', source: 'runtime' };
  }
  if (cutoverEpoch.value !== null && ledgerCutoff.value === null) {
    return { ...LEGACY_FIAT_STABLE_POINTS_POLICY, mode: 'unknown', source: 'runtime' };
  }
  if (cutoverEpoch.value !== null && cutoverEpoch.value === 0n) {
    return { ...LEGACY_FIAT_STABLE_POINTS_POLICY, mode: 'unknown', source: 'runtime' };
  }
  if (ledgerCutoff.value !== null && r.historical_next_offset > ledgerCutoff.value) {
    return { ...LEGACY_FIAT_STABLE_POINTS_POLICY, mode: 'unknown', source: 'runtime' };
  }
  if (
    ledgerCutoff.value !== null &&
    r.historical_complete !== (r.historical_next_offset === ledgerCutoff.value)
  ) {
    return { ...LEGACY_FIAT_STABLE_POINTS_POLICY, mode: 'unknown', source: 'runtime' };
  }
  if (
    legacyEpoch.value !== null &&
    (cutoverEpoch.value === null || cutoverEpoch.value !== legacyEpoch.value + 1n)
  ) {
    return { ...LEGACY_FIAT_STABLE_POINTS_POLICY, mode: 'unknown', source: 'runtime' };
  }
  if (
    cutoverEpoch.value !== null &&
    legacyEpoch.value === null &&
    (!r.inline_legacy_topups_complete || r.inline_legacy_topup_rows !== 0n)
  ) {
    return { ...LEGACY_FIAT_STABLE_POINTS_POLICY, mode: 'unknown', source: 'runtime' };
  }
  const isFlat = r.active_for_current_epoch;
  return {
    mode: isFlat ? 'flat4x' : 'legacy',
    source: 'runtime',
    effectiveEpoch: cutoverEpoch.value,
    legacyEpoch: legacyEpoch.value,
    historicalLedgerCutoff: ledgerCutoff.value,
    inlineTopupRows: r.inline_legacy_topup_rows,
    historicalComplete: r.historical_complete,
    inlineTopupsComplete: r.inline_legacy_topups_complete,
    pendingCutover: !isFlat && cutoverEpoch.value !== null,
    migration: migrationFromExactFields(
      r.historical_complete,
      r.historical_next_offset,
      r.inline_legacy_topups_complete,
      r.inline_legacy_topup_rows,
    ),
  };
}

export function fiatStableMigrationNotice(policy: FiatStablePointsPolicy): string | null {
  if (policy.historicalComplete === true && policy.inlineTopupsComplete === false) {
    return 'Recorded historical adjustments are complete. Any further qualifying legacy-epoch accrual receives its adjustment when that epoch closes.';
  }
  if (
    policy.effectiveEpoch !== null &&
    policy.historicalComplete === false &&
    policy.migration.status === 'not_started'
  ) {
    return 'Historical unmatched-row adjustments are scheduled and awaiting processing.';
  }
  if (policy.migration.status === 'in_progress') {
    return 'Historical unmatched-row adjustments are still being applied; existing earned points remain visible.';
  }
  if (policy.migration.status === 'complete') {
    return 'Historical unmatched-row adjustments are complete and shown separately in your earning history.';
  }
  return null;
}

export function fiatStableMultiplier(
  policy: FiatStablePointsPolicy,
  kind: 'matched' | 'unmatched' | 'flat',
): number | null {
  if (policy.mode === 'unknown') return null;
  if (policy.mode === 'flat4x') return 4;
  return kind === 'matched' ? 5 : 3;
}

export function fiatStablePolicyStatus(policy: FiatStablePointsPolicy): string {
  if (policy.mode === 'unknown') return 'Current fiat-stable points policy is unavailable.';
  if (policy.mode === 'flat4x') return 'Flat 4× applies to future ckUSDC and ckUSDT 3pool deposits.';
  if (policy.source === 'legacy-fallback') return 'Using the legacy 5× matched / 3× unmatched rule until the policy query is available.';
  return 'The current epoch still uses the legacy 5× matched / 3× unmatched rule.';
}
