# Spike: icp-cli Upgrade-arg Expressiveness

**Date:** 2026-05-27
**Branch:** `feat/icp-cli-migration`
**icp-cli version tested:** 0.2.0
**Question:** Can `icp.yaml` + `icp-cli` handle Rumi's required upgrade-arg patterns?

## Verdict

**GO.** Both required patterns are expressible cleanly via `icp.yaml` with a single source of truth, no per-command argument juggling. A CLI-flag escape hatch also exists for one-off deploys.

## Mechanism (key insight)

`icp.yaml` has two places to declare init args:

1. **Canister-level `init_args`** (top-level `canisters:` entry) → used for the initial `install` mode.
2. **Environment-level `init_args.<canister_name>`** (under an `environments:` entry) → **overrides** for that specific environment, and is the value passed on `--mode upgrade`.

Both accept a plain string (treated as inline Candid by default) or `{ value: "...", format: candid }` / `{ path: ..., format: bin }` objects.

`icp deploy --mode upgrade` reads the environment-level override and passes it to `post_upgrade`. `icp canister install --mode upgrade --args '(...)'` is the CLI escape hatch for ad-hoc overrides.

## Patterns tested

### Pattern A: backend `Upgrade` variant

Required by `rumi_protocol_backend`: every upgrade must pass `(variant { Upgrade = record { mode = null; description = opt "..." } })` so the description gets logged.

**Result: WORKS.**

End-to-end test:
- Canister with `enum InitOrUpgrade { Init, Upgrade(UpgradeArgs) }`, candid `service : (InitOrUpgrade) -> {}`.
- `icp.yaml` canister-level `init_args: '(variant { Init })'` for fresh install.
- `icp.yaml` environment-level `init_args.test_canister: '(variant { Upgrade = record { mode = null; description = opt "test via env override" } })'`.
- `icp deploy test_canister --environment local --mode upgrade` succeeded.
- `icp canister logs test_canister --environment local` showed `Upgrade description: test via env override`, confirming `post_upgrade` received the variant-wrapped record.

Also verified the CLI escape hatch works identically: `icp canister install test_canister --environment local --mode upgrade --args '(variant { Upgrade = record { mode = null; description = opt "test upgrade via canister install" } })' -y`.

### Pattern B: dummy `InitArgs` on upgrade

Required by `rumi_3pool`, `liquidation_bot`, `rumi_stability_pool`: each canister declares a flat `InitArgs` record (no variant wrapper). `post_upgrade` ignores most fields but the Candid decoder still requires the shape.

**Result: WORKS.**

End-to-end test:
- Canister with `struct PoolInitArgs { admin, token_a, token_b, fee_bps }`, candid `service : (PoolInitArgs) -> {}`.
- `icp.yaml` canister-level `init_args: '(record { admin = principal "..."; token_a = principal "..."; token_b = principal "..."; fee_bps = 30 : nat32 })'`.
- `icp.yaml` environment-level override with `fee_bps = 99` (different value, exercising the override).
- `icp deploy test_canister_b --environment local --mode upgrade` succeeded.
- `icp canister logs` confirmed `post_upgrade fired (args ignored) v2`. The record decoded cleanly; the field values are discarded by `post_upgrade` exactly as in production canisters.

CLI escape hatch also verified with `--args '(record { ... fee_bps = 77 : nat32 })'`.

## Sequence (`icp deploy --mode upgrade` end-to-end)

1. `icp.yaml` declares both canister-level init args (for first install) and environment-level overrides (for upgrades).
2. `icp deploy <canister> --environment <env> --mode upgrade` rebuilds the wasm, computes the effective init arg from the environment override, and calls the management canister with `install_code(mode=upgrade, arg=...)`.
3. The canister's `post_upgrade` runs with the decoded argument.

## Recommended `icp.yaml` pattern for each Rumi canister

Below are concrete snippets. The `init_args` values match the patterns documented in MEMORY.md.

### `rumi_protocol_backend` (Pattern A)

```yaml
canisters:
  - name: rumi_protocol_backend
    recipe:
      type: "@dfinity/rust@v3.2.0"
      configuration:
        package: rumi_protocol_backend
        shrink: true
        candid: src/rumi_protocol_backend/rumi_protocol_backend.did
    init_args: '(variant { Init = record { /* fresh-install init record */ } })'

environments:
  - name: ic
    network: ic
    canisters: [rumi_protocol_backend]
    init_args:
      rumi_protocol_backend: '(variant { Upgrade = record { mode = null; description = opt "<edit per deploy>" } })'
```

To change the description per deploy, either:
- edit the yaml line and commit, or
- override at the CLI: `icp canister install rumi_protocol_backend --environment ic --mode upgrade --args '(variant { Upgrade = record { mode = null; description = opt "fix bot liquidation safety" } })' --identity rumi_identity`.

### `rumi_3pool`, `liquidation_bot` (Pattern B, dummy on upgrade)

```yaml
canisters:
  - name: rumi_3pool
    recipe:
      type: "@dfinity/rust@v3.2.0"
      configuration:
        package: rumi_3pool
        shrink: true
        candid: src/rumi_3pool/rumi_3pool.did
    init_args: '(record { /* ThreePoolInitArgs for fresh install */ })'

environments:
  - name: ic
    network: ic
    canisters: [rumi_3pool]
    init_args:
      rumi_3pool: '(record { /* same shape; post_upgrade discards */ })'
```

For canisters whose `InitArgs` is huge, prefer the `path` form so the yaml stays readable:

```yaml
    init_args:
      rumi_3pool:
        path: deploy/args/rumi_3pool_upgrade.did
        format: candid
```

(The file holds raw Candid text the same way you'd pass via `--argument` to dfx.)

### `rumi_stability_pool` (Pattern B, full InitArgs on upgrade)

Same shape as `rumi_3pool` above (environment-level `init_args` carries the full `InitArgs` record). No special treatment needed beyond ensuring the record fields match the current `InitArgs` definition.

### `rumi_analytics` (Pattern B + post-build shrink/gzip)

The wasm post-processing (`ic-wasm shrink` + `gzip`) is the recipe's job, not init args'. Three options:

1. **Default rust recipe with `shrink: true`** already runs `ic-wasm shrink`; check whether the result still exceeds the 2 MiB ingress limit. If not, no extra step needed.
2. **Add a custom build adapter step** under the rust recipe configuration that runs `gzip -9 -f` on the wasm. (Recipe configuration accepts custom commands.)
3. **Use the `build:` (not `recipe:`) form** of `CanisterManifest` with explicit `steps:` if you need full control, e.g.:

   ```yaml
   canisters:
     - name: rumi_analytics
       build:
         steps:
           - type: script
             commands:
               - cargo build --package rumi_analytics --target wasm32-unknown-unknown --release
               - cp "${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release/rumi_analytics.wasm" "$ICP_WASM_OUTPUT_PATH"
               - ic-wasm "$ICP_WASM_OUTPUT_PATH" -o "$ICP_WASM_OUTPUT_PATH" shrink --keep-name-section
               - gzip -9 -f "$ICP_WASM_OUTPUT_PATH"
       init_args: '(record { /* InitArgs */ })'
   ```

   Defer the exact build-step config to Task 2 (the recipe spike).

## Concerns / surprises

- **Per-deploy description for `rumi_protocol_backend`**: editing the yaml on every mainnet deploy is friction. Recommended workflow: keep `icp.yaml` set to a generic description, override on the CLI for each real deploy. Less ceremony than editing+committing yaml just for the description string.
- **Two sources of truth**: canister-level `init_args` and environment-level `init_args.<name>` are easy to mix up. Convention: canister-level = "fresh install on a brand-new replica", environment-level = "the actual mainnet/test reality, including upgrades." On `ic` environment we'll override every canister explicitly.
- **`icp project show` does not surface environment-level overrides** in its output; only the canister-level `init_args` is printed. This is mildly confusing for verification (the override does take effect on deploy, but you can't easily see it via `project show`). Cross-check with the env yaml directly.
- **Local-network `icp deploy --mode upgrade` is a no-op when the wasm is unchanged.** That's expected and matches dfx behavior, but worth flagging so reviewers don't think their upgrade ran when it actually skipped.
- **`init_args` is also passed on `install` mode if the canister already exists with a wasm**, but that path is governed by the `--mode` flag, not the yaml structure. Always pass `--mode upgrade` (or `--mode reinstall` if intentional and the canister state is throwaway) explicitly on mainnet deploys.

## Test artefacts

- Pattern A throwaway project: `/tmp/icp-cli-spike-upgrade-args/test_project/`
- Pattern B throwaway project: `/tmp/icp-cli-spike-pattern-b/test_project_b/`
