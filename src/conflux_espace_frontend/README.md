# Conflux eSpace UI

This standalone UI defaults to the existing chain-71 staging configuration:

```sh
npm ci
npm test
npm run check
npm run build
```

The production canary is an explicit local/private build mode. It is not wired
into an ICP deployment recipe:

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
produce a deployment artifact. A future deployable artifact requires a
separately provisioned asset canister's exact certified origin:

```sh
VITE_PUBLIC_CANONICAL_ORIGIN="$REVIEWED_PUBLIC_ORIGIN" \
  npm run build:production-public:deploy
```

`REVIEWED_PUBLIC_ORIGIN` must be exactly
`https://<non-reserved-canister-principal>.icp0.io`. Management, anonymous,
non-canister, raw, `ic0.app`, custom-domain, IP, port, path, query, and fragment
values are rejected. Initial-launch custom-domain support is deliberately
deferred until one exact domain receives a separate review. If the variable is
unset, the build fails.

The deployment-shape verification build uses a denylisted syntactically valid
test Principal, writes only below the system temporary directory, scans the
bundle, and deletes it:

```sh
npm run verify:production-public:deploy-build
```

That test Principal is rejected by `build:production-public:deploy`, so the
verification artifact cannot become the deploy artifact.

The deployment command fails before emitting an artifact when the origin is
missing, malformed, not an exact certified canister origin, or denylisted.
`.ic-assets.production-public.json` is retained as the reviewed raw-access-off
policy for a future exact provisioning manifest; no current ICP recipe consumes
or deploys it.
At runtime, a build opened on any other certified gateway alias or domain shows
read-only status but blocks wallet connection and every write, with a link to
the canonical origin. It deliberately does not auto-redirect: automatic
navigation could discard user context and is unnecessary to enforce the wallet
boundary.

The explicit exact `icp0.io` canister origin is the deployment authority for
origin-scoped durable locks. Alternate certified/raw aliases remain blocked.

Only the no-default deployment build writes `dist-production-public`; verification
builds are ephemeral. Public mode uses the same pinned chain-1030 backend and
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

`icp.yaml` deliberately contains no public Conflux asset canister or environment.
Until an exact asset canister and canonical origin are separately provisioned and
reviewed, public artifact checks run only through the ephemeral npm/Vite commands
above. This prevents `icp project show` from auto-attaching an unprovisioned
recipe to the default `ic` environment.
