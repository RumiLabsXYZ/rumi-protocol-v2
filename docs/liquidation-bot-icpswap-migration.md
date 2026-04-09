# Liquidation Bot ICPSwap Migration Runbook

Deploys the rewritten liquidation bot (KongSwap+3pool replaced with single-hop ICPSwap ICP->ckUSDC) and the backend cleanup (delete `bot_deposit_to_reserves`, add `admin_resolve_stuck_claim`).

## Pre-deployment checklist

- [ ] Verify no active bot claims: `dfx canister call rumi_protocol_backend get_bot_stats --network ic`
  - `budget_remaining_e8s` should be 0 or the bot should have no pending vaults
- [ ] Verify bot has no stuck liquidations in the old format

## Step 1: Deploy the bot canister (upgrade, NOT reinstall)

```bash
dfx deploy liquidation_bot --network ic
```

The `post_upgrade` handler will:
1. Rescue the legacy JSON blob from raw stable memory
2. Initialize MemoryManager (writes header at offset 0)
3. Migrate legacy `BotLiquidationEvent` entries into the new StableBTreeMap
4. Mark `migrated_to_stable_structures = true`
5. Start the 30-second timer

## Step 2: Verify migration

```bash
# Should return the count of migrated legacy events
dfx canister call liquidation_bot get_liquidation_count --network ic

# Verify stats carried over
dfx canister call liquidation_bot get_bot_stats --network ic
```

## Step 3: Set new config

The old KongSwap/3pool fields are now optional. Set the new ICPSwap config:

```bash
dfx canister call liquidation_bot set_config '(record {
  backend_principal = principal "tfesu-vyaaa-aaaap-qrd7a-cai";
  treasury_principal = principal "tlg74-oiaaa-aaaap-qrd6a-cai";
  admin = principal "<YOUR_ADMIN_PRINCIPAL>";
  max_slippage_bps = 200 : nat16;
  icp_ledger = principal "ryjl3-tyaaa-aaaaa-aaaba-cai";
  ckusdc_ledger = principal "xevnm-gaaaa-aaaar-qafnq-cai";
  icpswap_pool = principal "mohjv-bqaaa-aaaag-qjyia-cai";
  icpswap_zero_for_one = null;
  icp_fee_e8s = opt (10_000 : nat64);
  ckusdc_fee_e6 = opt (10 : nat64);
  three_pool_principal = null;
  kong_swap_principal = null;
  ckusdt_ledger = null;
  icusd_ledger = null;
})' --network ic
```

## Step 4: Resolve pool ordering

Fetches ICPSwap pool metadata to determine if ICP is token0 or token1:

```bash
dfx canister call liquidation_bot admin_resolve_pool_ordering --network ic
```

## Step 5: Set infinite ICRC-2 approve

Approves the ICPSwap pool to spend the bot's ICP (u128::MAX, no expiry):

```bash
dfx canister call liquidation_bot admin_approve_pool --network ic
```

## Step 6: Deploy backend

```bash
dfx deploy rumi_protocol_backend --network ic --argument '(variant { Upgrade = record { mode = null; description = opt "Delete bot_deposit_to_reserves, add admin_resolve_stuck_claim" } })'
```

## Step 7: Deploy frontend

```bash
dfx deploy vault_frontend --network ic
```

## Step 8: Re-enable bot liquidations

Add ICP to the bot's allowed collateral types (if not already set):

```bash
dfx canister call rumi_protocol_backend set_bot_allowed_collateral_types '(vec { principal "ryjl3-tyaaa-aaaaa-aaaba-cai" })' --network ic
```

Set a budget if needed:

```bash
dfx canister call rumi_protocol_backend set_bot_budget '(50_000_000_000 : nat64)' --network ic
```

## Step 9: Monitor

```bash
# Watch for first liquidation
dfx canister call liquidation_bot get_liquidations '(0 : nat64, 5 : nat64)' --network ic

# Check for stuck liquidations
dfx canister call liquidation_bot get_stuck_liquidations --network ic
```

## Candid breaking change

`BotConfig` fields changed: `three_pool_principal`, `kong_swap_principal`, `ckusdt_ledger`, `icusd_ledger` are now `opt principal` (previously required `principal`). New required fields: `icpswap_pool`. Old dfx scripts that call `set_config` will need updating.

## Rollback

If something goes wrong:
1. Re-deploy the old bot wasm: `dfx deploy liquidation_bot --network ic --wasm <path-to-old-wasm>`
2. The old wasm's `post_upgrade` uses raw stable64_read, but MemoryManager has overwritten offset 0. **Rollback is NOT safe** after the new wasm has been deployed and upgraded.
3. If rollback is needed, consider reinstalling the bot canister (this loses history but restores a clean state). The bot's history is informational only; no funds are at risk.
4. The backend changes are backward-compatible (only deleted a dead endpoint and added a new one). No backend rollback needed.
