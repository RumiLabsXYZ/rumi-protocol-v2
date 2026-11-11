# XLM test-only withdrawal: focused local test result

Recorded 2026-11-11. Result: **PASS**, independently accepted. This is mocked local test evidence only.

## Approval

The request in `native-xlm-withdrawal-test-request.md` (commit `bea20dd9`, SHA-256 `a6834016620146ba0d2b93c679f46b5c70ea914a4265d2c6e798bb9afdf94a46`) was answered: Rob approved the focused XLM withdrawal test and Cody sending Mara the allocation. The earlier automatic-review denial (allocation `fd31b869`, released, nothing ran) is resolved by that approval.

## What ran

- Allocation `19430bb3-e757-486a-8525-19dae936bcd0`, exclusive, record SHA-256 `9908e2e707c9b410da1a7007163cec6ad465471822337111a07ee9dfe230c3b7`.
- Runner SHA-256 `d78c964a06b72cede2dc52482eab5ed06f32c095281ff8ca47ee6cfc57b76fb4`, invoked once.
- Implementation `b68748969d655e1c161e31bc3f388baaecff33e0`; run-time HEAD `bea20dd9ed90cd6b76bed6492d92a6ce20d81e2c` (only `docs/coordination/` files differ).
- Toolchain: `rustc 1.92.0 (ded5c06cf 2025-12-08)`, `cargo 1.92.0 (344c4567c 2025-10-21)`, `pocket-ic-server 7.0.0`.
- 2026-11-11 22:34:50Z to 22:37:08Z (137 s).

| Step | Exit | Result | Log SHA-256 (prefix) |
|---|---|---|---|
| 1. Test Wasm build (`--features test_endpoints`) | 0 | built | `42896881a76aa2d6` |
| 2. `xlm_confirm_pic` (PocketIC, mocked Horizon and price) | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 42.13s` | `cfde72fde16c91f7` |
| 3. Production compile check, features off | 0 | clean | `f7b3c1827db6b956` |

- Test Wasm SHA-256 `a4f137ae6bf31fefc2eaa22ecd261fa05d70861d365f3ce9ad6c1b4ef8f5db5f` (13,822,269 bytes), unchanged through steps 2 and 3.
- Resources: lowest free disk 87.3 GiB, growth 0.96 GiB, peak memory 1.83 GiB.
- All owned process groups closed and independently confirmed empty. Source clean after the run.
- Terminal receipt SHA-256 `4db74caccd3197acc8306dee4cdbc0617dcb3ce9a54098b53d7160bef9c4f1ff`.

## What this proves

In mocked local tests, on the same confirmed XLM vault after borrow and repay:

- Withdrawals by an outsider, withdrawals that would break the collateral ratio, and withdrawals to an invalid destination are rejected. Balances and icUSD supply stay unchanged and debt moves only within the bounded accrued-interest allowance.
- The owner's 10 XLM withdrawal creates one durable XLM claim with the correct custody owner, nonce, exact testnet network id, amount and fee, and reduces vault collateral by exactly that amount, with no icUSD movement.
- Earlier deposit-to-vault, borrow and repay checks still pass, and the production build compiles with the test-only code excluded.

## What it does not prove

- Production XLM remains Frozen; production route guards are unchanged.
- Claims stay pending: no settlement, signing, Stellar transaction or payout exists or was tested.
- Nothing ran on a live network or with real funds.

## Next

Test-only close-to-claim stays in the source-only backlog (see the close-vault slice note). No worker or build allocation is requested for it.
