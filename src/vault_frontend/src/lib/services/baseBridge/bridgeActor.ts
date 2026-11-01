/**
 * Glue between the pure bridge flows and the real world: actors, the wallet
 * session guard and the anonymous read paths.
 *
 * Every read is a public query made with an anonymous actor, so it never opens
 * a signer prompt and behaves the same for every wallet type, Oisy included
 * (`get_bridge_info`, `get_deposit`, `get_deposit_by_request`, `get_release`,
 * `list_deposits_for_owner`, `list_releases_for_owner`,
 * `list_releases_for_base_sender`, `icrc2_allowance`, `icrc1_balance_of`). Only the
 * calls that change state (`bridge_to_base`, `request_deposit_refund`,
 * `notify_base_bridge_out`, `icrc2_approve`) use the wallet's actor. Those
 * actors are created BEFORE the click (see `createSigners`), so the click
 * handler can issue its first signer call without any network await.
 */

import { Actor, AnonymousIdentity, HttpAgent } from '@dfinity/agent';
import { Principal } from '@dfinity/principal';
import { get } from 'svelte/store';
import { CANISTER_IDS, CONFIG } from '$lib/config';
import { idlFactory as bridgeIdl } from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did.js';
import type { _SERVICE as BridgeService } from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
import type { _SERVICE as LedgerService } from '$declarations/icusd_ledger/icusd_ledger.did';
import { walletSessionGeneration } from '$lib/services/auth';
import { principal as principalStore, walletStore } from '$lib/stores/wallet';
import { isOisyWallet } from '$lib/services/protocol/walletOperations';
import { fetchLedgerFeeStrict } from '$lib/services/ledgerFeeService';
import { resolveBridgeConfig, type BridgeConfig, type BridgeConfigResult } from './chains';
import type { BaseToIcpDeps } from './baseToIcp';
import type { IcpToBaseConfig, IcpToBaseDeps, PrepareDeps } from './icpToBase';
import type { IntentStore } from './intents';
import type { Eip1193Provider } from './evmWallet';

/**
 * Popup-signer wallets (Oisy) open their popup only for the first signer call of
 * a click, so approve and bridge_to_base need two clicks. Delegation wallets
 * (Internet Identity, Plug) sign without a popup and keep the single click path.
 * Oisy is the only popup-signer wallet in this app's configuration.
 */
export function needsSeparateApprovalClick(): boolean {
  return isOisyWallet();
}

export function getBridgeCanisterId(): string {
  return CANISTER_IDS.ICUSD_BRIDGE as string;
}

let anonymousAgent: HttpAgent | null = null;
async function getAnonymousAgent(): Promise<HttpAgent> {
  if (!anonymousAgent) {
    anonymousAgent = new HttpAgent({ host: CONFIG.host, identity: new AnonymousIdentity() });
    if (CONFIG.isLocal) await anonymousAgent.fetchRootKey();
  }
  return anonymousAgent;
}

export async function getAnonymousBridge(): Promise<BridgeService> {
  return Actor.createActor<BridgeService>(bridgeIdl, {
    agent: await getAnonymousAgent(),
    canisterId: getBridgeCanisterId(),
  });
}

export async function getAnonymousIcusdLedger(): Promise<LedgerService> {
  return Actor.createActor<LedgerService>(CONFIG.icusd_ledgerIDL as never, {
    agent: await getAnonymousAgent(),
    canisterId: CONFIG.currentIcusdLedgerId,
  });
}

/** The wallet's actors. Create them before the click, not inside it. */
export interface Signers {
  bridge: BridgeService;
  ledger: LedgerService;
}

export async function createSigners(): Promise<Signers> {
  const [bridge, ledger] = await Promise.all([
    walletStore.getActor(getBridgeCanisterId(), bridgeIdl) as Promise<BridgeService>,
    walletStore.getActor(CONFIG.currentIcusdLedgerId, CONFIG.icusd_ledgerIDL) as Promise<LedgerService>,
  ]);
  return { bridge, ledger };
}

/**
 * Liveness check for one action: true only while the same wallet session and
 * the same signed-in principal that were current when the action began are
 * still current. Capture it at the start of the action (synchronously).
 */
export function captureSessionGuard(): () => boolean {
  const generation = get(walletSessionGeneration);
  const principalText = get(principalStore)?.toText() ?? null;
  return () =>
    principalText !== null &&
    get(walletSessionGeneration) === generation &&
    (get(principalStore)?.toText() ?? null) === principalText;
}

/** Fetch and resolve the live bridge configuration from `get_bridge_info()`. */
export async function loadBridgeConfig(): Promise<BridgeConfigResult> {
  const bridge = await getAnonymousBridge();
  return resolveBridgeConfig(await bridge.get_bridge_info());
}

function toIcpToBaseConfig(config: BridgeConfig): IcpToBaseConfig {
  return {
    depositFeeE8s: config.depositFeeE8s,
    bridgeLedgerFeeE8s: config.ledgerFeeE8s,
    limits: {
      minDepositE8s: config.minDepositE8s,
      maxDepositE8s: config.maxDepositE8s,
      depositsPaused: config.depositsPaused,
      halted: config.halted,
    },
  };
}

/** Anonymous reads used to build a quote. Nothing here opens a wallet prompt. */
export function buildPrepareDeps(owner: Principal): PrepareDeps {
  const bridgeId = Principal.fromText(getBridgeCanisterId());
  return {
    async fetchConfig() {
      const result = await loadBridgeConfig();
      if (!result.ok) throw new Error(result.message);
      if (result.config.icusdLedger !== CONFIG.currentIcusdLedgerId) {
        throw new Error('The bridge escrows a different icUSD ledger than this app uses, so an approval would not apply to it.');
      }
      return toIcpToBaseConfig(result.config);
    },
    fetchLedgerFee: () => fetchLedgerFeeStrict({ ledgerId: CONFIG.currentIcusdLedgerId, decimals: 8, symbol: 'icUSD' }),
    async readAllowance() {
      const ledger = await getAnonymousIcusdLedger();
      const result = await ledger.icrc2_allowance({
        account: { owner, subaccount: [] },
        spender: { owner: bridgeId, subaccount: [] },
      });
      return { allowance: result.allowance, expiresAtNs: result.expires_at[0] ?? null };
    },
    async readBalance() {
      const ledger = await getAnonymousIcusdLedger();
      return ledger.icrc1_balance_of({ owner, subaccount: [] });
    },
    now: () => Date.now(),
  };
}

/**
 * The dependencies of an ICP to Base action, without the destination. The caller attaches the
 * destination with `withSigning` (see `signingGuard.ts`), so a flow can never be started without one.
 */
export function buildIcpToBaseDeps(opts: {
  principal: Principal;
  store: IntentStore;
  signers: Signers;
  anonymous: BridgeService;
  isCurrent: () => boolean;
}): Omit<IcpToBaseDeps, 'signing'> {
  const { signers, anonymous } = opts;
  return {
    owner: opts.principal,
    store: opts.store,
    bridgeCanister: Principal.fromText(getBridgeCanisterId()),
    ledger: { approve: (args) => signers.ledger.icrc2_approve(args) },
    bridge: {
      bridgeToBase: (args) => signers.bridge.bridge_to_base(args),
      requestDepositRefund: (id) => signers.bridge.request_deposit_refund(id),
    },
    reads: {
      async getDeposit(id) {
        return (await anonymous.get_deposit(id))[0] ?? null;
      },
      async getDepositByRequest(owner, clientRequestId) {
        return (await anonymous.get_deposit_by_request(owner, clientRequestId))[0] ?? null;
      },
    },
    separateApproval: needsSeparateApprovalClick(),
    isCurrent: opts.isCurrent,
  };
}

/** Like `buildIcpToBaseDeps`: the destination is attached by the caller with `withSigning`. */
export function buildBaseToIcpDeps(opts: {
  provider: Eip1193Provider;
  config: BridgeConfig;
  account: string;
  principal: Principal;
  store: IntentStore;
  signers: Signers | null;
  anonymous: BridgeService;
  isCurrent: () => boolean;
}): Omit<BaseToIcpDeps, 'signing'> {
  const { signers, anonymous } = opts;
  return {
    provider: opts.provider,
    chain: opts.config.chain,
    contract: opts.config.contract,
    account: opts.account,
    icpPrincipal: opts.principal,
    store: opts.store,
    reads: {
      async getRelease(transferId) {
        return (await anonymous.get_release(transferId))[0] ?? null;
      },
    },
    bridge: signers ? { notifyBaseBridgeOut: (txHash) => signers.bridge.notify_base_bridge_out(txHash) } : undefined,
    isCurrent: opts.isCurrent,
  };
}
