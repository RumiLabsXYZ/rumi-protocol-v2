# Implementation Plan: Stability Pool Holds 3USD Instead of Redeeming

## Context

When a liquidation occurs and the stability pool is used, the current flow redeems 3USD back through the 3pool to get icUSD, then burns that icUSD to cover the insolvent vault's debt. This pulls liquidity out of the 3pool every time a liquidation fires.

The proposed change: instead of redeeming 3USD and burning icUSD, the protocol simply transfers the 3USD from the stability pool to the backend canister (protocol reserves). The vault's debt is still written off. The 3USD sits in protocol reserves earning yield, and the 3pool liquidity stays intact.

## Why This Is Safe

All circulating icUSD remains fully backed. The insolvent vault's debt is written off and its collateral is seized. The 3USD held in protocol reserves represents a claim on stablecoins (icUSD, ckUSDC, ckUSDT) in the 3pool. The original icUSD that was borrowed from the liquidated vault is still backed by: (a) the collateral in the stability pool depositor's own vault (if they minted icUSD to get 3USD), and (b) the fiat-backed stablecoins (ckUSDC, ckUSDT) sitting in the 3pool that the 3USD represents. No unbacked icUSD enters circulation.

## Critical: Order of Operations

The 3USD transfer MUST happen before any vault state mutations. This is what makes the all-or-nothing guarantee possible.

1. Read-only checks: verify vault is undercollateralized, verify stability pool has sufficient 3USD. No state changes.
2. Async call: transfer 3USD from stability pool to backend subaccount `(backend_principal, hash("protocol_3usd_reserves"))`.
3. If transfer fails: stop. Nothing was mutated. Vault is untouched. Stability pool depositor's balance is untouched. Vault goes to manual liquidation queue via the existing cascade.
4. If transfer succeeds: NOW mutate state. Write off the debt, mark vault liquidated, increment `protocol_3usd_reserves`, credit the stability pool depositor with seized collateral.

Step 4 is the commit point. Everything before it is reversible by doing nothing. If the current code mutates vault state before the 3USD movement, it must be reordered.

## What Needs to Change

The change is narrow. Find the liquidation execution path where the stability pool's 3USD is redeemed through the 3pool for icUSD and that icUSD is burned. Replace that sequence with a simple ICRC-1 transfer of 3USD from the stability pool to a designated subaccount on the backend canister, e.g. `(backend_principal, hash("protocol_3usd_reserves"))`. Using a dedicated subaccount rather than the default account keeps liquidation-sourced 3USD identifiable at the ledger level without relying on internal state.

Specifically:

1. **Locate the stability pool liquidation handler.** This is the code path where, after a vault is identified as undercollateralized and the stability pool has sufficient funds, the protocol uses 3USD from the pool to cover the debt. Look for the sequence that:
   - Calculates how much 3USD is needed to cover the vault's icUSD debt
   - Calls the 3pool canister to redeem 3USD for icUSD
   - Burns the icUSD via the icUSD ledger to retire the debt

2. **Replace the redeem-and-burn sequence with a transfer.** Instead of calling the 3pool to redeem, transfer the 3USD to the backend canister. The debt is still written off in the vault state (the vault is closed, the debt counter decremented), but no icUSD is actually burned and no 3pool redemption happens.

3. **Track protocol-held 3USD reserves.** Add a field to the backend state (something like `protocol_3usd_reserves: u64`) that tracks how much 3USD the backend holds from liquidations. This is important for transparency and accounting. Increment it on each liquidation that routes through the stability pool.

4. **Update the backend canister's 3USD balance tracking.** The backend needs to be able to receive 3USD. If the backend doesn't currently interact with the 3pool's LP token ledger, you'll need to add the 3pool LP token's canister ID to the backend's known tokens so it can receive and hold 3USD. This may already exist if the backend has any existing 3USD awareness.

5. **Adjust the total supply / backing accounting if applicable.** If there is any on-chain or frontend logic that calculates "total icUSD backed by X" or "protocol health ratio," make sure the protocol-held 3USD is counted as backing. The 3USD in reserves is redeemable for stablecoins at any time, so it is real backing.

## What Does NOT Need to Change

- The collateral seizure logic (taking ICP/ckBTC/etc from the insolvent vault) stays the same.
- The distribution of seized collateral to stability pool depositors stays the same.
- The stability pool deposit/withdrawal logic stays the same (users still deposit 3USD into the stability pool).
- The vault closure and debt write-off logic stays the same (the vault is marked liquidated, its debt counter goes to zero).
- The liquidator bot canister logic stays the same (it still detects undercollateralized vaults and triggers liquidation).

## Edge Cases to Handle

- **What if the 3USD transfer to the backend fails?** The entire stability pool liquidation attempt is aborted. No debt is written off, no collateral is seized, the depositor's 3USD stays exactly where it was as part of their deposit. The vault remains undercollateralized and gets sent to the next mechanism in the liquidation cascade (manual liquidation queue). No retry logic, no partial states. It either completes fully or it doesn't happen at all.
- **What about partial liquidations?** If the stability pool only has enough 3USD to cover part of the debt, the same proportional logic applies. Transfer what you can, fall back to other liquidation mechanisms (manual liquidation, liquidator bot) for the remainder.
- **What if someone queries total backing?** The frontend or any transparency dashboard should show: total vault collateral + protocol-held 3USD reserves = total backing for circulating icUSD.

## Future Consideration (Do NOT Build Now)

An emergency unwind function where governance (future SNS DAO) could vote to redeem protocol-held 3USD, recover icUSD, and burn it. This is not needed now. The 3USD is safe in reserves and there is no realistic edge case that requires immediate unwinding. Build this if and when the protocol has governance infrastructure.

## Testing

- Deploy to local replica. Open a vault, borrow icUSD, let it become undercollateralized, trigger liquidation via stability pool. Verify:
  - Vault is closed and debt is written off
  - 3USD moves from stability pool to backend canister (check 3USD ledger balances)
  - No 3pool redemption call is made
  - No icUSD burn call is made
  - Stability pool depositor receives seized collateral as expected
  - `protocol_3usd_reserves` state field increments correctly
  - 3pool TVL is unchanged after liquidation
