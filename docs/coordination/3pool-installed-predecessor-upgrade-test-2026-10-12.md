# 3pool installed-predecessor upgrade regression (2026-10-12)

An ignored, hash-gated PocketIC test installs the exact 3pool production Wasm rebuilt from a clean Git archive of PR #483 merge commit `7b2d0dbf690fa0a64007e99ba016a89e31f61491` (SHA-256 `9797bf2454923ef41d6ef6a003435e0430aaa2b2b413734c6d9e6e921889dfc0`) and upgrades it through its real `pre_upgrade` into the candidate production Wasm (SHA-256 `7e2e6f0bc4054000acd4ef6875752a663c99af41e39aba2f20585f5a1cd2c631`). The predecessor hash matches the installed module; the expected hashes are checked before either module is installed.

Run from the repository root with the PocketIC server at `/private/tmp/pocketic-7.0.0/pocket-ic` and the two Wasm paths supplied through the variables shown:

```bash
CARGO_TARGET_DIR=/private/tmp/rumi-exact-predecessor-test-target \
CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_DEV_DEBUG=0 \
POCKET_IC_BIN=/private/tmp/pocketic-7.0.0/pocket-ic \
RUMI_3POOL_INSTALLED_PREDECESSOR_WASM=/private/tmp/rumi-3pool-installed-pr483-9797.wasm \
RUMI_3POOL_CANDIDATE_WASM="$PWD/.icp/cache/artifacts/rumi_3pool" \
cargo test --offline -p rumi_3pool --test installed_predecessor_upgrade -- --ignored --nocapture
```

Result: **1 passed**. The predecessor contained 300,000,000,000,000 LP shares across two owners, a legacy LP allowance of 1,234,567, and 49,999 units of accrued admin fees. The first upgrade preserved LP balances/supply, pool and ledger reserves, and admin fees; it invalidated the ambiguous legacy allowance and left user and pool underlying-token balances unchanged. A newly created 765,432 allowance survived a second upgrade. The predecessor reported zero pending claims; the test reads the reported count and all claim pages and confirms the same complete empty list after upgrade. This run does not exercise preservation of a nonzero legacy claim.

Captured output: `/private/tmp/rumi-3pool-installed-predecessor-upgrade-test.log`. This representative PocketIC state does not prove recovery of the live canister's complete state or liabilities and is not deployment evidence.
