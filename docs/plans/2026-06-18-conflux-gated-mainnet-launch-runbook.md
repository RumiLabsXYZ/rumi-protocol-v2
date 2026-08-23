# Conflux eSpace mainnet public-launch runbook

Status: **production canary complete; public launch not active**

Last reconciled: 2026-08-22

Production backend: `tfesu-vyaaa-aaaap-qrd7a-cai`

Network: Conflux eSpace mainnet, chain `1030`

This runbook records the remaining public-launch gates. It deliberately treats
source, merge, deployment, configuration, enablement, and public availability
as different proof states. Passing one state never proves the next one.

## Current production truth

| Item | Current proof state |
|---|---|
| Backend | Production canister selected and running; threshold ECDSA key read back as `key_1` |
| Required trust anchors | Official EVM-RPC canister `7hfb6-caaaa-aaaar-qadga-cai` and chains threshold-ECDSA key `key_1`; the new consolidated status read-back still requires production deployment and verification |
| Canister EVM authority | Settlement/admin/minter address `0x00142f7ee842b171d539ec6053eaf88dd9a1adda` |
| IcUSD | Deployed at `0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff` in block `154846297`; deploy transaction `0x2513af548bf95a79cbb262338c08128d3cc96568feb2c5cce1d06f521f6a4383` |
| Binding | Production backend is bound to that IcUSD contract |
| Real-funds proof | Vault `#1` completed open, 5 CFX deposit, 0.10 icUSD mint, exact burn, and close; vault debt/collateral and IcUSD supply reconciled to zero |
| Chain status | Chain `1030` is **Disabled**. Timer entry checks keep its observer and settlement worker from starting new work; post-await checks also prevent a deposit transition or settlement broadcast after Disable |
| Public-readiness release | The status/gating implementation exists on the working branch; review/source-ready, merge, production deployment, and live API verification remain separate proof states |
| Public frontend | Public-mode source and fail-closed local verification commands exist, but no public asset canister or checked-in ICP deployment recipe currently exists. The existing `icp.yaml` frontend entry is not a production-public recipe |
| Public availability | **Not public.** Asset-canister provisioning, canonical-origin/manifest review, frontend deployment, and live verification remain undone; liquidation configuration/route/depth has not been verified live for public use; and the chain is disabled |

The completed canary proves one bounded lifecycle against the production
backend and contract. It does not prove public operational readiness, sustained
provider diversity, liquidation depth, frontend deployment, or activation.

## Proof-state vocabulary

- **Source-ready:** the required code and docs exist on a reviewed branch and
  deterministic/adversarial gates pass.
- **Merged:** the reviewed commit is present on the deployment branch.
- **Deployed:** the expected WASM or frontend asset hash is installed on the
  intended production canister.
- **Configured:** live production read-back matches the reviewed RPC, debt,
  price, contract, and liquidation configuration.
- **Enabled:** chain `1030` is `Registered`, so its workers and public write
  path may run.
- **Public:** the production frontend is deployed, monitoring is green, and
  enablement has been independently verified after the final approval.

Use these exact terms in handoffs. In particular, do not call a merged change
"live" and do not call the completed private canary "public."

## RPC agreement: what code proves and what an operator must prove

The runtime deduplicates configured endpoint URLs and refuses financial reads
when there are fewer distinct URLs than the configured floor. For a read with
`N` distinct URLs, the accepted value needs agreement from:

```text
max(configured_floor, floor(N / 2) + 1)
```

This is implemented by `endpoints_and_floor` and `tally_provider_outcomes` in
`src/rumi_protocol_backend/src/chains/evm/evm_rpc.rs:857` and
`src/rumi_protocol_backend/src/chains/evm/evm_rpc.rs:993`; the default floor
and override semantics are in
`src/rumi_protocol_backend/src/chains/config.rs:146` and
`src/rumi_protocol_backend/src/chains/config.rs:150`. The Conflux mainnet helper
currently sets a floor of `2` in
`src/rumi_protocol_backend/src/chains/evm/conflux/config.rs:23`. Therefore three
distinct configured URLs require matching responses from two; four URLs
require three; five URLs require three. The earlier blanket three-provider
floor was inaccurate and is retired.

URL deduplication and response agreement cannot prove provider independence.
Before activation, a human operator must verify that the configured endpoints
are controlled by at least two genuinely independent provider operators,
including operator and upstream/control-plane ownership. Multiple domains
backed by one operator do not satisfy this gate. Record the
provider-to-operator mapping in
the activation evidence without committing credentials or paid endpoint URLs.
The bounded public gate separately requires at least two distinct endpoint URLs
and an effective agreement requirement of at least two.

## Authoritative public risk gate

The frontend status display is not the authority for admitting debt. The
backend's bounded `conflux_mainnet_public_risk_blockers` predicate is enforced
inside both risk-increasing endpoints:

- `open_chain_vault_evm` checks it before spending the nonce and again after
  the threshold-ECDSA await, immediately before vault insertion; and
- `borrow_chain_vault_evm` checks it atomically with the borrow mutation and
  nonce increment;
- the bounded deposit observer rechecks Registered plus the same public gate
  after each custody-balance await, before changing `AwaitingDeposit` to
  `MintPending` or enqueueing a mint; and
- settlement rechecks Registered immediately before signing and again after
  signing, immediately before broadcast. A signature completed after Disable is
  discarded rather than broadcast.

The projection returned by `get_chain_public_launch_status(1030)` is the public
view of that same predicate. A frontend bug or stale UI cannot bypass the
backend Open/Borrow refusal.

## Remaining gates before public activation

All items are required unless explicitly marked optional.

1. **Merge and deploy the public-readiness release.**
   - Reviewed source is merged.
   - A fresh backend WASM is built from the merged commit.
   - Production upgrade completes with the expected module hash.
   - `get_chain_public_launch_status(1030)` and
     `get_expected_evm_nonce(1030, owner)` are callable and match the deployed
     Candid interface.

2. **Verify independent RPC configuration.**
   - The status API reports a distinct-endpoint count of at least two, an
     effective agreement requirement of at least two, and a sufficient
     configuration under the threshold formula above. It does not expose the
     endpoint URLs.
   - The human provider-independence review is recorded separately.
   - No endpoint credentials appear in a commit, terminal transcript, or
     launch report.

3. **Verify the fixed production shape while the chain remains Disabled.**
   - `effective_evm_rpc_principal` is exactly
     `7hfb6-caaaa-aaaar-qadga-cai` and
     `evm_rpc_principal_matches_expected` is true. This is the official
     EVM-RPC canister trust anchor; endpoint URL diversity is a separate gate.
   - `chains_ecdsa_key_name` is exactly `key_1` and
     `chains_ecdsa_key_matches_expected` is true. A derived address or an older
     standalone read-back is supporting evidence, not a substitute for the
     consolidated live status verdict.
   - Bound IcUSD is exactly
     `0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff`, and
     `icusd_contract_matches_expected` is true.
   - `finality_depth` is exactly `400`.
   - `burn_cursor` is greater than zero and has been seeded from a reviewed
     mainnet block.
   - `collateral_config_matches_expected` and
     `debt_config_matches_expected` are both true; present ratio/debt fields are
     supporting evidence, not substitutes for those exact-shape verdicts.

4. **Stage and validate liquidation while the chain remains Disabled.**
   - The chain-1030 liquidation row uses the reviewed WCFX/USDC route:
     - router `0x62b0873055bf896dd869e172119871ac24aea305`;
     - factory `0xe2a6f7c0ce4d5d300f97aa7e125455f5cd3342f5`;
     - pair `0x0736b3384531cda2f545f5449e84c6c6bcd6f01b`;
     - WCFX `0x14b2d3bc65e74dae1030eafd8ac30c533c976a9b`; and
     - USDC `0x6963efed0ab40f6c3d7bda44a05dcf1437c44372`.

     Swappi's [official contract registry](https://docs.swappi.io/swappi/about-swappi/swappi-contract-addresses)
     identifies the router and factory, and its
     [official pool/address list](https://docs.swappi.io/swappi/quick-reference/liquidity-pools-token-addresses)
     identifies the WCFX-USDC pair and token route. Record the source snapshot
     time in the activation evidence.
   - Stage the exact reviewed row below. This update is developer-gated and is
     valid while the chain is Disabled; because `enabled = true`, the setter
     also verifies the factory-derived pair before committing:

     ```bash
     icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
       set_chain_liquidation_config \
       '(1030 : nat32, record {
         dex = variant { UniswapV2 };
         router = "0x62b0873055bf896dd869e172119871ac24aea305";
         factory = "0xe2a6f7c0ce4d5d300f97aa7e125455f5cd3342f5";
         pair = "0x0736b3384531cda2f545f5449e84c6c6bcd6f01b";
         collateral_token = "0x14b2d3bc65e74dae1030eafd8ac30c533c976a9b";
         settle_stable_token = "0x6963efed0ab40f6c3d7bda44a05dcf1437c44372";
         slippage_cap_bps = 250 : nat16;
         restore_target_cr_e4 = 15_500 : nat64;
         enabled = true;
         max_swap_value_e8s = 200_000_000_000 : nat;
         max_price_age_ns = 1_800_000_000_000 : nat64;
         max_dex_oracle_divergence_bps = 500 : nat32;
         fee_bps = 25 : nat16;
         settle_stable_decimals = 18 : nat8;
         deadline_secs = 180 : nat64;
       })' -n ic --identity rumi_identity
     ```

     The exact values are the reviewed slippage, divergence, price-age, fee,
     deadline, decimals, recovery-target, and maximum-swap values.
     `liquidation_config_matches_expected` must be true, and
     `liquidation_config_digest` must be present and byte-for-byte equal to
     `expected_liquidation_config_digest`.
   - The factory-derived pair matches the configured pair.
   - The actual Swappi route is exercised or quoted against current mainnet
     liquidity, and the configured maximum swap is demonstrably inside the
     accepted depth/slippage envelope.
   - Set the row's `enabled` field to true while chain `1030` itself is still
     Disabled. The row is then activation-ready without admitting Open/Borrow
     or starting a new observer/settlement tick.
   - Source or mock-router tests alone do not satisfy this live-liquidity gate.
   - Refresh the cached settlement gas balance after the reviewed RPC and route
     configuration is staged, while the chain is still Disabled. This is a
     developer-only production update; it reads the chain balance and updates
     only the cached balance and timestamp:

     ```bash
     icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
       refresh_chain_hot_wallet_balance '(1030 : nat32)' \
       -n ic --identity rumi_identity
     ```

     Record an unambiguous `Ok` response, then promptly read status. Do not treat
     a nonzero balance alone as current evidence. Require
     `hot_wallet_ready = opt true`, a nonzero
     `hot_wallet_balance_refreshed_at_ns`, a present
     `hot_wallet_balance_age_ns` no greater than
     `hot_wallet_balance_max_age_ns`, and
     `hot_wallet_balance_is_fresh = true`. The monitor itself remains anonymous
     and query-only; it never performs this refresh.

5. **Deploy and verify the public frontend.**
   - First provision a dedicated public asset canister. No public Conflux asset
     canister or environment is currently checked into `icp.yaml`, and no
     checked-in ICP recipe consumes the production-public bundle or its asset
     policy. Do not treat the existing staging/default
     `conflux_espace_frontend` entry as a public deployment recipe.
   - After provisioning, review and pin the exact canonical certified origin as
     `https://<non-reserved-canister-principal>.icp0.io`. Review the final ICP
     provisioning/deployment manifest separately, including its consumption of
     `.ic-assets.production-public.json` and the raw-access-off policy. Canister
     provisioning, canonical-origin approval, manifest approval, artifact
     construction, and deployment are separate proof states.
   - Until those prerequisites exist, only the README's local/ephemeral public
     verification commands are valid:

     ```bash
     cd src/conflux_espace_frontend
     npm run test:production-public
     npm run build:production-public
     npm run dev:production-public
     npm run verify:production-public:deploy-build
     ```

     These commands cannot deploy. The build verifiers write only to temporary
     directories and remove their output; the local Vite server is fixed to
     `127.0.0.1:5174`; and the deployment-shape verifier uses a denylisted test
     Principal. Do not claim that they produce a deployable artifact or that
     `npm run build:production-public:deploy` is authorized before exact
     provisioning/origin/manifest review.
   - It targets production backend `tfesu-vyaaa-aaaap-qrd7a-cai`, chain `1030`,
     and the bound IcUSD contract above.
   - It shows live debt limits and launch blockers from the status API.
   - It fails closed when `public_open_ready` is false.
   - Mainnet uses an injected wallet only; every signature or transaction
     remains an explicit user action, and nonce ambiguity never triggers an
     automatic second signature.

6. **Complete the bounded vault inventory.**
   - Use `list_chain_vaults_page(1030, cursor, scan_limit)` and continue with
     each `next_start_after` until `done = true`.
   - Record every page and its `scanned_count`. The cursor advances over the
     global vault map, so an empty chain-1030 page is not terminal unless
     `done = true`.
   - Do not use legacy `list_chain_vaults` as proof of a complete inventory; it
     is compatibility-only and capped at 500 returned matches.

7. **Make the read-only monitor green.**

   While staging with the chain disabled:

   ```bash
   scripts/check-conflux-public-launch.sh --expect-disabled
   ```

   After the final enable action:

   ```bash
   scripts/check-conflux-public-launch.sh --expect-public-active
   ```

   The monitor must fail closed on unavailable/malformed queries, unexpected
   chain status, wrong official EVM-RPC principal, wrong threshold-ECDSA key,
   wrong IcUSD binding, collateral/debt/liquidation shape mismatch, absent or
   unequal liquidation digest, fewer than two distinct endpoint URLs,
   agreement below two, finality other than 400, an unseeded burn cursor,
   stale/missing price, any breaker/halt, insufficient cycles,
   unknown/low/stale hot-wallet evidence, or an inconsistent operator supply
   audit. Public-active mode additionally requires the backend's consolidated
   `public_open_ready` verdict to be true.

   `get_chain_public_launch_status` deliberately contains no vault-map,
   settlement-queue, or burn-key scans. The monitor calls `get_supply_audit`
   separately and labels it an **operator internal supply audit**; it verifies
   the reported total equals the per-chain sum and that chain 1030 agrees with
   the bounded status. This is not a live EVM `totalSupply()` outcall. Retain a
   separate, recent on-chain supply reconciliation result in the activation
   evidence.

   Disabled status is a refusal/freeze boundary, not by itself proof that the
   system is already quiet. The deposit observer is bounded to at most 500
   global-map examinations and 25 balance polls per tick, and its post-await
   Registered/risk-gate check prevents a late deposit RPC from enqueueing a mint
   after Disable. Settlement rechecks Disable before signing and before
   broadcast, but a transaction broadcast before Disable can still settle on
   chain. The current `chain_has_active_settlement_op` query scans the queue and
   therefore is not the bounded complete-queue proof required for a launch
   assertion. Do not invent a “no active/queued work” claim from it. Use the
   complete paged vault inventory, submitted-transaction receipts, and explicit
   reconciliation of any known work; add a bounded settlement-inventory API
   before making complete queue emptiness a mandatory proof.

8. **Obtain fresh explicit activation approval.**
   - Present the merged commit, deployed module hash, frontend target/hash,
     redacted provider-independence evidence, liquidation route/depth evidence,
     and green disabled-stage monitor output.
   - Approval is for the exact activation action only. Deployment,
     configuration, enablement, and future cap changes are separate authority.

9. **Enable once, then reconcile without an ambiguous retry.**
   - Perform the approved chain-1030 enable action exactly once.
   - If the update result is ambiguous, do not retry. Reconcile live status and
     events first.
   - Require the public-active monitor to pass before announcing public
     availability.

A dedicated chains canister may still be useful for future isolation, but it is
optional architecture work. Production `tfesu-vyaaa-aaaap-qrd7a-cai` has
already been selected for this launch, so a dedicated canister is not a public
activation gate.

## Stop conditions and recovery posture

Stop and keep (or return) chain `1030` to Disabled if any of the following is
observed:

- status API unavailable or malformed;
- effective EVM-RPC principal differs from `7hfb6-caaaa-aaaar-qadga-cai`, or
  the chains threshold-ECDSA key differs from `key_1`;
- bound IcUSD differs from the exact production contract;
- RPC endpoint/agreement configuration insufficient or provider independence
  no longer defensible;
- finality depth differs from 400, the burn cursor is zero, or the reviewed
  collateral/debt shape verdicts are false;
- missing or stale CFX price;
- invariant, reorg, bad-debt, or protocol freeze breaker;
- malformed or inconsistent operator internal supply audit;
- low/unknown canister cycles or low/unknown/stale settlement hot-wallet
  evidence;
- liquidation configuration missing, disabled, mismatched, digest-mismatched,
  or no longer inside measured Swappi depth;
- unexpected deployed module/frontend hash; or
- ambiguous fund-affecting or activation result that has not been reconciled.

Disabling prevents new observer/settlement ticks, and post-await checks prevent
late custody reads or threshold signatures from crossing the state/broadcast
boundary. It does not cancel a transaction that was already broadcast, prove
that every queue is empty, or erase queued work; unresolved work remains paused
until re-enabled and already-broadcast work must be reconciled on chain. Do not
treat Disable as a harmless UI switch or as instant proof of quiescence. Take a
production snapshot before a backend upgrade and retain the previously approved
snapshot-restoration contingency for a critical post-upgrade failure.
Restoration itself remains an operator-controlled production action.

## Evidence packet for the launch record

Capture, redact, and retain:

- merged commit and green CI checks;
- backend WASM hash and production module-hash read-back;
- public frontend asset hash and public URL;
- `get_chain_public_launch_status(1030)` output immediately before and after
  enablement;
- exact effective EVM-RPC principal plus its match verdict, and exact
  threshold-ECDSA key name plus its match verdict;
- the successful `refresh_chain_hot_wallet_balance(1030)` response plus the
  subsequent refreshed-at/age/max-age/freshness status fields;
- equal actual and expected liquidation-configuration digests;
- `cycles_status()` output;
- complete `list_chain_vaults_page` inventory through `done = true`;
- `get_supply_audit()` operator internal audit output;
- recent on-chain IcUSD supply reconciliation output, separately from the
  operator internal supply audit;
- provider-to-operator independence mapping (no credentials);
- Swappi factory/pair/route and live depth/slippage evidence;
- disabled-stage and public-active monitor outputs;
- exact activation approval; and
- the single enable result plus event/status reconciliation.

Only after that packet is complete may the state be reported as **public**.
