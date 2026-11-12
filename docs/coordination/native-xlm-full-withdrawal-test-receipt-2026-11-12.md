# XLM test-only full withdrawal: focused local test result

Recorded 2026-11-12. Result: **PASS**, independently accepted. Mocked local test evidence only.

## Approval

Rob approved the full-withdrawal focused test (Decision B). That approval is answered and consumed by this single run; Cody issued a fresh exclusive allocation and has released the slot. Earlier grants and denials are unchanged and were not reused (`fd31b869` denied and released; `19430bb3` consumed by the partial-withdrawal run).

## What ran

- Allocation `4856e03a-2958-43bc-8b86-7c238c7a8b1d`, exclusive, record SHA-256 `07450b88770a652906428a69ef4f5c12c761ff9051eb0e83dcc3cc54c60ed3dd`.
- Runner SHA-256 `cb1c89dacc0114a2f582649856b68b3039f213b65f28970c82237a6ffcd60be5`, invoked once.
- Implementation `0aa0e62338b572dc728889d65930ac71d6be8ff1`; run-time HEAD `065d1ad0200d5f21f7311ec099ba2d6fd309c008` (only `docs/coordination/` files differ).
- Toolchain: `rustc 1.92.0 (ded5c06cf 2025-12-08)`, `cargo 1.92.0 (344c4567c 2025-10-21)`, `pocket-ic-server 7.0.0`.
- 2026-11-12 22:35:42Z to 22:38:04Z (142 s).

| Step | Exit | Result | Log SHA-256 (prefix) |
|---|---|---|---|
| 1. Test Wasm build (`--features test_endpoints`) | 0 | built | `367eba727b92f980` |
| 2. `xlm_confirm_pic` (PocketIC, mocked Horizon and price) | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 44.18s` | `46efea9f39e8fe6b` |
| 3. Production compile check, features off | 0 | clean | `263ffc8c2298ccc4` |

- Test Wasm SHA-256 `a6c1c8a4ab798de1d828e3c94fd39854015c3b9402cea4676cc3eeea8fb5c729` (13,824,662 bytes), unchanged through steps 2 and 3.
- Resources: lowest free disk 73.5 GiB, growth 0.58 GiB, peak memory 1.86 GiB.
- All owned process groups closed and independently confirmed empty. Source clean after the run.
- Terminal receipt SHA-256 `1caaefde39c3ed45b68c4d75027eb9cf065f22f36661ed4a6fca691ad7f3652a`.

## What this proves (mocked local tests)

- Full withdrawal is rejected while debt remains, for an outsider, for an invalid destination and when repeated on an empty vault; each time icUSD balances, supply, collateral and claim count are unchanged and debt moves only within the bounded accrued-interest allowance.
- After the remaining debt is repaid to exactly zero, the owner's full withdrawal creates one claim for all remaining collateral (889,990,000 stroops) with the correct custody owner, nonce, testnet network id and fee. The vault stays open with zero collateral and zero debt, the locked reserve is not claimed, no duplicate claim is created, and no icUSD moves.
- Deposit-to-vault, borrow, repay and partial withdrawal still pass; the production build compiles with test-only code excluded.

## What it does not prove

- Production XLM remains Frozen; production route guards are unchanged.
- Claims stay pending: no settlement, signing, Stellar transaction or payout exists or was tested.
- Nothing ran on a live network or with real funds.
