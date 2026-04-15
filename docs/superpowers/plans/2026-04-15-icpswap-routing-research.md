# ICPswap Routing Research (Task 1)

Research output for Task 1 of `2026-04-15-icpswap-routing-integration.md`.
Captured on 2026-04-15 from mainnet via `dfx canister --network ic`.

## Pool canister IDs

- **ICPswap 3USD/ICP:** `mu2zw-6iaaa-aaaar-qb56q-cai` (fee: 30 bps, token0: 3USD, token1: ICP)
- **ICPswap icUSD/ICP:** `nqxwe-hiaaa-aaaar-qb5yq-cai` (fee: 30 bps, token0: ICP, token1: icUSD)

Both standards are `"ICRC2"` (ICPswap canonicalizes ICP too, even though the protocol-side ledger is ICRC-1 with ICRC-2 approval support).

Discovery path: NodeIndex (`ggzvv-5qaaa-aaaag-qck7a-cai`) did not have these pools registered (`getPoolsForToken` returned empty, `getAllPools` had no matching ledger IDs). Both were found via SwapFactory `4mmnk-kiaaa-aaaag-qbllq-cai` `getPool` using fee = 3000 and standard = "ICRC2".

The `key` field from the factory encodes the canonical ordering: `<token0>_<token1>_<fee>`.
- 3USD/ICP key: `fohh4-yyaaa-aaaap-qtkpa-cai_ryjl3-tyaaa-aaaaa-aaaba-cai_3000` (3USD is token0)
- icUSD/ICP key: `ryjl3-tyaaa-aaaaa-aaaba-cai_t6bor-paaaa-aaaap-qrd5q-cai_3000` (ICP is token0)

**Important for `zeroForOne` wiring:** token0 is the token being sold when `zeroForOne = true`.
- 3USD/ICP pool: `zeroForOne = true` sells 3USD for ICP; `false` sells ICP for 3USD.
- icUSD/ICP pool: `zeroForOne = true` sells ICP for icUSD; `false` sells icUSD for ICP.

## Interface summary

- **Style:** V3 (sqrtPriceX96, tickSpacing = 60, concentrated liquidity). The `metadata` response includes `sqrtPriceX96`, `tick`, `liquidity`, `maxLiquidityPerTick`, and `nextPositionId`.
- **Candid hash:** both pools share identical candid (md5 `56851a30f484d94af5130dd3d7c3d530`, 958 lines). Captured to `/tmp/icpswap_pool.did` during research (may need to re-pull for Task 2).

### Methods present

| Method | Kind | Purpose |
|---|---|---|
| `metadata` | query | Returns `PoolMetadata` (fee, key, sqrtPriceX96, tick, liquidity, token0, token1, etc.) |
| `quote` | query | Simulates swap, returns expected output `nat` (includes fee, does not update state) |
| `quoteForAll` | query | Variant (crosses ticks differently - use `quote` as default) |
| `deposit` | update | Moves internal subaccount balance into pool after user ICRC-1 transfer |
| `depositFrom` | update | Pulls token from caller via ICRC-2 `transfer_from` (what we want) |
| `swap` | update | Executes swap, consumes internal balance, credits output to caller's internal balance |
| `withdraw` | update | Sends output token from internal balance back to caller |
| `depositAndSwap` | update | Combined deposit + swap (requires separate upfront deposit, expects caller balance already on internal account) |
| `depositFromAndSwap` | update | Combined `depositFrom` + `swap` - **preferred path** (single ICRC-2 approval + one call) |
| `getTokenAmountState` | query | Returns current pool reserves (`token0Amount`, `token1Amount`, plus fee accounting fields) |

### Parameter shapes

**`SwapArgs`** (used by `swap` and `quote`):
```candid
record {
  amountIn: text;          // integer as string (e8s), "100000000" = 1.0
  amountOutMinimum: text;  // slippage floor (e8s string), "0" for no floor
  zeroForOne: bool;        // true = sell token0, false = sell token1
}
```

**`DepositArgs`** (used by `deposit` and `depositFrom`):
```candid
record {
  amount: nat;  // base units including the ledger transfer fee
  fee: nat;     // the ledger's transfer fee (e.g. ICP = 10_000)
  token: text;  // ledger canister ID as text
}
```

**`DepositAndSwapArgs`** (used by `depositAndSwap` and `depositFromAndSwap`):
```candid
record {
  amountIn: text;
  amountOutMinimum: text;
  tokenInFee: nat;   // fee for input token ledger
  tokenOutFee: nat;  // fee for output token ledger
  zeroForOne: bool;
}
```

**`WithdrawArgs`**:
```candid
record {
  amount: nat;
  fee: nat;
  token: text;  // ledger canister ID as text
}
```

### Result envelope

All of `quote`, `swap`, `deposit`, `depositFrom`, `depositAndSwap`, `depositFromAndSwap`, `withdraw` return:
```candid
variant {
  ok: nat;
  err: variant { CommonError; InsufficientFunds; InternalError: text; UnsupportedToken: text }
}
```
The `ok` payload is a `nat`: the output amount for swap/quote methods, the credit amount for deposit, and the withdrawn amount for withdraw.

`metadata` returns `Result_6 = variant { ok: PoolMetadata; err: Error }` and `getTokenAmountState` returns `Result_18 = variant { ok: record { token0Amount, token1Amount, swapFee0Repurchase, swapFee1Repurchase, swapFeeReceiver }; err: Error }`.

### Verification

Sanity-checked `quote` for both pools on 2026-04-15:
- 3USD/ICP, 1 3USD in (zeroForOne = true): `ok = 43_374_455` (0.4337 ICP out)
- icUSD/ICP, 1 ICP in (zeroForOne = true): `ok = 243_622_161` (2.4362 icUSD out)

Both match the implied spot price from sqrtPriceX96 (below), so the pool is responsive and the method shapes are correct.

## Liquidity depth

Reserves from `getTokenAmountState`, priced at ICP = $2.51 (from Rumi backend `get_protocol_status` at query time):

- **ICPswap 3USD/ICP** (`mu2zw-6iaaa-aaaar-qb56q-cai`):
  - 3USD: 46,766,082,357 = **467.66 3USD** (≈$509 at ~$1.09 spot)
  - ICP: 20,739,134,202 = **207.39 ICP** (≈$520)
  - **Total: ~$1,029.** Spot price implies 1 3USD ≈ 0.435 ICP ≈ $1.09 (modest premium above $1, expected for a yield-bearing stablecoin).
- **ICPswap icUSD/ICP** (`nqxwe-hiaaa-aaaar-qb5yq-cai`):
  - ICP: 66,346,915,816 = **663.47 ICP** (≈$1,665)
  - icUSD: 159,414,639,114 = **1,594.15 icUSD** (≈$1,594)
  - **Total: ~$3,259.** Spot price implies 1 ICP ≈ 2.44 icUSD (close to the $2.51 oracle, slight discount from pool fee + thin book).

### Flags / concerns

- **Both pools are below the $5k flag threshold.** 3USD/ICP is the thinnest at ~$1k. Even moderate swaps (>$100) will eat noticeable slippage from the V3 active-range liquidity (active `liquidity` values are 100B and 302B respectively, both concentrated around a single tick).
- **3USD/ICP at ~$1k depth is essentially dust.** Anything over ~$50 will probably have >1% slippage. The aggregator should still quote it (maybe best-of-n), but we should expect Rumi AMM to win most routes until LPs deepen this pool.
- **icUSD/ICP at ~$3.3k is slightly better but still thin.** For our target swap sizes (retail vault top-ups / icUSD exits, probably $10–$500), this pool is usable but single-side-heavy fills will skew price.
- **Consider slippage guard defaults.** Because both pools are thin, Task 8 (UI slippage UX) should default to a generous `amountOutMinimum` tolerance (e.g. 1–2%) or at minimum surface the quoted impact prominently.
- **Both pools are registered with SwapFactory but not NodeIndex.** This means ICPswap's own analytics site won't surface them. Not a bug for our integration (we talk to the pool canister directly), but worth noting that volume stats will be invisible from NodeIndex.
- Both pools share `swapFeeReceiver = cbkxt-gaaaa-aaaag-qcs4a-cai` (ICPswap protocol treasury — expected).

## Notes for Task 2 (candid generation)

- **Full `.did` captured at:** `/tmp/icpswap_pool.did` during this research. Both pool canisters return **byte-identical candid** (md5 `56851a30f484d94af5130dd3d7c3d530`), so we only need one declaration set and can reuse it for both pool IDs.
  - For Task 2, re-pull to a stable location: `dfx canister --network ic metadata mu2zw-6iaaa-aaaar-qb56q-cai candid:service > src/declarations/icpswap_pool/icpswap_pool.did` (or chosen path).
- **Token standard string is `"ICRC2"`** in ICPswap's `Token.standard` field for all three ledgers (ICP, icUSD, 3USD). Don't pass `"ICRC1"` - the factory treats them as distinct and returns `InvalidPoolId`.
- **Use `depositFromAndSwap` as the primary path.** Single call, single ICRC-2 approval. The fallback 3-call flow (`depositFrom` -> `swap` -> `withdraw`) is still useful for partial-failure recovery but the combined method is cheaper and atomic from the UI's perspective.
  - Open question for Task 4: does `depositFromAndSwap` auto-withdraw the output, or does it leave it on the internal subaccount? The candid doesn't say - we'll need to test on mainnet or read ICPswap docs. If it leaves the balance internal, we still need an explicit `withdraw` step.
- **Amounts are stringified `nat`** in `SwapArgs` / `DepositAndSwapArgs` but plain `nat` in `DepositArgs` / `WithdrawArgs`. Be careful with the type mismatch when generating TS bindings.
- **Fee semantics in `DepositArgs` / `WithdrawArgs`:** the `fee` field is the ledger's ICRC-1 transfer fee, not the pool fee. We should query `icrc1_fee()` on each ledger rather than hardcoding (see existing pattern in `DepositInterface.svelte` and related bundling note in project memory).
- **Slippage unit:** `amountOutMinimum` is in the output token's base units (e8s). For a $100 trade with 1% tolerance, compute `expectedOut * 0.99` in base units.
- **`zeroForOne` per pool** (worth pinning in a constant so downstream code doesn't guess):
  - 3USD/ICP: `zeroForOne = true` = selling 3USD.
  - icUSD/ICP: `zeroForOne = true` = selling ICP.
- **Query vs update timing:** `quote` is a query (fast, no consensus wait), but swap/deposit/withdraw are updates (2-3s each). The `depositFromAndSwap` combined call cuts update latency by ~50% vs. the 3-call flow.
