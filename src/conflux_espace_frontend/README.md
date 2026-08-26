# Conflux eSpace UI

This standalone UI defaults to the existing chain-71 staging configuration:

```sh
npm ci
npm test
npm run check
npm run build
```

The production canary is an explicit local/private build mode. It is not wired
into the public ICP deployment recipe:

```sh
npm run build:production-canary
npm run dev:production-canary
```

The production-canary build pins chain 1030, `https://evm.confluxrpc.com`,
`https://evm.confluxscan.io`, backend `tfesu-vyaaa-aaaap-qrd7a-cai`, and IcUSD
`0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff`. It accepts injected wallets only,
declares exactly 5 CFX collateral and 0.10 icUSD debt in the Open intent, and
removes borrow/withdraw controls. Every signature or EVM transaction still
requires an explicit click and wallet confirmation. Building does not deploy or
call production.

The browser persists the canary vault id, lifecycle phase, and submitted
transaction hashes per wallet. It writes a cross-tab safety lock before every
wallet request, so Open, deposit, burn, and close remain locked across reloads
and ambiguous provider responses. Confirmed wallet rejection safely restores
the prior phase. If a prompt is interrupted without a definitive result, the UI
first rechecks backend state and requires an explicit operator confirmation of
no authorization before it can clear that lock. Confirmed reverted or cancelled
EVM transactions expose an explicit retry. A semantic replacement remains
fail-closed because it may have moved funds; it advances only through backend
observation or the same explicit no-action recovery confirmation. Repricing
keeps the action locked under the replacement hash, and transient receipt errors
retry read-only polling. Any existing untracked vault is read-only, and either a
persisted lifecycle or any existing chain-1030 vault permanently disables a
second Open in this build.

The safe local verification build for public production mode is:

```sh
npm run test:production-public
npm run build:production-public
npm run dev:production-public
```

The local server embeds only `http://127.0.0.1:5174`; the local build uses an
ephemeral directory and deletes it after the structural scan. Neither path can
produce a deployment artifact.

The deployment-shape verification build uses a denylisted syntactically valid
test Principal, writes only below the system temporary directory, scans the
bundle, and deletes it:

```sh
npm run verify:production-public:deploy-build
```

That test Principal is rejected by `build:production-public:deploy`, so the
verification artifact cannot become the deploy artifact. The production bundle
scanner also rejects the canary lifecycle module, dev signer, chain-71 backend,
testnet RPC and IcUSD contract.

## Dedicated production-public asset recipe

`icp.yaml` defines `conflux_public_frontend` in the one-canister
`conflux-production-public` environment. It preserves the existing
`conflux_espace_frontend` / `mainnet-staging` recipe unchanged. The dedicated
recipe uploads only `dist-production-public/`; its build wrapper copies
`.ic-assets.production-public.json` to the asset canister's required
`.ic-assets.json` name, where every rule has `allow_raw_access: false` and the
catch-all enables SPA aliasing.

The canister-ID/origin bootstrap is deliberately two phase. Source control does
not contain a fake mapping or deployable public bundle before allocation.
`npm run build:production-public:deploy` deletes any stale output and fails
unless `.icp/data/mappings/conflux-production-public.ids.json` contains exactly
the one dedicated canister. It derives the only accepted origin as
`https://<mapped-principal>.icp0.io`; callers cannot supply or override it.
Management, anonymous, non-canister, raw, `ic0.app`, custom-domain, IP, port,
path, query, fragment, and the deterministic verification Principal are
rejected. Initial-launch custom-domain support remains deferred.

Run the source-only gate at any time:

```sh
cd src/conflux_espace_frontend
npm run verify:production-public:recipe
```

It validates positive and negative recipe/origin/policy/manifest fixtures and
performs a deployment-shaped build only in a temporary directory.

After a separate approval to create a mainnet canister and fund it with exactly
2T cycles, phase 1 is only the command below. `robvector` (Principal
`ft3ml-xex6k-ppiwj-ie6tc-zwkgb-ybm2x-eat4a-5p2jg-auzl3-latf4-aae`) is the
payer; its read-only preflight balance was `5_005_940_070_754` cycles. Refresh
that balance immediately before approval and require at least
`2_000_100_000_000` cycles, covering the 2T amount and current 100M
cycles-ledger update fee.

```sh
cd ../..
env ICP_ENVIRONMENT=conflux-production-public \
  /usr/local/bin/icp canister create conflux_public_frontend \
  -e conflux-production-public \
  --identity robvector \
  --controller fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae \
  --cycles 2t
```

`robvector` is only the payer and must not be a controller. The explicit sole
controller is `rumi_identity` at
`fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`. No subnet is
pinned, so icp-cli randomly selects an available application subnet. The
canister pays the creation charge from the attached 2T: currently 500B on a
13-node subnet or
approximately 1.308T on a 34-node subnet, leaving approximately 1.5T to 0.692T
before later usage.

If creation is ambiguous, do not retry. Reconcile the cycles-ledger result,
command output, and mapping first. After an unambiguous result, stop and require
exactly one mapped `conflux_public_frontend`; verify Running, the selected
application subnet, actual cycles, empty/uninstalled state, `rumi_identity` as
the sole controller, and `robvector` absent from controllers. Commit and
separately review the mapping and exact `https://<new-principal>.icp0.io` origin
before artifact construction. Creation performs no frontend build, release
seal, Wasm install, asset sync, or deploy.

Only after canonical-origin and manifest approval, construct the real artifact:

```sh
env ICP_ENVIRONMENT=conflux-production-public \
  /usr/local/bin/icp build conflux_public_frontend -e conflux-production-public
```

The recipe runs `npm ci` and the guarded build. Then create the deterministic
release seal (this does not contact the IC):

```sh
env ICP_ENVIRONMENT=conflux-production-public \
  npm run seal:production-public-release
```

The seal binds the exact `icp.yaml` recipe, mapping, canonical origin, manifest,
sorted asset inventory, and cached asset-canister Wasm hashes. Review
`dist-production-public/rumi-conflux-production-public.json`,
`dist-production-public/.ic-assets.json`, the complete asset hashes, and the
asset-canister Wasm hash; commit the mapping and
`.icp/data/reviews/conflux-production-public.release.json`, and approve the
printed release-seal sha256. Building and sealing still do not install or sync
anything.

Installation and asset sync require another explicit production-deployment
approval. Use the already built outputs so review and execution do not straddle
an unreviewed rebuild:

```sh
env ICP_ENVIRONMENT=conflux-production-public \
  npm run install:reviewed-production-public -- \
  --mode install \
  --approved-release-sha256 <exact-approved-release-seal-sha256>
```

The wrapper rechecks the exact ICP recipe, target, executable bundle, manifest,
every asset hash, and the explicit Wasm path before install and again before sync. A failed
install or any intervening drift prevents sync.

Never use `icp deploy`, the implicit `ic` environment, `mainnet-live`, or
`mainnet-staging` for this canister. For a later asset-canister upgrade use
`--mode upgrade`, never reinstall. After sync, verify the canonical certified
URL, exact deployed manifest and bundle, SPA fallback, raw-gateway refusal,
module hash, controllers, and cycles before calling the frontend deployed.
At runtime, a build opened on any other certified gateway alias or domain shows
read-only status but blocks wallet connection and every write, with a link to
the canonical origin. It deliberately does not auto-redirect: automatic
navigation could discard user context and is unnecessary to enforce the wallet
boundary.

The explicit exact `icp0.io` canister origin is the deployment authority for
origin-scoped durable locks. Alternate certified/raw aliases remain blocked.

Only the mapping-derived deployment build writes `dist-production-public`;
verification builds are ephemeral. Public mode uses the same pinned chain-1030 backend and
contract and is not the fixed-amount canary. It reads the backend's
anonymous `get_chain_public_launch_status` projection and exhausts the bounded
`list_chain_vaults_page` cursor before using vault inventory. Open, deposit,
borrow, and withdraw require full public readiness. Repay and debt-free close
remain available when readiness degrades, but only while the exact contract
binding, connected chain/address, complete inventory, and durable-lock checks
pass. It reads `get_expected_evm_nonce` immediately before every typed-intent
signature. A mainnet click can create at most one wallet prompt; nonce drift
requires a fresh review and click.

Production-public writes use a per-wallet cross-tab action lock persisted before
the wallet request. Signed backend actions and EVM deposit/burn transactions stay
locked across reloads until exact action-specific evidence resolves them. EVM
deposit/burn actions require a successful receipt plus the exact target vault
transition. Deposit specifically requires `AwaitingDeposit` to become
`MintPending` or `Open`; `Closing`, `Closed`, and unknown states remain locked.
Synchronous signed actions use their exact spend-on-success nonce;
Open never resolves from nonce movement alone. Ambiguous outcomes never invite
an automatic retry. Read-only vault and launch status remain available while
risk-increasing writes are paused.

The checked-in source is a provisioning/deployment path, not a deployed
frontend. Until the exact mapping/origin and manifest are reviewed, the only
valid public checks remain the ephemeral commands above. Chain 1030, backend
configuration, canister creation, asset installation/sync, and public
activation are separate approval and proof states.
