/**
 * Real-world dependency builders for the Sui flows: the ICP wallet's actors, the anonymous reads a quote needs, the keyless
 * node adapter and the dependency bags the forms pass to the flows. Kept apart from `suiActor.ts` (which the destination
 * loader imports) so reading the destination list never pulls in a wallet store or a ledger-fee service.
 */

import { Actor } from '@dfinity/agent';
import { Principal } from '@dfinity/principal';
import { CONFIG } from '$lib/config';
import { idlFactory as suiIdl } from '$declarations/rumi_icusd_sui_bridge/rumi_icusd_sui_bridge.did.js';
import type { _SERVICE as SuiService } from '$declarations/rumi_icusd_sui_bridge/rumi_icusd_sui_bridge.did';
import type { _SERVICE as LedgerService } from '$declarations/icusd_ledger/icusd_ledger.did';
import { walletStore } from '$lib/stores/wallet';
import { fetchLedgerFeeStrict } from '$lib/services/ledgerFeeService';
import { getAnonymousAgent } from '../solanaBridge/solanaActor';
import { resolveSuiConfig, type SuiBridgeConfig } from './config';
import { SUI_NODE_ENDPOINTS } from './endpoints';
import type { IcpToBaseConfig, PrepareDeps, SuiDepositDeps } from './icpToSui';
import { resolveSuiRpc, type SuiRpc } from './rpc';
import type { ReturnDeps } from './suiToIcp';
import type { SuiIntentStore } from './intents';
import type { SuiWalletSession } from './walletStandard';
import { getAnonymousSuiBridge, getSuiBridgeCanisterId } from './suiActor';

async function getAnonymousIcusdLedger(): Promise<LedgerService> {
  return Actor.createActor<LedgerService>(CONFIG.icusd_ledgerIDL as never, { agent: await getAnonymousAgent(), canisterId: CONFIG.currentIcusdLedgerId });
}

export interface SuiSigners {
  bridge: SuiService;
  ledger: LedgerService;
}

/** The ICP wallet's actors. Create them before the click, not inside it. */
export async function createSuiSigners(): Promise<SuiSigners> {
  const id = getSuiBridgeCanisterId();
  if (!id) throw new Error('No Sui bridge instance is configured.');
  const [bridge, ledger] = await Promise.all([
    walletStore.getActor(id, suiIdl) as Promise<SuiService>,
    walletStore.getActor(CONFIG.currentIcusdLedgerId, CONFIG.icusd_ledgerIDL) as Promise<LedgerService>,
  ]);
  return { bridge, ledger };
}

/**
 * The keyless Sui node adapter for a network, from the deployment-time endpoint list (`endpoints.ts`, empty in every build
 * today). Null when no endpoint is configured for this network and chain identifier: a Sui to ICP return then cannot be
 * prepared, and the page says so. A configured endpoint is served by the real SDK-backed adapter (`createGraphqlSuiRpc`).
 */
export function getSuiRpc(network: SuiBridgeConfig['network']): SuiRpc | null {
  return resolveSuiRpc(network, SUI_NODE_ENDPOINTS);
}

/** Anonymous reads used to build a quote. Nothing here opens a wallet prompt. */
export function buildSuiPrepareDeps(owner: Principal, config: SuiBridgeConfig): PrepareDeps {
  const bridgeId = Principal.fromText(config.instanceCanisterId);
  return {
    async fetchConfig(): Promise<IcpToBaseConfig> {
      const bridge = await getAnonymousSuiBridge();
      const [cfg, roles, lane, observed] = await Promise.all([bridge.get_config(), bridge.get_roles(), bridge.get_mint_lane(), bridge.get_observed_deployment()]);
      const fresh = resolveSuiConfig(cfg, roles, lane, observed[0] ?? null, config.instanceCanisterId);
      if (!fresh.ok) throw new Error(fresh.message);
      if (fresh.config.icusdLedger !== CONFIG.currentIcusdLedgerId) {
        throw new Error('The bridge escrows a different icUSD ledger than this app uses, so an approval would not apply to it.');
      }
      const c = fresh.config;
      return {
        depositFeeE8s: c.depositFeeE8s,
        bridgeLedgerFeeE8s: null,
        limits: { minDepositE8s: c.minDepositE8s, maxDepositE8s: c.maxDepositE8s, depositsPaused: c.depositsPaused, halted: c.halted },
      };
    },
    fetchLedgerFee: () => fetchLedgerFeeStrict({ ledgerId: CONFIG.currentIcusdLedgerId, decimals: 8, symbol: 'icUSD' }),
    async readAllowance() {
      const ledger = await getAnonymousIcusdLedger();
      const result = await ledger.icrc2_allowance({ account: { owner, subaccount: [] }, spender: { owner: bridgeId, subaccount: [] } });
      return { allowance: result.allowance, expiresAtNs: result.expires_at[0] ?? null };
    },
    async readBalance() {
      const ledger = await getAnonymousIcusdLedger();
      return ledger.icrc1_balance_of({ owner, subaccount: [] });
    },
    now: () => Date.now(),
  };
}

/** The dependencies of an ICP to Sui action, without the destination (attached by the caller with `withSigning`). */
export function buildIcpToSuiDeps(opts: {
  config: SuiBridgeConfig;
  principal: Principal;
  store: SuiIntentStore;
  signers: SuiSigners;
  anonymous: SuiService;
  separateApproval: boolean;
  isCurrent: () => boolean;
}): Omit<SuiDepositDeps, 'signing'> {
  const { signers, anonymous } = opts;
  return {
    config: opts.config,
    owner: opts.principal,
    store: opts.store,
    bridgeCanister: Principal.fromText(opts.config.instanceCanisterId),
    ledger: { approve: (args) => signers.ledger.icrc2_approve(args) },
    bridge: {
      bridgeToSui: (args) => signers.bridge.bridge_to_sui(args),
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
    separateApproval: opts.separateApproval,
    isCurrent: opts.isCurrent,
  };
}

/** The dependencies of a Sui to ICP action, without the destination. */
export function buildSuiToIcpDeps(opts: {
  config: SuiBridgeConfig;
  store: SuiIntentStore;
  rpc: SuiRpc;
  wallet: () => SuiWalletSession | null;
  signers: SuiSigners | null;
  anonymous: SuiService;
  isCurrent: () => boolean;
}): Omit<ReturnDeps, 'signing'> {
  const { signers, anonymous } = opts;
  return {
    config: opts.config,
    store: opts.store,
    rpc: opts.rpc,
    wallet: opts.wallet,
    reads: {
      async getRelease(transferId) {
        return (await anonymous.get_release(transferId))[0] ?? null;
      },
      async getCandidate(sender, nonce) {
        const wanted = BigInt(nonce);
        return (await anonymous.get_observer()).candidates.find((c) => c.sui_sender === sender && c.client_nonce === wanted) ?? null;
      },
    },
    bridge: signers ? { notifySuiReturn: (digest) => signers.bridge.notify_sui_return(digest) } : undefined,
    isCurrent: opts.isCurrent,
  };
}
