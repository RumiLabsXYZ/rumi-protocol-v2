import { describe, expect, it } from 'vitest';
import {
  LEGACY_FIAT_STABLE_POINTS_POLICY,
  fiatStableMigrationNotice,
  fiatStableMultiplier,
  normalizeFiatStablePointsPolicy,
} from './fiatStablePointsPolicy';
import { compute3poolMultiplier, threePoolHeadline } from './pointsRules';

describe('fiat stable points policy', () => {
  it('keeps the old rule when the additive query is unavailable', () => {
    expect(fiatStableMultiplier(LEGACY_FIAT_STABLE_POINTS_POLICY, 'matched')).toBe(5);
    expect(fiatStableMultiplier(LEGACY_FIAT_STABLE_POINTS_POLICY, 'unmatched')).toBe(3);
  });

  it('recognizes a live flat4x policy and cutover epoch', () => {
    const policy = normalizeFiatStablePointsPolicy({
      cutover_epoch: [19n],
      legacy_epoch: [],
      active_for_current_epoch: true,
      historical_ledger_cutoff: [42n],
      historical_next_offset: 3n,
      historical_complete: false,
      inline_legacy_topups_complete: true,
      inline_legacy_topup_rows: 0n,
    });
    expect(policy.mode).toBe('flat4x');
    expect(policy.effectiveEpoch).toBe(19n);
    expect(policy.historicalLedgerCutoff).toBe(42n);
    expect(fiatStableMultiplier(policy, 'matched')).toBe(4);
    expect(fiatStableMultiplier(policy, 'unmatched')).toBe(4);
    const m = compute3poolMultiplier({ ckusdc: 100, ckusdt: 40 }, policy);
    expect(m.effective).toBe(4);
    expect(threePoolHeadline(m)).toContain('4×');
  });

  it('keeps the legacy rate while a cutover is pending', () => {
    const policy = normalizeFiatStablePointsPolicy({
      cutover_epoch: [19n],
      legacy_epoch: [18n],
      active_for_current_epoch: false,
      historical_ledger_cutoff: [42n],
      historical_next_offset: 42n,
      historical_complete: true,
      inline_legacy_topups_complete: true,
      inline_legacy_topup_rows: 2n,
    });
    expect(policy.mode).toBe('legacy');
    expect(policy.pendingCutover).toBe(true);
    expect(fiatStableMultiplier(policy, 'matched')).toBe(5);
    expect(fiatStableMultiplier(policy, 'unmatched')).toBe(3);
  });

  it('distinguishes a completed historical prefix from pending inline legacy adjustments', () => {
    const policy = normalizeFiatStablePointsPolicy({
      cutover_epoch: [19n],
      legacy_epoch: [18n],
      active_for_current_epoch: false,
      historical_ledger_cutoff: [401n],
      historical_next_offset: 401n,
      historical_complete: true,
      inline_legacy_topups_complete: false,
      inline_legacy_topup_rows: 0n,
    });
    expect(policy.historicalComplete).toBe(true);
    expect(policy.inlineTopupsComplete).toBe(false);
    expect(fiatStableMigrationNotice(policy)).toBe(
      'Recorded historical adjustments are complete. Any further qualifying legacy-epoch accrual receives its adjustment when that epoch closes.',
    );
  });

  it('keeps the in-progress wording while the historical prefix cursor is unfinished', () => {
    const policy = normalizeFiatStablePointsPolicy({
      cutover_epoch: [19n],
      legacy_epoch: [18n],
      active_for_current_epoch: false,
      historical_ledger_cutoff: [401n],
      historical_next_offset: 37n,
      historical_complete: false,
      inline_legacy_topups_complete: false,
      inline_legacy_topup_rows: 0n,
    });
    expect(fiatStableMigrationNotice(policy)).toContain('still being applied');
  });

  it('shows scheduled migration before the first historical prefix row is processed', () => {
    const policy = normalizeFiatStablePointsPolicy({
      cutover_epoch: [19n],
      legacy_epoch: [18n],
      active_for_current_epoch: false,
      historical_ledger_cutoff: [401n],
      historical_next_offset: 0n,
      historical_complete: false,
      inline_legacy_topups_complete: true,
      inline_legacy_topup_rows: 0n,
    });
    expect(policy.migration.status).toBe('not_started');
    expect(fiatStableMigrationNotice(policy)).toBe(
      'Historical unmatched-row adjustments are scheduled and awaiting processing.',
    );
  });

  it('does not convert unknown runtime variants into zero or a false 4x', () => {
    const policy = normalizeFiatStablePointsPolicy({
      active_for_current_epoch: { FuturePolicy: null },
      cutover_epoch: [19n],
      legacy_epoch: [18n],
      historical_ledger_cutoff: [],
      historical_next_offset: 0n,
      historical_complete: false,
      inline_legacy_topups_complete: false,
      inline_legacy_topup_rows: 0n,
    });
    expect(policy.mode).toBe('unknown');
    expect(fiatStableMultiplier(policy, 'flat')).toBeNull();
    const m = compute3poolMultiplier({ ckusdc: 100 }, policy);
    expect(m.policyAvailable).toBe(false);
    expect(threePoolHeadline(m)).toContain('unavailable');
  });

  it('rejects alias fields and incomplete runtime records', () => {
    expect(normalizeFiatStablePointsPolicy({ flat4x_active: true }).mode).toBe('unknown');
    expect(
      normalizeFiatStablePointsPolicy({
        cutover_epoch: [19n],
        legacy_epoch: [18n],
        active_for_current_epoch: false,
        historical_ledger_cutoff: [],
        historical_next_offset: 0,
        historical_complete: false,
        inline_legacy_topups_complete: false,
        inline_legacy_topup_rows: 0n,
      }).mode,
    ).toBe('unknown');
  });

  it('rejects semantically inconsistent epoch metadata', () => {
    const base = {
      cutover_epoch: [19n],
      legacy_epoch: [18n],
      active_for_current_epoch: true,
      historical_ledger_cutoff: [401n],
      historical_next_offset: 401n,
      historical_complete: true,
      inline_legacy_topups_complete: true,
      inline_legacy_topup_rows: 0n,
    };
    expect(normalizeFiatStablePointsPolicy({ ...base, cutover_epoch: [] }).mode).toBe('unknown');
    expect(normalizeFiatStablePointsPolicy({ ...base, historical_ledger_cutoff: [] }).mode).toBe('unknown');
    expect(normalizeFiatStablePointsPolicy({ ...base, historical_next_offset: 402n }).mode).toBe('unknown');
    expect(normalizeFiatStablePointsPolicy({ ...base, legacy_epoch: [19n] }).mode).toBe('unknown');
    expect(normalizeFiatStablePointsPolicy({ ...base, legacy_epoch: [20n] }).mode).toBe('unknown');
    expect(normalizeFiatStablePointsPolicy({ ...base, historical_next_offset: 400n }).mode).toBe('unknown');
    expect(normalizeFiatStablePointsPolicy({ ...base, historical_complete: false }).mode).toBe('unknown');
    expect(normalizeFiatStablePointsPolicy({ ...base, cutover_epoch: [20n] }).mode).toBe('unknown');
    expect(normalizeFiatStablePointsPolicy({ ...base, cutover_epoch: [0n], legacy_epoch: [] }).mode).toBe('unknown');
  });

  it('accepts the pristine no-cutover backend default', () => {
    const policy = normalizeFiatStablePointsPolicy({
      cutover_epoch: [],
      legacy_epoch: [],
      active_for_current_epoch: false,
      historical_ledger_cutoff: [],
      historical_next_offset: 0n,
      historical_complete: false,
      inline_legacy_topups_complete: false,
      inline_legacy_topup_rows: 0n,
    });
    expect(policy.mode).toBe('legacy');
    expect(policy.pendingCutover).toBe(false);
  });

  it('rejects progress or completion flags without a cutover', () => {
    const pristine = {
      cutover_epoch: [],
      legacy_epoch: [],
      active_for_current_epoch: false,
      historical_ledger_cutoff: [],
      historical_next_offset: 0n,
      historical_complete: false,
      inline_legacy_topups_complete: false,
      inline_legacy_topup_rows: 0n,
    };
    expect(normalizeFiatStablePointsPolicy({ ...pristine, historical_next_offset: 1n }).mode).toBe('unknown');
    expect(normalizeFiatStablePointsPolicy({ ...pristine, historical_complete: true }).mode).toBe('unknown');
    expect(normalizeFiatStablePointsPolicy({ ...pristine, inline_legacy_topups_complete: true }).mode).toBe('unknown');
    expect(normalizeFiatStablePointsPolicy({ ...pristine, inline_legacy_topup_rows: 1n }).mode).toBe('unknown');
    expect(normalizeFiatStablePointsPolicy({ ...pristine, historical_ledger_cutoff: [1n] }).mode).toBe('unknown');
  });

  it('rejects scheduled no-legacy migration with inline work remaining', () => {
    expect(
      normalizeFiatStablePointsPolicy({
        cutover_epoch: [19n],
        legacy_epoch: [],
        active_for_current_epoch: false,
        historical_ledger_cutoff: [401n],
        historical_next_offset: 0n,
        historical_complete: false,
        inline_legacy_topups_complete: false,
        inline_legacy_topup_rows: 1n,
      }).mode,
    ).toBe('unknown');
  });

  it('accepts a genuine between-epoch activated policy with no legacy epoch', () => {
    const policy = normalizeFiatStablePointsPolicy({
      cutover_epoch: [19n],
      legacy_epoch: [],
      active_for_current_epoch: true,
      historical_ledger_cutoff: [401n],
      historical_next_offset: 200n,
      historical_complete: false,
      inline_legacy_topups_complete: true,
      inline_legacy_topup_rows: 0n,
    });
    expect(policy.mode).toBe('flat4x');
    expect(policy.migration.status).toBe('in_progress');
  });
});
