# Implementation Plan: Stability Pool Holds 3USD Instead of Redeeming

## Context

When a liquidation occurs and the stability pool is used, the current flow redeems 3USD back through the 3pool to get icUSD, then burns that icUSD to cover the insolvent vault's debt. This pulls liquidity out of the 3pool every time a liquidation fires.

The proposed change: instead of redeeming 3USD and burning icUSD, the protocol transfers the 3USD from the stability pool to the backend canister. The vault's debt is still written off. The 3USD sits in protocol reserves earning yield, and the 3pool liquidity stays intact.

## Why This Is Safe

All circulating icUSD remains fully backed. The insolvent vault's debt is written off and its collateral is seized. The 3USD held in protocol reserves represents a claim on stablecoins (icUSD, ckUSDC, ckUSDT) in the 3pool. No unbacked icUSD enters circulation.

## Critical: Order of Operations

The 3USD transfer MUST happen before any vault state mutations.

1. Read-only checks: verify vault is undercollateralized, verify stability pool has sufficient 3USD. No state changes.
2. Async call: transfer 3USD from stability pool to a designated subaccount on the backend canister: `(backend_principal, hash("protocol_3usd_reserves"))`. Using a dedicated subaccount keeps liquidation-sourced 3USD identifiable at the ledger level without relying on internal state.
3. If transfer fails: stop. Nothing was mutated. Vault is untouched. Stability pool depositor's balance is untouched. Vault goes to the next mechanism in the liquidation cascade (manual liquidation queue).
4. If transfer succeeds: NOW mutate state. Write off the debt, mark vault liquidated, increment `protocol_3usd_reserves`, credit the stability pool depositor with seized collateral.

Step 4 is the commit point. Everything before it is reversible by doing nothing. If the current code mutates vault state before the 3USD movement, it must be reordered.

## What Needs to Change

Find the liquidation execution path where the stability pool's 3USD is redeemed through the 3pool for icUSD and that icUSD is burned. Replace that redeem-and-burn sequence with a simple ICRC-1 transfer of 3USD from the stability pool to the backend subaccount `(backend_principal, hash("protocol_3usd_reserves"))`.

Specifically:

1. **Locate the stability pool liquidation handler.** Find the code path where, after a vault is identified as undercollateralized and the stability pool has sufficient funds, the protocol uses 3USD from the pool to cover the debt. Look for the sequence that calculates how much 3USD is needed, calls the 3pool to redeem for icUSD, and burns the icUSD.
2. **Replace the redeem-and-burn sequence with a transfer.** Transfer the 3USD to the backend subaccount. Debt is still written off in vault state, but no icUSD is burned and no 3pool redemption happens.
3. **Track protocol-held 3USD reserves.** Add `protocol_3usd_reserves: u64` to backend state. Increment on each stability pool liquidation.
