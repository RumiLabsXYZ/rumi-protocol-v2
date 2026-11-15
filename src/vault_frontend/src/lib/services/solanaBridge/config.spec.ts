import { readFileSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { loadDestinations } from '../baseBridge/destinationLoader';
import { buildDestinations, findDestination, isEnabled, pickSelected, solanaDestination, unreachableDestinations, withSolanaDestination } from '../baseBridge/destinations';
import { resolveBridgeConfig } from '../baseBridge/chains';
import { makeBridgeInfo } from '../baseBridge/testSupport';
import { parseProfile } from '../baseBridge/profiles';
import { base58Encode, pubkeyFromBase58 } from './bytes';
import { resolveSolanaConfig } from './config';
import { SOLANA_MAINNET, SOLANA_NETWORKS, solanaNetworkByGenesis } from './networks';
import { MEMO_PROGRAM_V3_B58, TOKEN_2022_PROGRAM_B58, TOKEN_PROGRAM_B58 } from './programs';
import { KEYS, SOL_BRIDGE_ID, makeSolBootstrap, makeSolBridgeInfo, makeSolanaConfig, returnAtaOf } from './testSupport';
import { validateSolanaRecipient } from './icpToSolana';
import { getSolanaBridgeCanisterId, loadSolanaDestination } from './solanaActor';

vi.mock('$lib/config', () => ({
  CANISTER_IDS: { ICUSD_BRIDGE: 'tfesu-vyaaa-aaaap-qrd7a-cai' },
  CONFIG: { isLocal: false, host: 'https://icp0.io', currentIcusdLedgerId: 't6bor-paaaa-aaaap-qrd5q-cai' },
}));

describe('the network table follows the destination profile', () => {
  it('has the same networks and genesis hashes as src/rumi_bridge_destinations/profiles/solana.json', () => {
    const file = path.resolve(__dirname, '../../../../../rumi_bridge_destinations/profiles/solana.json');
    const profile = parseProfile(JSON.parse(readFileSync(file, 'utf8')));
    expect(profile.family).toBe('solana');
    expect(SOLANA_NETWORKS.map((n) => [n.networkId, n.genesisHash])).toEqual(profile.networks.map((n) => [n.networkId, n.identity.value]));
    // Every native profile network stays disabled: nothing here enables a destination.
    expect(profile.networks.every((n) => n.status === 'disabled' && n.instanceRef === null)).toBe(true);
    expect(profile.networks.every((n) => n.identity.kind === 'solana_genesis_hash')).toBe(true);
  });
});

describe('resolving the instance configuration', () => {
  it('resolves an initialized, bound instance into an enabled native config (Token-2022)', () => {
    const result = resolveSolanaConfig(makeSolBridgeInfo(), makeSolBootstrap(), SOL_BRIDGE_ID);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    const c = result.config;
    expect(c.family).toBe('solana');
    expect(c.network).toBe(SOLANA_MAINNET);
    expect([c.tokenProgram, c.tokenProgramId]).toEqual(['token2022', TOKEN_2022_PROGRAM_B58]);
    expect(c.mint).toBe(KEYS.mint);
    expect(c.returnAccount).toBe(returnAtaOf('token2022'));
    expect([c.minDepositE8s, c.maxDepositE8s, c.depositFeeE8s, c.releaseFeeE8s]).toEqual([100_000_000n, 1_000_000_000n, 10_000n, 20_000n]);
    expect([c.depositsPaused, c.mintsPaused, c.releasesPaused, c.halted]).toEqual([false, false, false, null]);
    expect(c.icusdLedger).toBe('t6bor-paaaa-aaaap-qrd5q-cai');
  });

  it('selects the token program from the canister, and derives the matching return account', () => {
    const classic = makeSolanaConfig('classic');
    expect([classic.tokenProgram, classic.tokenProgramId]).toEqual(['classic', TOKEN_PROGRAM_B58]);
    expect(classic.returnAccount).toBe(returnAtaOf('classic'));
    expect(classic.returnAccount).not.toBe(returnAtaOf('token2022'));
    // The same bootstrap with the wrong program's account is refused.
    const wrong = resolveSolanaConfig(makeSolBridgeInfo({}, 'classic'), makeSolBootstrap({}, 'token2022'), SOL_BRIDGE_ID);
    expect(wrong).toMatchObject({ ok: false });
    expect(wrong.ok === false && wrong.message).toContain('not the associated token account this app derives');
  });

  it('refuses, with a plain reason, whatever the canister has not set up or this app does not know', () => {
    const cases: Array<[string, ReturnType<typeof resolveSolanaConfig>]> = [
      ['not set up', resolveSolanaConfig(makeSolBridgeInfo({ initialized: false }), makeSolBootstrap(), SOL_BRIDGE_ID)],
      ['network this app does not know', resolveSolanaConfig(makeSolBridgeInfo({ genesis_hash: base58Encode(new Uint8Array(32).fill(5)) }), makeSolBootstrap(), SOL_BRIDGE_ID)],
      ['memo program this app does not recognize', resolveSolanaConfig(makeSolBridgeInfo({ memo_program: TOKEN_PROGRAM_B58 }), makeSolBootstrap(), SOL_BRIDGE_ID)],
      ['no icUSD mint on Solana yet', resolveSolanaConfig(makeSolBridgeInfo(), makeSolBootstrap({ mint_state: { DeployedUnbound: null } }), SOL_BRIDGE_ID)],
      ['mint is missing', resolveSolanaConfig(makeSolBridgeInfo(), makeSolBootstrap({ mint_account: [] }), SOL_BRIDGE_ID)],
      ['return account is missing', resolveSolanaConfig(makeSolBridgeInfo(), makeSolBootstrap({ return_account: [] }), SOL_BRIDGE_ID)],
      ['not a Solana key', resolveSolanaConfig(makeSolBridgeInfo(), makeSolBootstrap({ return_owner: ['not a key'] }), SOL_BRIDGE_ID)],
      ['not the associated token account', resolveSolanaConfig(makeSolBridgeInfo(), makeSolBootstrap({ return_account: [KEYS.holderSource] }), SOL_BRIDGE_ID)],
    ];
    for (const [fragment, result] of cases) {
      expect(result.ok, fragment).toBe(false);
      expect(result.ok === false && result.message, fragment).toContain(fragment);
    }
  });

  it('carries pauses and a halt into the config without turning them into a disabled destination', () => {
    const paused = resolveSolanaConfig(
      makeSolBridgeInfo({ pauses: { mints: true, observer: false, releases: true, deposits: true }, halted: ['stop'] }),
      makeSolBootstrap(),
      SOL_BRIDGE_ID,
    );
    expect(paused.ok).toBe(true);
    if (paused.ok) expect([paused.config.depositsPaused, paused.config.mintsPaused, paused.config.releasesPaused, paused.config.halted]).toEqual([true, true, true, 'stop']);
  });
});

describe('the Solana recipient', () => {
  const config = makeSolanaConfig();

  it('accepts a wallet and shows the Token-2022 account the mint will land in', () => {
    const check = validateSolanaRecipient(KEYS.holder, config);
    expect(check.ok).toBe(true);
    if (check.ok) {
      expect(check.address).toBe(KEYS.holder);
      expect(pubkeyFromBase58(check.tokenAccount)).not.toBeNull();
      expect(check.tokenAccount).not.toBe(validateSolanaRecipient(KEYS.holder, makeSolanaConfig('classic')).ok ? (validateSolanaRecipient(KEYS.holder, makeSolanaConfig('classic')) as { tokenAccount: string }).tokenAccount : '');
    }
  });

  it('refuses what the backend refuses: programs, the mint and every bridge key', () => {
    for (const bad of ['', '   ', 'not a key', 'abc', TOKEN_PROGRAM_B58, TOKEN_2022_PROGRAM_B58, MEMO_PROGRAM_V3_B58, '11111111111111111111111111111111', config.mint, config.feePayer, config.mintAuthority, config.returnOwner, config.returnAccount]) {
      expect(validateSolanaRecipient(bad, config).ok, bad).toBe(false);
    }
    expect(validateSolanaRecipient('', config)).toMatchObject({ code: 'empty' });
    expect(validateSolanaRecipient('abc', config)).toMatchObject({ code: 'format' });
    expect(validateSolanaRecipient(config.mint, config)).toMatchObject({ code: 'reserved' });
  });
});

describe('the destination list', () => {
  const base = () => resolveBridgeConfig(makeBridgeInfo());

  it('shows Solana as a static, DISABLED row with no actor, and never selects it', () => {
    const list = buildDestinations(base(), 'tfesu-vyaaa-aaaap-qrd7a-cai');
    const sol = findDestination(list, 'solana-mainnet')!;
    expect(sol).toMatchObject({ family: 'solana', profileId: 'solana', status: 'disabled', actorCanisterId: null, config: null, native: null, limits: null, fees: null, chainId: SOLANA_MAINNET.genesisHash });
    expect(sol.reason).toBe('Not available yet: there is no icUSD bridge to Solana at the moment.');
    expect(pickSelected(list, 'solana-mainnet')?.id).toBe('solana-mainnet');
    expect(pickSelected(list, null)?.id).toBe('base-mainnet');
    expect(unreachableDestinations('x').some((d) => d.id === 'solana-mainnet' && !isEnabled(d))).toBe(true);
  });

  it('builds an enabled entry only from a passing answer, and a disabled one with the reason otherwise', () => {
    const ok = solanaDestination({ ok: true, config: makeSolanaConfig() }, SOL_BRIDGE_ID);
    expect(ok).toMatchObject({ id: 'solana-mainnet', family: 'solana', status: 'enabled', reason: null, actorCanisterId: SOL_BRIDGE_ID, config: null });
    expect(ok.native?.family === 'solana' ? ok.native.mint : null).toBe(KEYS.mint);
    const refused = solanaDestination(resolveSolanaConfig(makeSolBridgeInfo({ initialized: false }), makeSolBootstrap(), SOL_BRIDGE_ID), SOL_BRIDGE_ID);
    expect(refused).toMatchObject({ status: 'disabled', native: null });
    expect(refused.reason).toContain('not set up');
  });

  it('puts an enabled entry in place of the static row of the same network, or after the others', () => {
    const list = buildDestinations(base(), 'x');
    const enabled = solanaDestination({ ok: true, config: makeSolanaConfig() }, SOL_BRIDGE_ID);
    const replaced = withSolanaDestination(list, enabled);
    expect(replaced.map((d) => d.id)).toEqual(list.map((d) => d.id));
    expect(findDestination(replaced, 'solana-mainnet')?.status).toBe('enabled');
    const devnet = solanaDestination({ ok: false, message: 'x', network: solanaNetworkByGenesis('EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG') }, SOL_BRIDGE_ID);
    expect(withSolanaDestination(list, devnet).map((d) => d.id)).toEqual([...list.map((d) => d.id), 'solana-devnet']);
  });

  it('makes no Solana request at all while no instance is configured', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    expect(getSolanaBridgeCanisterId()).toBeNull();
    expect(await loadSolanaDestination()).toBeNull();
    // The default loader reads only Base and leaves the static, disabled Solana row in place.
    const list = await loadDestinations(async () => base());
    expect(findDestination(list, 'solana-mainnet')).toMatchObject({ status: 'disabled', native: null, actorCanisterId: null });
    expect(fetchSpy).not.toHaveBeenCalled();
    fetchSpy.mockRestore();
  });

  it('loads an injected enabled instance, keeps Base untouched, and survives a failing Solana read', async () => {
    const enabled = solanaDestination({ ok: true, config: makeSolanaConfig() }, SOL_BRIDGE_ID);
    const list = await loadDestinations(async () => base(), async () => enabled);
    expect(findDestination(list, 'solana-mainnet')?.status).toBe('enabled');
    expect(findDestination(list, 'base-mainnet')?.status).toBe('enabled');
    const failing = await loadDestinations(async () => base(), async () => {
      throw new Error('replica offline');
    });
    expect(findDestination(failing, 'solana-mainnet')).toMatchObject({ status: 'disabled' });
    expect(findDestination(failing, 'base-mainnet')?.status).toBe('enabled');
  });
});
