# Operations Note: icp-cli Production Deploy Pattern

**Date:** 2026-05-27
**Discovered during:** Task 9 (icusd_index migration)
**Status:** Locked. Apply to Tasks 10-16.

## Important correction to Task 2 (deps-pull spike)

The deps-pull spike documented `.icp/cache/mappings/<env>.ids.json` as the canister-ID mapping path, but that's only where icp-cli AUTO-GENERATES local-network mappings during `icp network start`. The canonical, manually-authored mapping path is `.icp/data/mappings/<env>.ids.json` per the official migration guide at cli.internetcomputer.org/0.2/migration/from-dfx/.

The docs are explicit: `.icp/data/` should be committed to source control; `.icp/cache/` is the only subdirectory that should be gitignored.

This was the missing piece. With the mapping at the correct path, `icp canister install <name>` resolves the principal automatically and the per-deploy command simplifies.

## Problem (with original wrong path)

`icp deploy <name> --environment mainnet-live --mode upgrade` does NOT work for canisters that already exist on mainnet. Its pipeline is `build → create-if-not-exists → install`, and the create step:

1. Demands 2T cycles from icp-cli's cycle wallet (separate from dfx's wallet)
2. Fails with `Insufficient cycles. Requested: 2000000000000 cycles, available balance: 326_850_000_000 cycles`

`--mode upgrade` only governs the install step; it does not suppress the create step.

When the canister-ID mapping is at the wrong path (`.icp/cache/mappings/...`), `icp canister install <name>` errors with `failed to lookup canister ID for canister '<name>' in environment 'mainnet-live'`.

## Locked deploy pattern (with correct mapping path)

For every existing-mainnet canister upgrade, with `.icp/data/mappings/mainnet-live.ids.json` populated:

```bash
icp canister install <CANISTER_NAME> \
  --environment mainnet-live \
  --identity rumi_identity \
  --mode upgrade \
  --args '<CANDID_UPGRADE_ARGS>'
```

icp-cli resolves the canister name to its principal via the mapping file, uses the build output from `icp.yaml` as the wasm, runs the install (no create step), and respects `--mode upgrade`. This is much closer to dfx ergonomics than the principal-and-explicit-wasm fallback used in Task 9.

### Important: `--args` is required for canisters whose post_upgrade has a parameter

Discovered during Task 14 (AMM deploy). The env-level `init_args` override in `icp.yaml` is NOT applied by `icp canister install`. It only applies to `icp deploy` (the create-then-install pipeline we cannot use for existing canisters).

For `icp canister install`:
- If the canister's `post_upgrade()` takes zero parameters (e.g., rumi_treasury): `--args` can be omitted. Candid never decodes the wire.
- If the canister's `post_upgrade(_args: SomeType)` has a parameter (e.g., rumi_amm, rumi_3pool, liquidation_bot, rumi_stability_pool): `--args '<candid>'` MUST be passed explicitly. Otherwise Candid decode fails with `No more values on the wire` and the upgrade traps. Atomic upgrade semantics roll the state back, but it is a deploy failure that needs a retry with `--args`.

Always pass `--args` for safety. The IC upgrade is atomic — even if the args are wrong, the state survives via rollback.

### Fallback (target by principal directly)

If name resolution fails for any reason, target by principal with explicit wasm:

```bash
icp canister install <CANISTER_PRINCIPAL> \
  --wasm <PATH_TO_WASM> \
  --environment mainnet-live \
  --identity rumi_identity \
  --mode upgrade \
  --args '<CANDID_UPGRADE_ARGS>'
```

Used in Task 9 (icusd_index) before the mapping path was corrected.

## Pre-deploy hook compatibility

`.claude/hooks/pre-deploy-test.sh` was extended to fire on `icp canister install` (in addition to `dfx deploy`, `dfx canister install`, and `icp deploy`). The test gate is preserved.

## Worked example: icusd_index

```bash
icp canister install 6niqu-siaaa-aaaap-qrjeq-cai \
  --wasm src/ledger/ic-icrc1-index-ng.wasm.gz \
  --environment mainnet-live \
  --identity rumi_identity \
  --mode upgrade \
  --args '(null)'
```

Result: `Canister 6niqu-siaaa-aaaap-qrjeq-cai installed successfully`
Pre-deploy module hash: `0xcf3bf8f87dc908be156f314fae3b83aae56d1f63e74a63c32994c4e02babdb2d`
Post-deploy module hash: `0xcf3bf8f87dc908be156f314fae3b83aae56d1f63e74a63c32994c4e02babdb2d` (unchanged, true no-op)
Smoke test (`status` query): `num_blocks_synced = 18_452` (state preserved)

## Mappings bootstrap

`scripts/icp-mappings-bootstrap.sh` regenerates `.icp/data/mappings/mainnet-live.ids.json` from the project's `canister_ids.json`. The mapping file IS committed to source control (per docs) so subsequent devs do not need to re-run this. The script remains useful for keeping the mapping fresh if `canister_ids.json` is updated (e.g. new canister IDs from Phase 1 staging deploys).

```bash
./scripts/icp-mappings-bootstrap.sh
```

## What this changes for Tasks 10-16

Every remaining canister migration uses the worked-example pattern, substituting:
- Principal from `canister_ids.json`
- Wasm path from `dfx.json` (or icp-cli's `.icp/cache/artifacts/<name>` after `icp build`)
- Upgrade args matching that canister's documented shape

Decision gates on Tasks 11 (3pool), 14 (AMM), 16 (backend) still apply: hash mismatch halts the deploy and surfaces to the user.

## What this does NOT change

- Wasm hash compares are still load-bearing (pre-deploy and post-deploy)
- User authorization is still required before each production deploy
- The `--identity rumi_identity` flag is still required
- The pre-deploy hook still fires (now via the expanded regex)
- Frontend declarations still come from `scripts/regenerate-declarations.sh`
- icp.yaml still describes the canisters (the file is the source of truth for what we deploy)
