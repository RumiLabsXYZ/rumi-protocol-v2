/**
 * Direct Oisy signer integration using @slide-computer/signer v4.
 *
 * PNP bundles @slide-computer/signer-agent v3.20.0 internally but doesn't
 * expose the SignerAgent. v3's agent also pre-checks ICRC-21 on target
 * canisters, which fails for canisters that don't implement it.
 *
 * This module creates a v4 SignerAgent that delegates consent to the Oisy
 * signer (Tier 1 for known ICRC methods, Tier 3 blind request for custom
 * methods). This lets us call custom canister methods (stability pool
 * deposit, withdraw, etc.) through Oisy without requiring ICRC-21.
 *
 * Usage:
 *   import { getOisySignerAgent, createOisyActor, clearOisySigner } from './oisySigner';
 *
 *   const signerAgent = await getOisySignerAgent(principal);
 *   const actor = createOisyActor(canisterId, idlFactory, signerAgent);
 *
 *   signerAgent.batch();
 *   actor.icrc2_approve(...);
 *   signerAgent.batch();
 *   actor.deposit(...);
 *   await signerAgent.execute();
 */

import { Signer } from '@slide-computer/signer';
import { SignerAgent } from '@slide-computer/signer-agent';
import { PostMessageTransport } from '@slide-computer/signer-web';
import { Actor } from '@dfinity/agent';
import type { Principal } from '@dfinity/principal';

// Module-level Signer — lightweight, no popup until first signing request.
// windowOpenerFeatures opens Oisy as a popup instead of a new tab.
const oisySigner = new Signer({
  transport: new PostMessageTransport({
    url: 'https://oisy.com/sign',
    windowOpenerFeatures: 'toolbar=0,location=0,menubar=0,width=525,height=705',
  }),
});

let cachedAgent: any = null;
let cachedPrincipalText: string | null = null;

/**
 * Get or create a v4 SignerAgent for the given principal.
 * The SignerAgent is cached and reused across mutations.
 * Creating it does NOT open a popup — the popup opens on execute().
 */
export async function getOisySignerAgent(principal: Principal): Promise<any> {
  const principalText = principal.toText();

  // Reuse cached agent if principal hasn't changed
  if (cachedAgent && cachedPrincipalText === principalText) {
    return cachedAgent;
  }

  // Let SignerAgent create its own @icp-sdk/core HttpAgent internally.
  // Passing @dfinity/agent's HttpAgent causes a rootKey type mismatch:
  // @dfinity returns ArrayBuffer but @icp-sdk/core expects Uint8Array,
  // which breaks certificate validation after signing.
  cachedAgent = await SignerAgent.create({
    signer: oisySigner as any,
    account: principal as any,
  });

  cachedPrincipalText = principalText;
  return cachedAgent;
}

/**
 * Create an actor that routes calls through the Oisy v4 SignerAgent.
 * This actor supports batch()/execute() for ICRC-112 batched signing.
 */
export function createOisyActor(
  canisterId: string,
  idlFactory: any,
  signerAgent: any,
): any {
  return Actor.createActor(idlFactory, {
    agent: signerAgent,
    canisterId,
  });
}

/**
 * Clear cached signer agent (call on disconnect).
 */
export function clearOisySigner(): void {
  cachedAgent = null;
  cachedPrincipalText = null;
}
