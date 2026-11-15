import { readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { EVM_DESTINATION_CHAINS, baseNetworkId } from './chains';
import { unavailableEvmDestinations } from './destinations';
import {
  ProfileError,
  U64_MAX_DECIMAL,
  findEvmNetwork,
  findNetwork,
  isCanonicalU64,
  parseProfile,
  parseProfiles,
  type DestinationProfile,
} from './profiles';

// The REAL profile files, read from disk (src/rumi_bridge_destinations/profiles). Nothing is copied into the specs.
const PROFILES_DIR = `${resolve(__dirname, '../../../../../rumi_bridge_destinations/profiles')}/`;
const FILES = readdirSync(PROFILES_DIR)
  .filter((name) => name.endsWith('.json'))
  .sort();
const RAW: Record<string, any> = Object.fromEntries(FILES.map((name) => [name.replace(/\.json$/, ''), JSON.parse(readFileSync(PROFILES_DIR + name, 'utf-8'))]));
const RAW_TEXT: Record<string, string> = Object.fromEntries(FILES.map((name) => [name.replace(/\.json$/, ''), readFileSync(PROFILES_DIR + name, 'utf-8')]));

const NINE = ['arbitrum', 'base', 'ethereum', 'hyperevm', 'monad', 'robinhood', 'solana', 'sui', 'xrpl'];
const NETWORK_IDS = [
  'arbitrum-mainnet', 'arbitrum-sepolia',
  'base-mainnet', 'base-sepolia',
  'ethereum-mainnet', 'ethereum-sepolia',
  'hyperevm-mainnet', 'hyperevm-testnet',
  'monad-mainnet', 'monad-testnet',
  'robinhood-mainnet', 'robinhood-testnet',
  'solana-mainnet', 'solana-testnet', 'solana-devnet',
  'sui-mainnet', 'sui-testnet',
  'xrpl-mainnet', 'xrpl-testnet',
];
const NON_BASE_EVM = ['arbitrum', 'ethereum', 'hyperevm', 'monad', 'robinhood'];

const profiles: DestinationProfile[] = parseProfiles(FILES.map((name) => RAW[name.replace(/\.json$/, '')]));
const byId = (id: string) => profiles.find((profile) => profile.id === id)!;
const allNetworks = profiles.flatMap((profile) => profile.networks.map((network) => ({ profile, network })));

/** A deep copy of one raw profile, to break one thing at a time. */
function broken(id: string, change: (raw: any) => void): unknown {
  const copy = structuredClone(RAW[id]);
  change(copy);
  return copy;
}

describe('the real profile files', () => {
  it('are the nine profiles, 19 networks', () => {
    expect(FILES).toEqual(NINE.map((id) => `${id}.json`));
    expect(profiles.map((profile) => profile.id).sort()).toEqual(NINE);
    expect(allNetworks).toHaveLength(19);
    expect(allNetworks.map(({ network }) => network.networkId).sort()).toEqual([...NETWORK_IDS].sort());
  });

  it('are all disabled with no instance: groundwork data enables nothing', () => {
    for (const { network } of allNetworks) {
      expect(network.status, network.networkId).toBe('disabled');
      expect(network.instanceRef, network.networkId).toBeNull();
    }
  });

  it('give every network a per-network id that no other network shares, with its role', () => {
    const roles = Object.fromEntries(allNetworks.map(({ network }) => [network.networkId, network.role]));
    expect(roles['base-mainnet']).toBe('mainnet');
    expect(roles['base-sepolia']).toBe('testnet');
    expect(roles['solana-devnet']).toBe('devnet');
    expect(new Set(allNetworks.map(({ network }) => network.networkId)).size).toBe(19);
    // A mainnet and a testnet of one profile never share an id.
    for (const profile of profiles) expect(new Set(profile.networks.map((network) => network.networkId)).size).toBe(profile.networks.length);
  });

  it('keep every chain identity a string', () => {
    for (const { network } of allNetworks) {
      expect(typeof network.identity.value, network.networkId).toBe('string');
      expect(network.identity.value.length).toBeGreaterThan(0);
    }
    expect(byId('base').networks.map((network) => network.identity)).toEqual([
      { kind: 'evm_chain_id', value: '8453' },
      { kind: 'evm_chain_id', value: '84532' },
    ]);
    expect(byId('xrpl').networks.map((network) => network.identity.value)).toEqual(['0', '1']);
    expect(byId('ethereum').networks[1].identity.value).toBe('11155111');
  });

  it('keep every amount a canonical decimal string, exactly as written', () => {
    for (const profile of profiles) {
      const { minE8s, maxE8s, maxOutstandingE8s } = profile.amountBounds;
      expect(isCanonicalU64(minE8s), `${profile.id} min`).toBe(true);
      expect(isCanonicalU64(maxE8s), `${profile.id} max`).toBe(true);
      if (maxOutstandingE8s !== null) expect(isCanonicalU64(maxOutstandingE8s), `${profile.id} outstanding`).toBe(true);
      // Same text as the file (the reader copies, it does not convert).
      const raw = RAW[profile.id].representation.amount_bounds.value;
      expect(typeof raw.max_e8s).toBe('string');
      expect(maxE8s).toBe(raw.max_e8s);
      expect(minE8s).toBe(raw.min_e8s);
    }
  });

  it('keep the largest u64 exact, which a JavaScript number cannot', () => {
    expect(byId('base').amountBounds).toMatchObject({ minE8s: '1', maxE8s: U64_MAX_DECIMAL, maxOutstandingE8s: null, encoding: 'u64' });
    expect(U64_MAX_DECIMAL).toBe('18446744073709551615');
    // The reason the strings matter: as a number it is already a different value.
    expect(String(Number(U64_MAX_DECIMAL))).not.toBe(U64_MAX_DECIMAL);
    expect(RAW_TEXT.base).toContain('"max_e8s": "18446744073709551615"');
  });

  it('read each XRPL amount with its own encoding and a total cap', () => {
    const xrpl = byId('xrpl').amountBounds;
    expect(xrpl.encoding).toBe('xrpl_decimal_15');
    expect(typeof xrpl.maxOutstandingE8s).toBe('string');
    expect(typeof xrpl.maxE8s).toBe('string');
  });

  it('read a finality summary for each network', () => {
    for (const { network } of allNetworks) {
      expect(network.finality.typicalWait.length, network.networkId).toBeGreaterThan(0);
    }
    expect(byId('base').networks[0].finality).toMatchObject({ rule: 'evm_tag', tag: 'finalized', typicalWait: 'about 20 minutes (L1 batch finality)' });
    expect(byId('monad').networks.map((network) => [network.finality.rule, network.finality.executionDelayBlocks])).toEqual([
      ['monad_verified', 3],
      ['monad_verified', 3],
    ]);
    expect(byId('solana').networks[0].finality).toMatchObject({ rule: 'solana_commitment', commitment: 'finalized' });
    expect(byId('sui').networks[0].finality).toMatchObject({ rule: 'sui_checkpoint', checkpoint: 'finalized' });
    expect(byId('xrpl').networks[0].finality).toMatchObject({ rule: 'xrpl_validated', ledgerIndex: 'validated', requiredResult: 'tesSUCCESS' });
  });

  it('carry the blockers the groundwork requires, with kinds', () => {
    const ids = (networkId: string) => findNetwork(profiles, networkId)!.network.blockerIds;
    for (const { network } of allNetworks) {
      expect(network.blockerIds, network.networkId).toContain('controller-model-undecided');
      // A test network says so; a mainnet does not.
      expect(network.blockerIds.includes('testnet-synthetic-ledger-only'), network.networkId).toBe(network.role !== 'mainnet');
    }
    for (const networkId of ['monad-mainnet', 'monad-testnet']) expect(ids(networkId)).toContain('monad-verified-head');
    for (const networkId of ['hyperevm-mainnet', 'hyperevm-testnet']) expect(ids(networkId)).toContain('hyperevm-historical-state');
    for (const networkId of ['robinhood-mainnet', 'robinhood-testnet']) {
      expect(ids(networkId)).toEqual(expect.arrayContaining(['robinhood-finalized-tag-unverified', 'single-keyless-endpoint']));
    }
    for (const { network } of allNetworks) {
      for (const blocker of network.blockers) {
        expect(['config', 'code', 'external', 'decision']).toContain(blocker.kind);
        expect(blocker.text).not.toMatch(new RegExp(`[${String.fromCharCode(0x2013, 0x2014)}]`));
      }
    }
  });
});

describe('profile upkeep for the EVM implementation', () => {
  it('retires the blocker that said the Base Wasm accepts only Base', () => {
    for (const { network } of allNetworks) expect(network.blockerIds, network.networkId).not.toContain('evm-wasm-generalization');
  });

  it('replaces it, on every non-Base EVM network, with the activation gate that source support does not clear', () => {
    for (const id of NON_BASE_EVM) {
      for (const network of byId(id).networks) {
        const gate = network.blockers.find((blocker) => blocker.id === 'evm-instance-not-activated');
        expect(gate, network.networkId).toBeDefined();
        expect(gate!.kind).toBe('external');
        expect(gate!.text).toContain('instance_ref is null');
        expect(gate!.text).toMatch(/unfunded/);
        expect(gate!.text).toMatch(/unmeasured/);
        expect(network.status).toBe('disabled');
      }
    }
    // Base keeps its own live-status blocker and is not given this one.
    for (const network of byId('base').networks) expect(network.blockerIds).not.toContain('evm-instance-not-activated');
  });

  it('keeps every activation blocker that source support does not clear', () => {
    const must: Record<string, string[]> = {
      'ethereum-mainnet': ['evm-instance-not-activated', 'ethereum-canonical-bridge-registration', 'controller-model-undecided'],
      'arbitrum-mainnet': ['evm-instance-not-activated', 'arbitrum-l1-poster-fee-buffers', 'controller-model-undecided'],
      'monad-mainnet': [
        'evm-instance-not-activated',
        'monad-verified-head',
        'monad-explicit-gas-limits',
        'monad-reserve-aware-funding',
        'monad-log-range-chunking',
        'monad-token-naming-d8',
        'controller-model-undecided',
      ],
      'robinhood-mainnet': [
        'evm-instance-not-activated',
        'robinhood-finalized-tag-unverified',
        'single-keyless-endpoint',
        'robinhood-push0-arbos-unconfirmed',
        'controller-model-undecided',
      ],
      'hyperevm-mainnet': [
        'evm-instance-not-activated',
        'hyperevm-historical-state',
        'single-keyless-endpoint',
        'hyperevm-finality-semantics-unverified',
        'hyperevm-deploy-gas-small-block',
        'hyperevm-nonce-queue-limit',
        'controller-model-undecided',
      ],
    };
    for (const [networkId, want] of Object.entries(must)) {
      expect(findNetwork(profiles, networkId)!.network.blockerIds, networkId).toEqual(want);
    }
  });

  it('no longer calls source work that is done a code blocker: what remains is measurement, probes and funding', () => {
    const kind = (networkId: string, blockerId: string) => findNetwork(profiles, networkId)!.network.blockers.find((b) => b.id === blockerId)!.kind;
    for (const networkId of ['monad-mainnet', 'monad-testnet']) {
      for (const blockerId of ['monad-verified-head', 'monad-explicit-gas-limits', 'monad-reserve-aware-funding', 'monad-log-range-chunking']) {
        expect(kind(networkId, blockerId), `${networkId} ${blockerId}`).toBe('external');
      }
    }
    expect(kind('arbitrum-mainnet', 'arbitrum-l1-poster-fee-buffers')).toBe('external');
    // These stay open source work: the Monad token labels (D8), the HyperEVM historical state and nonce queue.
    expect(kind('monad-mainnet', 'monad-token-naming-d8')).toBe('code');
    expect(kind('hyperevm-mainnet', 'hyperevm-historical-state')).toBe('code');
    expect(kind('hyperevm-mainnet', 'hyperevm-nonce-queue-limit')).toBe('code');
  });

  it('names the Monad fast-tag consensus evidence as what still blocks the head rule', () => {
    const head = findNetwork(profiles, 'monad-mainnet')!.network.blockers.find((blocker) => blocker.id === 'monad-verified-head')!;
    expect(head.text).toContain('finalized minus 3 blocks');
    expect(head.text).toMatch(/consensus/);
    expect(head.text).toMatch(/probe/);
  });
});

describe('the page tables agree with the real profiles', () => {
  it('every non-Base EVM network has exactly one chain row with the same per-network key, chain id and role', () => {
    const evmProfileNetworks = allNetworks.filter(({ profile }) => NON_BASE_EVM.includes(profile.id));
    expect(evmProfileNetworks).toHaveLength(10);
    expect(EVM_DESTINATION_CHAINS).toHaveLength(10);
    for (const { profile, network } of evmProfileNetworks) {
      const rows = EVM_DESTINATION_CHAINS.filter((row) => String(row.chainId) === network.identity.value);
      expect(rows, network.networkId).toHaveLength(1);
      expect(rows[0].networkId, network.networkId).toBe(network.networkId);
      expect(rows[0].profileId).toBe(profile.id);
      expect(rows[0].testnet).toBe(network.role !== 'mainnet');
    }
  });

  it('the Base keys come from the chain id the bridge reports, and match the Base profile', () => {
    const base = byId('base');
    for (const network of base.networks) {
      expect(baseNetworkId(Number(network.identity.value)), network.networkId).toBe(network.networkId);
    }
  });

  it('the static destination entries are the five non-Base mainnet networks, keyed by their network id', () => {
    const entries = unavailableEvmDestinations();
    const expected = NON_BASE_EVM.map((id) => byId(id).networks.find((network) => network.role === 'mainnet')!)
      .map((network) => ({ id: network.networkId, chainId: network.identity.value }))
      .sort((a, b) => a.id.localeCompare(b.id));
    expect(entries.map((entry) => ({ id: entry.id, chainId: entry.chainId })).sort((a, b) => a.id.localeCompare(b.id))).toEqual(expected);
    for (const entry of entries) {
      expect(entry.status).toBe('disabled');
      expect(NON_BASE_EVM).toContain(entry.profileId);
      expect(findNetwork(profiles, entry.id)?.profile.id).toBe(entry.profileId);
    }
  });

  it('finds a network by its id or by its chain id as text', () => {
    expect(findNetwork(profiles, 'monad-testnet')?.profile.id).toBe('monad');
    expect(findNetwork(profiles, 'nope')).toBeNull();
    expect(findEvmNetwork(profiles, '8453')?.network.networkId).toBe('base-mainnet');
    expect(findEvmNetwork(profiles, '10143')?.network.networkId).toBe('monad-testnet');
    expect(findEvmNetwork(profiles, '0')).toBeNull(); // XRPL network 0 is not an EVM chain
  });
});

describe('the reader refuses what it should', () => {
  const bounds = (raw: any) => raw.representation.amount_bounds.value;

  it('rejects a JSON number where an amount is required, even a safe one', () => {
    expect(() => parseProfile(broken('base', (raw) => (bounds(raw).max_e8s = 18446744073709552000)))).toThrowError(/JSON number/);
    expect(() => parseProfile(broken('base', (raw) => (bounds(raw).min_e8s = 1)))).toThrowError(/JSON number/);
  });

  it('rejects an amount that is not canonical', () => {
    for (const bad of ['', '01', '007', '+5', '-1', ' 5', '5 ', '1.5', '1e3', '0x10', 'abc', '18446744073709551616', '99999999999999999999', '100000000000000000000']) {
      expect(() => parseProfile(broken('base', (raw) => (bounds(raw).max_e8s = bad))), JSON.stringify(bad)).toThrowError(ProfileError);
    }
    expect(() => parseProfile(broken('xrpl', (raw) => (bounds(raw).max_outstanding_e8s = 5)))).toThrowError(/JSON number/);
    expect(() => parseProfile(broken('xrpl', (raw) => (bounds(raw).max_outstanding_e8s = '05')))).toThrowError(ProfileError);
  });

  it('accepts the u64 boundaries and nothing past them', () => {
    for (const ok of ['0', '1', '9', '10', '18446744073709551614', '18446744073709551615']) expect(isCanonicalU64(ok), ok).toBe(true);
    for (const bad of ['18446744073709551616', '18446744073709551620', '00', '-0', '', ' 1', 1, null, undefined]) expect(isCanonicalU64(bad), String(bad)).toBe(false);
  });

  it('rejects a chain identity written as a JSON number, or not as a canonical decimal', () => {
    expect(() => parseProfile(broken('base', (raw) => (raw.networks[0].identity.value.value = 8453)))).toThrowError(/string/);
    expect(() => parseProfile(broken('base', (raw) => (raw.networks[0].identity.value.value = '08453')))).toThrowError(ProfileError);
    expect(() => parseProfile(broken('base', (raw) => (raw.networks[0].identity.value.value = '')))).toThrowError(ProfileError);
  });

  it('rejects a wrong schema version, an unknown vocabulary word, or a missing piece', () => {
    expect(() => parseProfile(broken('base', (raw) => (raw.schema_version = 2)))).toThrowError(/schema_version/);
    expect(() => parseProfile(broken('base', (raw) => (raw.family = 'cosmos')))).toThrowError(/family/);
    expect(() => parseProfile(broken('base', (raw) => (raw.networks[0].status = 'live')))).toThrowError(/status/);
    expect(() => parseProfile(broken('base', (raw) => (raw.networks[0].role = 'prod')))).toThrowError(/role/);
    expect(() => parseProfile(broken('base', (raw) => delete raw.networks[0].network_id))).toThrowError(/network_id/);
    expect(() => parseProfile(broken('base', (raw) => delete raw.networks[0].finality))).toThrowError(/finality/);
    expect(() => parseProfile(broken('base', (raw) => (raw.networks = [])))).toThrowError(/at least one network/);
    expect(() => parseProfile(broken('base', (raw) => (raw.networks[0].activation_blockers[0].kind = 'later')))).toThrowError(/kind/);
    expect(() => parseProfile(null)).toThrowError(ProfileError);
    expect(() => parseProfile([])).toThrowError(ProfileError);
  });

  it('rejects a bad Monad delay count', () => {
    expect(() => parseProfile(broken('monad', (raw) => (raw.networks[0].finality.parameters.value.execution_delay_blocks = 2.5)))).toThrowError(/execution_delay_blocks/);
    expect(() => parseProfile(broken('monad', (raw) => (raw.networks[0].finality.parameters.value.execution_delay_blocks = -1)))).toThrowError(/execution_delay_blocks/);
  });

  it('rejects duplicates: a blocker id, a network id in one profile, a profile id, or a network id shared across profiles', () => {
    expect(() =>
      parseProfile(broken('base', (raw) => raw.networks[0].activation_blockers.push(structuredClone(raw.networks[0].activation_blockers[0])))),
    ).toThrowError(/duplicate blocker id/);
    expect(() => parseProfile(broken('base', (raw) => (raw.networks[1].network_id = raw.networks[0].network_id)))).toThrowError(/duplicate network id/);
    expect(() => parseProfiles([RAW.base, RAW.base])).toThrowError(/duplicate profile id/);
    const clash = structuredClone(RAW.ethereum);
    clash.networks[0].network_id = 'base-mainnet';
    expect(() => parseProfiles([RAW.base, clash])).toThrowError(/network id base-mainnet is used twice/);
  });

  it('names the JSON path of the first problem', () => {
    try {
      parseProfile(broken('base', (raw) => (bounds(raw).max_e8s = 'x')));
      throw new Error('should have thrown');
    } catch (error) {
      expect(error).toBeInstanceOf(ProfileError);
      expect((error as ProfileError).path).toBe('profile base.representation.amount_bounds.value.max_e8s');
    }
  });
});
