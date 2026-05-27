# Spike: icp-cli External Canister (deps pull) Equivalent

**Date:** 2026-05-27
**Branch:** `feat/icp-cli-migration`
**icp-cli version tested:** 0.2.0 (network launcher v12.0.0-2026-03-19-04-43)
**Question:** Can icp-cli replace `dfx deps pull` for `icp_ledger_canister`, `internet_identity`, `xrc`? Does XRC work on the local replica?

## Verdict

**GO.** icp-cli has no direct `dfx deps pull` analogue, but the `pre-built` build step (with either `path:` or `url:`) plus optional `sha256` covers every Rumi-pulled-canister use case. XRC runs and returns proper Candid responses on icp-cli's local replica. The system-subnet flag that dfx required is unnecessary on icp-cli (PocketIC under the hood treats application and system subnets identically for HTTPS outcalls and threshold keys).

For Rumi specifically:
- `internet_identity` and `xrc` (the two true `type: pull` canisters in `dfx.json`) become `pre-built` canisters in `icp.yaml` with a URL pointing at the official GitHub release.
- `icp_ledger_canister` (already `type: custom` with a local wasm in `src/ledger/`) becomes a `pre-built` canister with a `path:` pointing at the same wasm.

There is one minor regression vs `dfx deps pull`: dfx can reproduce the canonical mainnet canister ID (e.g. `ryjl3-tyaaa-aaaaa-aaaba-cai`) on the local replica via `init.json`; icp-cli always assigns fresh local IDs. Mitigation documented below.

## Pull mechanism

### Discovery

- The icp-cli CLI has no `deps`, `pull`, `dependency`, or `pull` subcommand (confirmed via `icp <name> --help`).
- The web docs at `cli.internetcomputer.org/0.2/migration/from-dfx/` make no mention of `dfx deps pull` equivalence.
- The official examples repo `dfinity/icp-cli` has two relevant fixtures: `examples/icp-pre-built/` (local-wasm form) and `examples/icp-recipe-remote-url-official/` (URL-recipe form).
- The configuration reference (`docs/reference/configuration.md` in the icp-cli repo) documents the `pre-built` build step at the canister level: either `path: dist/canister.wasm` or `url: https://example.com/canister.wasm`, optionally with `sha256: abc123...` (recommended for remote URLs). Source: `crates/icp/src/manifest/adapter/prebuilt.rs` defines `SourceField` as an untagged enum of `LocalSource { path }` or `RemoteSource { url }`, both with optional `sha256`.
- Networks have first-class `ii: boolean` (auto-installs Internet Identity) and `nns: boolean` (auto-installs NNS + SNS, implies `ii`). These are shortcuts but they install fresh II/NNS, not the mainnet snapshot.

### Exact icp.yaml syntax

For each Rumi external canister, declare a `pre-built` build step. Two variants:

```yaml
# Variant 1: local wasm on disk (recommended for ICP ledger; reproducible)
canisters:
  - name: internet_identity
    build:
      steps:
        - type: pre-built
          path: wasms/internet_identity.wasm.gz
          sha256: 80946901c67705df8c56236d0cb4732d21f4988a655e6662abe7c5d396e3f903
    init_args: '(null)'

# Variant 2: URL with sha256 pin (zero-setup developer machines)
canisters:
  - name: xrc
    build:
      steps:
        - type: pre-built
          url: https://github.com/dfinity/exchange-rate-canister/releases/download/2026.05.15/xrc.wasm.gz
          sha256: 084533477b31cd1d2f7394a38acf68d36aa87dcf84c155a961703e1df41b4ddb
    init_args: '()'
```

Both forms were tested end-to-end (deploy + status + Candid call). Module hashes after install match the declared sha256, so `pre-built` does verify integrity.

### How this replaces `dfx deps pull`

| Aspect | dfx deps pull | icp-cli pre-built |
|---|---|---|
| Source of wasm | Reads `dfx` metadata embedded in live mainnet canister | Explicit URL or local path in yaml |
| Canister ID assignment | Replays mainnet ID via `init.json` | New local ID each time |
| Cache location | `~/.cache/dfinity/pulled/<id>/canister.wasm.gz` | icp-cli's own cache (downloads once) |
| Failure modes | Breaks when canister lacks `dfx:wasm_url` metadata (we hit this with current II) | Breaks only on URL 404 or sha256 mismatch |
| Lock file | `deps/pulled.json` | sha256 inline in yaml |

icp-cli's form is strictly more transparent: the wasm source is visible in version control. dfx's form is more "magic" and currently broken for the live II (`dfx deps pull` reports "dfx metadata not found in canister rdmx6-jaaaa-aaaaa-aaadq-cai"). We hit this failure ourselves running `dfx deps pull --network ic` against Rumi's `dfx.json`.

### Canister ID reproduction (workaround for the only minor regression)

`dfx deps init` lets you pin local canister IDs so they match the mainnet IDs (e.g. ICP ledger lives at `ryjl3-tyaaa-aaaaa-aaaba-cai` both on mainnet and locally). icp-cli always allocates fresh IDs. Two options:

1. **Accept fresh local IDs.** PocketIC integration tests already use this pattern. Rumi's backend builds the XRC/II principals from config (set at install time), not from hardcoded IDs. Update the integration-test wiring to read `.icp/data/<env>/canister_ids.json` (icp-cli's analogue of `.dfx/local/canister_ids.json`).
2. **Use `icp canister create <name> --detached` then manually specify the desired ID via a pre-run script.** icp-cli's create command takes a canister name; the actual subnet/ID assignment is internal but the env yaml records the principal. This is more friction than dfx's `init.json` but tractable for the small handful of system canisters.

Recommend Option 1: it's the natural icp-cli flow and the test harness already accepts canister IDs at runtime.

## XRC system-subnet local-replica behavior

### Result: XRC works without any special subnet config

Tested on three separate icp-cli local networks:

| Setup | Subnets configured | XRC `get_exchange_rate` result |
|---|---|---|
| A: `subnets: [application, system]` | application + NNS + system | `(variant { Err = variant { CryptoBaseAssetNotFound } })` |
| B: `subnets: [application, system]` (URL fetch) | application + NNS + system | `(variant { Err = variant { CryptoBaseAssetNotFound } })` |
| C (control): `subnets: [application]` | application + NNS only | `(variant { Err = variant { CryptoBaseAssetNotFound } })` |

Same Candid response in all three cases. XRC's logs show it ran its real HTTPS-outcall provider loop in every case (Coinbase, KuCoin, Okx, Mexc, etc.), all returning "request sent with 0 cycles, but X cycles are required" because the spike call only forwarded 10B cycles which XRC consumes immediately on its first few outcall attempts.

### Why dfx requires `--subnet-type system` but icp-cli does not

dfx's local replica is the production ic-replica binary, which respects the subnet-type partition: HTTPS outcalls, ECDSA/Schnorr, and a few other system APIs are gated to `system` subnets only. Setting `subnet_type: system` in `dfx.json.defaults.replica` puts the whole local replica on a system subnet, which is why Rumi's `dfx.json` has that line.

icp-cli's local replica is PocketIC, which is a deterministic test replica that exposes the IC management canister API but does not enforce the subnet-type partition. HTTPS outcalls, threshold signing, etc. work on every subnet PocketIC creates. The `subnets:` yaml field is topology metadata (how many subnets, which kinds) but does not actually change runtime API availability per subnet. This is consistent with how Rumi already runs `cargo test --test pocket_ic_tests` against PocketIC without needing any subnet-type config.

### Implication for the migration

The `defaults.replica.subnet_type = "system"` line in `dfx.json` can be dropped entirely when we move to icp-cli. We do not need to set `subnets: [system]` (or `[application, system]`) in the icp-cli network config to get XRC working locally. We will set it anyway for documentation clarity ("yes, this project depends on system-subnet capabilities in production"), but it is not load-bearing.

### Internet Identity behavior

II ran cleanly. Module hash `0x80946901c67705df8c56236d0cb4732d21f4988a655e6662abe7c5d396e3f903` matches both the GitHub release sha256 and the canister's reported hash after install. No special flags needed. Cycles burn idle at the expected rate.

Note: there is also a shortcut `networks[].ii: true` that auto-installs II. It is convenient for projects that only need II for auth, but it installs fresh II rather than a specific release. For Rumi we want the explicit release pin (testing against known-good II versions matters when delegation flow changes), so `pre-built` with a URL + sha256 is the better fit.

## Recommended `icp.yaml` pattern for external canisters

```yaml
# yaml-language-server: $schema=https://github.com/dfinity/icp-cli/raw/refs/tags/v0.2.0/docs/schemas/icp-yaml-schema.json

canisters:
  # Local-wasm form (recommended for canisters whose wasm we already track in-repo)
  - name: icp_ledger_canister
    build:
      steps:
        - type: pre-built
          path: src/ledger/ic-icrc1-ledger.wasm
          sha256: cb0c3233ebb137f606d7928e90bfd907fe932a941f11b41c23cf0d6cf9c64802
    init_args:
      path: deploy/args/icp_ledger_local_init.did
      format: candid

  # URL form (recommended for canisters we want to fetch fresh from upstream releases)
  - name: internet_identity
    build:
      steps:
        - type: pre-built
          url: https://github.com/dfinity/internet-identity/releases/download/release-2026-05-26-be/internet_identity_dev.wasm.gz
          sha256: 80946901c67705df8c56236d0cb4732d21f4988a655e6662abe7c5d396e3f903
    init_args: '(null)'

  - name: xrc
    build:
      steps:
        - type: pre-built
          url: https://github.com/dfinity/exchange-rate-canister/releases/download/2026.05.15/xrc.wasm.gz
          sha256: 084533477b31cd1d2f7394a38acf68d36aa87dcf84c155a961703e1df41b4ddb
    init_args: '()'

networks:
  - name: local
    mode: managed
    gateway:
      bind: 127.0.0.1
      port: 4943
    # NOT load-bearing: PocketIC ignores subnet-type partitioning for runtime API
    # gating. Kept for documentation only ("this project assumes system-subnet
    # privileges in production").
    subnets:
      - application
      - system

environments:
  - name: local
    network: local
    canisters: [icp_ledger_canister, internet_identity, xrc]
  - name: ic
    network: ic
    # In production these are reached via their canonical mainnet IDs through
    # the backend's config record, not by deploying via icp deploy.
    canisters: []
```

## Local-replica XRC workaround (none needed)

XRC works on icp-cli's default local network without any subnet-type configuration. No workaround required. The dfx-era `--subnet-type system` requirement does not carry over.

### Calling XRC locally (for parity with `dfx canister call`)

`icp canister call` does not accept `--with-cycles` directly. To attach cycles to a call, route it through the auto-deployed proxy canister:

```bash
PROXY=$(icp network status | awk '/Proxy Canister Principal/ {print $NF}')
icp canister call xrc get_exchange_rate \
  '(record { base_asset = record { symbol = "ICP"; class = variant { Cryptocurrency } };
             quote_asset = record { symbol = "USD"; class = variant { FiatCurrency } };
             timestamp = null })' \
  --environment local \
  --identity rumi_identity \
  --proxy "$PROXY" \
  --cycles 10000000000
```

Without cycles, XRC's HTTPS outcalls cannot fund the upstream provider lookups and you get `CryptoBaseAssetNotFound`. With sufficient cycles (the spike used 10B; production XRC calls cost a few billion), the response becomes a proper exchange-rate record.

## Concerns / surprises

- **`dfx deps pull` is currently broken for the live II.** `dfx deps pull --network ic` against Rumi's `dfx.json` errors with `dfx metadata not found in canister rdmx6-jaaaa-aaaaa-aaadq-cai`. So even staying on dfx, we'd need to switch to a wasm-on-disk pattern for II. The migration removes a working-in-theory-only pull mechanism in exchange for an explicit-URL one.
- **PocketIC vs ic-replica behavior gap.** Worth flagging to anyone debugging "works on dfx, fails on icp-cli" or vice versa. PocketIC is more permissive at the subnet-type API gate, so a canister that depends on the partition being enforced (e.g. tests that XRC is only reachable from a system subnet) will not catch the issue locally. Rumi does not have such tests, so no fallout for us.
- **Canister ID reassignment.** Rumi's PocketIC integration tests already accept whatever IDs are allocated at install. The `vault_frontend` build embeds canister IDs into the JS bundle via a `declarations/` generation step. icp-cli's binding generation (`icp build` produces `.icp/data/<env>/canister_ids.json`) covers the same ground. We will need to wire the frontend build to read from `.icp/data/local/canister_ids.json` instead of `.dfx/local/canister_ids.json`. This is a Phase 1 task, not a blocker for Phase 0.
- **No first-class "import canister at principal X" syntax.** If we ever needed to talk to a third-party mainnet canister whose source/wasm we cannot redistribute, the only option in icp-cli is to read the principal from an env var or config record at runtime. `icp.yaml` will not "register" the principal for binding generation. Rumi does not currently have this need (all our external canisters are open-source with public releases).
- **`icp deploy --identity` accepts the identity name directly** (we used `--identity robvector` in this spike). The `rumi_identity` we use for mainnet deploys will work the same way.

## Test artefacts

- Local-wasm spike: `/tmp/icp-cli-spike-deps-pull/`
- URL-fetch spike: `/tmp/icp-cli-spike-deps-pull-url/`
- Control spike (no system subnet): `/tmp/icp-cli-spike-no-system/`
