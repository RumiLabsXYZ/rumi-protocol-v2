/**
 * Real-world dependency builders for the Solana flows: the ICP wallet's actors, the anonymous reads a quote needs and the
 * dependency bags the forms pass to the flows. Kept apart from `solanaActor.ts` (which the destination loader imports)
 * so reading the destination list never pulls in a wallet store or a ledger-fee service.
 */

import { Actor } from '@dfinity/agent';
import { Principal } from '@dfinity/principal';
import { CONFIG } from '$lib/config';
import { idlFactory as solIdl } from '$declarations/rumi_icusd_sol_bridge/rumi_icusd_sol_bridge.did.js';
import type { _SERVICE as SolService } from '$declarations/rumi_icusd_sol_bridge/rumi_icusd_sol_bridge.did';
import type { _SERVICE as LedgerService } from '$declarations/icusd_ledger/icusd_ledger.did';
import { walletStore } from '$lib/stores/wallet';
import { fetchLedgerFeeStrict } from '$lib/services/ledgerFeeService';
import { resolveSolanaConfig, type SolanaBridgeConfig } from './config';
import type { IcpToBaseConfig, PrepareDeps, SolDepositDeps } from './icpToSolana';
import type { ReturnDeps } from './solanaToIcp';
import type { SolanaIntentStore } from './intents';
import { resolveSolanaRpc, type SolanaRpc } from './rpc';
import { SOLANA_RPC_ENDPOINTS } from './endpoints';
import type { SolanaWalletSession } from './walletStandard';
import { getAnonymousAgent, getAnonymousSolanaBridge, getSolanaBridgeCanisterId } from './solanaActor';

async function getAnonymousIcusdLedger(): Promise<LedgerService> {
  return Actor.createActor<LedgerService>(CONFIG.icusd_ledgerIDL as never, { agent: await getAnonymousAgent(), canisterId: CONFIG.currentIcusdLedgerId });
}

export interface SolanaSigners {
  bridge: SolService;
  ledger: LedgerService;
}

/** The ICP wallet's actors. Create them before the click, not inside it. */
export async function createSolanaSigners(): Promise<SolanaSigners> {
  const id = getSolanaBridgeCanisterId();
  if (!id) throw new Error('No Solana bridge instance is configured.');
  const [bridge, ledger] = await Promise.all([
    walletStore.getActor(id, solIdl) as Promise<SolService>,
    walletStore.getActor(CONFIG.currentIcusdLedgerId, CONFIG.icusd_ledgerIDL) as Promise<LedgerService>,
  ]);
  return { bridge, ledger };
}

/**
 * The keyless Solana node adapter for a network, from the deployment-time endpoint list (`endpoints.ts`, empty in every
 * build today). Null when no endpoint is configured for this network and genesis: a Solana to ICP return then cannot be
 * prepared, and the page says so. A configured endpoint is served by the real `createFetchSolanaRpc`.
 */
export function getSolanaRpc(network: SolanaBridgeConfig['network']): SolanaRpc | null {
  return resolveSolanaRpc(network, SOLANA_RPC_ENDPOINTS);
}

/** Anonymous reads used to build a quote. Nothing here opens a wallet prompt. */
export function buildSolanaPrepareDeps(owner: Principal, config: SolanaBridgeConfig): PrepareDeps {
  const bridgeId = Principal.fromText(config.instanceCanisterId);
  return {
    async fetchConfig(): Promise<IcpToBaseConfig> {
      const bridge = await getAnonymousSolanaBridge();
      const fresh = resolveSolanaConfig(await bridge.get_bridge_info(), await bridge.get_bootstrap_status(), config.instanceCanisterId);
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

/** The dependencies of an ICP to Solana action, without the destination (attached by the caller with `withSigning`). */
export function buildIcpToSolanaDeps(opts: {
  config: SolanaBridgeConfig;
  principal: Principal;
  store: SolanaIntentStore;
  signers: SolanaSigners;
  anonymous: SolService;
  separateApproval: boolean;
  isCurrent: () => boolean;
}): Omit<SolDepositDeps, 'signing'> {
  const { signers, anonymous } = opts;
  return {
    config: opts.config,
    owner: opts.principal,
    store: opts.store,
    bridgeCanister: Principal.fromText(opts.config.instanceCanisterId),
    ledger: { approve: (args) => signers.ledger.icrc2_approve(args) },
    bridge: {
      bridgeToSolana: (args) => signers.bridge.bridge_to_solana(args),
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

/** The dependencies of a Solana to ICP action, without the destination. */
export function buildSolanaToIcpDeps(opts: {
  config: SolanaBridgeConfig;
  store: SolanaIntentStore;
  rpc: SolanaRpc;
  wallet: () => SolanaWalletSession | null;
  signers: SolanaSigners | null;
  anonymous: SolService;
  isCurrent: () => boolean;
}): Omit<ReturnDeps, 'signing'> {
  const { signers, anonymous } = opts;
  return {
    config: opts.config,
    store: opts.store,
    rpc: opts.rpc,
    wallet: opts.wallet,
    reads: {
      async getReturn(returnId) {
        return (await anonymous.get_return(returnId))[0] ?? null;
      },
      async getRelease(returnId) {
        return (await anonymous.get_release(returnId))[0] ?? null;
      },
    },
    bridge: signers ? { notifySolanaReturn: (signature) => signers.bridge.notify_solana_return(signature) } : undefined,
    isCurrent: opts.isCurrent,
  };
}
