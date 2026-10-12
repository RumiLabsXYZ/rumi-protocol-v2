# 3pool installed-predecessor upgrade regression (2026-10-12)

The ignored, hash-gated PocketIC test upgrades the exact Wasm installed on `rumi_3pool` at preflight, SHA-256 `74a81602f61ee08023bc1a6746f4961aebcdb8c7f98b0019c9eefbb63a3998ae`, to the candidate built from merged PR #553, SHA-256 `d1bc983879dc4a385ec87a398168f798802027b428bbef0f1c8382435ed96c6e`. A clean `icp build rumi_3pool -e mainnet-live` at source commit `5f776101` reproduced the installed predecessor hash; the same command at merge commit `3ddd3134` reproduced the candidate hash twice. The test checks both artifact hashes before touching its local PocketIC canister.

The `/private/tmp` paths below are local evidence, not tracked fixtures. To reproduce them elsewhere, build `rumi_3pool` with `icp build rumi_3pool -e mainnet-live` in clean checkouts of commits `5f776101` and `3ddd3134`, save each `.icp/cache/artifacts/rumi_3pool` separately, and verify the two SHA-256 values above before running the test.

Run the opt-in test with the two exact production artifacts:

```bash
POCKET_IC_BIN=/private/tmp/rumi-pocketic7/pocket-ic \
RUMI_3POOL_INSTALLED_PREDECESSOR_WASM=/private/tmp/rumi-3pool-live-74a816.wasm \
RUMI_3POOL_CANDIDATE_WASM=/private/tmp/rumi-3pool-pr553-d1bc.wasm \
cargo test --locked -p rumi_3pool --test installed_predecessor_upgrade -- --ignored --nocapture
```

Result: **1 passed**. The predecessor contained 300,000,000,000,000 LP shares across two owners, a valid LP allowance of 1,234,567, 49,999 units of accrued admin fees, three underlying ledger balances, and at least one retained global payout identity from a swap. The upgrade preserved LP balances and supply, pool reserves, underlying balances, admin fees, the allowance, every page of the global payout inventory, and all exposed pending claims. A second candidate-to-candidate upgrade preserved these values and a new allowance. The predecessor reported zero pending claims; this test does not exercise a nonzero legacy claim. PR #553 separately tests an archived-input proof cursor across a candidate-to-candidate upgrade.

The older PR #483 predecessor expected an ambiguous LP allowance to be invalidated. That migration had already run in the currently installed predecessor, so this test now requires a valid pre-upgrade allowance to survive. This is local migration evidence, not proof of the live canister's full state, exact liabilities, or a successful production installation.
