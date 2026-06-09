import { describe, it, expect } from 'vitest';
import { Principal } from '@dfinity/principal';
import {
  formatPoints,
  qualifyingActionLabel,
  seasonState,
  bodyState,
  deriveRank,
  hasNextPage,
} from './points';
import type {
  PublicEpochStatus,
  PointsConfig,
  PrincipalState,
  LeaderboardEntry,
} from '$declarations/rumi_points/rumi_points.did';

const P1 = Principal.fromText('aaaaa-aa');
const P2 = Principal.fromText('2vxsx-fae');

function status(open: boolean, driver: boolean): PublicEpochStatus {
  return {
    open_epoch: open
      ? [{ epoch_index: 0n, epoch_start_ns: 0n, snapshot_a_ns: 0n, snapshot_b_ns: 0n, epoch_end_ns: 0n }]
      : [],
    snapshot_seed_committed: true,
    driver_interval_secs: 604800n,
    revealed_seed_count: 0n,
    current_epoch_index: 0n,
    driver_enabled: driver,
  };
}
function cfg(endNs: bigint): PointsConfig {
  return {
    admin: P1,
    registered_count: 10n,
    snapshot_seed_committed: true,
    excluded_count: 9,
    season_start_ns: 1_780_272_000_000_000_000n,
    season_end_ns: endNs,
    current_epoch_index: 0n,
  };
}

describe('formatPoints', () => {
  it('renders usd_e8s-days as USD-days', () => {
    // $100 held 1 day = 100 * 1e8 * 1 = 1e10 raw -> "100"
    expect(formatPoints(10_000_000_000n)).toBe('100');
  });
  it('keeps two decimals', () => {
    // 123.45 USD-days = 12_345_000_000 raw
    expect(formatPoints(12_345_000_000n)).toBe('123.45');
  });
  it('groups thousands', () => {
    expect(formatPoints(1_234_567n * 100_000_000n)).toBe('1,234,567');
  });
  it('is zero for empty', () => {
    expect(formatPoints(0n)).toBe('0');
  });
});

describe('qualifyingActionLabel', () => {
  it('maps each variant', () => {
    expect(qualifyingActionLabel({ MintIcUsd: null })).toBe('Minted icUSD');
    expect(qualifyingActionLabel({ DepositStabilityPool: null })).toBe('Deposited to the stability pool');
    expect(qualifyingActionLabel({ Deposit3Pool: null })).toBe('Added 3pool liquidity');
    expect(qualifyingActionLabel({ ProvideAmmLiquidity: null })).toBe('Provided AMM liquidity');
    expect(qualifyingActionLabel({ RepayVault: null })).toBe('Repaid a vault');
  });
});

describe('seasonState', () => {
  const now = 1_784_000_000_000_000_000n;
  it('unknown when data missing', () => {
    expect(seasonState(null, null, now)).toBe('unknown');
  });
  it('ended when now >= season_end', () => {
    expect(seasonState(status(false, false), cfg(now), now)).toBe('ended');
  });
  it('live when an epoch is open', () => {
    expect(seasonState(status(true, true), cfg(now + 1n), now)).toBe('live');
  });
  it('live when driver enabled between epochs', () => {
    expect(seasonState(status(false, true), cfg(now + 1n), now)).toBe('live');
  });
  it('pre when not started and not ended', () => {
    expect(seasonState(status(false, false), cfg(now + 1n), now)).toBe('pre');
  });
});

describe('bodyState', () => {
  const ps = { principal: P1, total_points: 1n } as unknown as PrincipalState;
  it('disconnected', () => {
    expect(bodyState({ connected: false, excluded: false, state: null })).toBe('disconnected');
  });
  it('excluded wins over state', () => {
    expect(bodyState({ connected: true, excluded: true, state: ps })).toBe('excluded');
  });
  it('enrolled when state present', () => {
    expect(bodyState({ connected: true, excluded: false, state: ps })).toBe('enrolled');
  });
  it('not_enrolled when connected without state', () => {
    expect(bodyState({ connected: true, excluded: false, state: null })).toBe('not_enrolled');
  });
});

describe('deriveRank', () => {
  const rows: LeaderboardEntry[] = [
    { principal: P1, total_points: 5n, rank: 1, estimated_share_bps: 0 },
    { principal: P2, total_points: 3n, rank: 2, estimated_share_bps: 0 },
  ];
  it('finds a present principal', () => {
    expect(deriveRank(rows, P2.toText())).toBe(2);
  });
  it('null when principal absent from the slice', () => {
    const absent = Principal.fromText('rdmx6-jaaaa-aaaaa-aaadq-cai');
    expect(deriveRank(rows, absent.toText())).toBeNull();
    expect(deriveRank([], P1.toText())).toBeNull();
  });
});

describe('hasNextPage', () => {
  it('no next on a partial page', () => {
    expect(hasNextPage(0, 30, 50, 1000)).toBe(false);
  });
  it('no next when total reached exactly on a full page', () => {
    expect(hasNextPage(50, 50, 50, 100)).toBe(false);
  });
  it('next when full page and below total', () => {
    expect(hasNextPage(0, 50, 50, 1000)).toBe(true);
  });
  it('full page with unknown total allows next', () => {
    expect(hasNextPage(0, 50, 50, null)).toBe(true);
  });
});
