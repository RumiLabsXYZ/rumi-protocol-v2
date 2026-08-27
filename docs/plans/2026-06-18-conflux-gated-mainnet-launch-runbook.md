# Conflux eSpace mainnet public-launch runbook

Status: **production canary complete; public launch not active**

Last reconciled: 2026-08-24

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
| Public frontend | Dedicated `conflux_public_frontend` canister `a52ri-naaaa-aaaas-qgy4a-cai` is allocated in `conflux-production-public`, Running and uninstalled with `rumi_identity` as sole controller. Its checked-in mapping derives canonical origin `https://a52ri-naaaa-aaaas-qgy4a-cai.icp0.io`. No deploy artifact, install, or asset sync exists yet. The chain-71 staging recipe remains separate |
| Public availability | **Not public.** Deploy-manifest review, artifact construction, installation/sync, and live verification remain undone; liquidation configuration/route/depth has not been verified live for public use; and the chain is disabled |

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
   - For the frozen gzip backend artifact, run
     `scripts/verify-rumi-backend-upgrade-artifact.sh` before the upgrade. Its
     uploaded gzip SHA-256 is
     `14d65746d2d801347ecdb24dc54611b12cb3cca8765f5bf80f929751f1eda287`;
     this is the exact value required from `canister_status.module_hash` both
     while stopped after install and after restart. The locally decompressed
     Wasm content SHA-256 is
     `44d13c58f20d53dda91030f2c6c038e9db976b5e83cd2cb019b56219b744654e`;
     retain it as semantic-content/build-reproducibility evidence only and
     never use it as the live `module_hash` expectation. Pass the read-back to
     the verifier with `--live-module-hash <exact-status-hash>`.
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
   - Stage the bad-debt circuit at exactly `10_000_000` e8s (`0.10 icUSD`, one
     minimum-size vault debt) while chain `1030` remains Disabled. Missing,
     zero, lower, or higher values fail the authoritative public-readiness
     gate. Use the developer-gated setter and retain its unambiguous `Ok`
     response:

     ```bash
     icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
       set_chain_bad_debt_circuit_threshold \
       '(1030 : nat32, opt (10_000_000 : nat))' \
       -n ic --identity rumi_identity
     ```

     Then require `bad_debt_threshold_e8s = opt (10_000_000 : nat)` in the
     consolidated status response and exact-value PASS from the read-only
     monitor. Mere presence of a nonzero threshold is not sufficient.

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

   #### 2026-08-26 bounded Disabled-cursor reseed approval artifact

   This subsection and the credential-free observation record at
   `docs/plans/2026-08-26-conflux-disabled-cursor-reseed-evidence.md` are the
   complete readable approval artifact for one bounded production recovery.
   They authorize no action by themselves. The requested approval covers
   exactly three update calls, in order: (1) one cursor reseed, (2) one exact
   enabled liquidation-config setter, and (3) one non-state-mutating supply
   reconciliation outcall. Every readback and expiry gate between them is
   mandatory. Chain `1030` must remain Disabled throughout. No retry, reversal,
   or other update is included.

   The bad-debt threshold was staged successfully with an unambiguous `Ok` and
   read back as exactly `10_000_000` e8s, with bad debt zero and the circuit
   untripped. The exact liquidation setter above was then called twice. Both
   calls returned an explicit
   `factory pair sanity getPair failed: eth_call RPC error` with JSON-RPC code
   `-32016` and message `state is not ready`; both failures were reconciled as
   no-mutation outcomes (`get_chain_liquidation_config(1030) = null`, no digest,
   chain still Disabled). A subsequent single `reconcile_chain_supply(1030)`
   outcall failed explicitly with the same `-32016` while reading
   `totalSupply()`. It returned no reconciliation report and changed no stable
   state. No further setter or reconciliation retry is permitted before the
   cursor recovery below.

   The causal evidence is a stale burn cursor plus historical-state retention,
   not a wrong chain, contract, route, EVM-RPC trust anchor, or a transient
   chain failure:

   - the exact pre-recovery cursor is `154_966_240`;
   - Disabled chains are omitted from observer dispatch, so this cursor stopped
     advancing while the chain remained closed;
   - liquidation validation derived block `154_967_264` from that cursor and
     proved finality by reading block `154_967_664`; supply reconciliation read
     exactly at cursor `154_966_240`;
   - all three configured provider paths returned chain ID `1030` and matching
     historical block headers. Confura returned the canonical pair and zero
     IcUSD supply at those old blocks, while BlockPI and Unifra independently
     returned the same `-32016` for historical `eth_call` state. Their two
     matching errors correctly met the configured two-of-three fail-closed
     agreement requirement; and
   - all three provider paths subsequently agreed on recent state. No endpoint
     credential, API key, paid URL, header, or secret is part of this artifact.

   The first approved recovery target (`T = 155_237_900`) expired before call
   one and was never used. Its immediate execution gate still found matching
   headers from all three providers, but BlockPI and Unifra both returned
   explicit `-32016 state is not ready` errors for the required pair/supply
   state reads at `T` and the required pair read at `C`; only Confura succeeded.
   That was below the required two-provider floor, so execution stopped before
   `set_last_observed_block`. No cursor, liquidation row, reconciliation state,
   chain status, endpoint, quorum, price, hot-wallet cache, frontend, Stability
   Pool, or funds mutation occurred. The old target and its approval are void.

   The exact reviewed recovery target is:

   ```text
   T = 155_249_500
   F = T + 400 = 155_249_900
   T header hash = 0xfdb455b5e2e8fb8fc20d58709f98d94a082232bd105d83786afda7205ea9f2ef
   F header hash = 0x47307952a58607bcd5cf6af9078bf10abc842b8a926d57ed7bda632da29b8086
   factory getPair(WCFX, USDC) at T = 0x0736b3384531cda2f545f5449e84c6c6bcd6f01b
   IcUSD totalSupply() at T = 0
   ```

   Confura, BlockPI, and Unifra independently returned the identical target
   header, canonical pair, and zero supply. All three also returned the header
   for `F`, proving `T` was buried by the configured `400`-block finality depth
   when this artifact was prepared.

   The cursor setter stores `T`, but the liquidation setter then derives a
   different pinned factory-read block when its finality probe succeeds. Freeze
   that branch as well:

   ```text
   C = T + 1_024 = 155_250_524
   P = C + 400 = 155_250_924
   C header hash = 0xfa50433742b5f87654563cb3d8edc3c22d35900ae4a0ff0b05cfb6e8ceafd1fd
   P header hash = 0xff2f7eba0c362295a051f682a39bbeb6054fb126d2d017d0e65c3d662a547592
   ```

   Before approval and again immediately before execution, require `P` already
   exists with matching headers from at least two configured independent
   providers, require the same agreement for `C`, and require factory
   `getPair(WCFX, USDC)` at `C` to return the canonical pair above. This removes
   the timing-dependent branch: with `P` present, `fetch_block_numbers` returns
   `C`, so the liquidation setter validates at `C`. Supply reconciliation still
   reads exactly at stored cursor `T`; the fetch does not persist `C`.

   This evidence is time-sensitive. Immediately before execution, revalidate
   `T`, `F`, `C`, `P`, chain ID `1030`, the exact `T` header hash, canonical pair
   at both `T` and `C`, and zero IcUSD supply at `T` against the configured
   provider paths. This approval expires without execution if fewer than two
   configured independent providers agree, if `P` does not yet exist, if any
   required block/state read is unavailable, or if any immutable evidence or
   unchanged safety invariant below has drifted. The cursor and liquidation row
   may change only through the three authorized calls and are not drift when
   they exactly match the next phase-specific state gate. Before each call,
   re-run the immutable provider/block matrix and every unchanged safety
   invariant, then apply that call's phase-specific gate. Do not replace `T`,
   `C`, or `P` under this approval; different blocks require a new readable
   artifact and approval.

   `set_last_observed_block` is developer-gated but otherwise accepts any
   `nat64`: it does not validate chain status, finality, monotonicity, vaults,
   supply, or settlement work. Its source contract also states that events
   before the seed are not scanned. Therefore every item below is required
   immediately before the first (cursor-reseed) call. Items other than the
   phase-specific cursor/row state are also rerun before calls two and three:

   1. Production backend is Running live module
      `14d65746d2d801347ecdb24dc54611b12cb3cca8765f5bf80f929751f1eda287`,
      has at least `5_000_000_000_000` cycles, and has exactly these controllers:
      `cpbhu-5iaaa-aaaad-aalta-cai`,
      `mi66c-zqlu4-4kxd6-2gtp7-szg5v-6a62a-geoty-fahu5-4trje-xyfby-wqe`, and
      `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`.
      Effective EVM-RPC principal is exactly
      `7hfb6-caaaa-aaaar-qadga-cai` with `overridden = false`; threshold-ECDSA
      key is exactly `key_1`; bound IcUSD is exactly
      `0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff`; protocol mode is
      GeneralAvailability and not frozen; invariant halt, chain reorg halt, and
      bad-debt circuit are false; bad debt is zero; and the bad-debt threshold
      is exactly `10_000_000` e8s.
   2. Chain `1030` is still Disabled and its cursor is exactly
      `154_966_240`; the liquidation row and digest are still absent. Any other
      first-call state expires this approval.
   3. The complete bounded `list_chain_vaults_page` traversal reaches
      `done = true` and contains only Vault `#1`, `Closed`, with zero debt,
      collateral, and pending mint.
   4. Internal chain supply, global supply, reserve backing, pending burn, bad
      debt, and in-flight mint are all zero. IcUSD `totalSupply()` is zero at
      both the old cursor and `T` on the archive-capable path, and zero at `T`
      from at least two configured independent providers.
   5. The canary deposit, mint, burn, close/withdrawal, and every submitted
      transaction receipt are fully reconciled. There is no known queued,
      in-flight, signed-but-unbroadcast, broadcast-but-unconfirmed, or otherwise
      unresolved chain-1030 settlement work. The current
      `chain_has_active_settlement_op = false` query is supporting evidence only,
      not a complete queue-inventory proof.
   6. The canister remains the intended IcUSD admin/minter, with no unexpected
      role grant or revocation evidence.
   7. The current redacted operator configuration still maps the exact three
      credential-free public paths recorded in the dated evidence file to
      Confura, BlockPI, and Unifra, and the provider-independence review remains
      valid. The `T`/`F` and mandatory `C`/`P` provider agreement revalidation
      immediately above passes without endpoint or quorum changes.

   If and only if every precondition passes and the exact action receives
   explicit approval, call once:

   ```bash
   /usr/local/bin/icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
     set_last_observed_block \
     '(1030 : nat32, 155_249_500 : nat64)' \
     -n ic --identity rumi_identity
   ```

   Require an unambiguous `Ok`, then read back before any other update:

   ```bash
   /usr/local/bin/icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
     get_last_observed_block '(1030 : nat32)' \
     -n ic --identity rumi_identity --query
   ```

   The only accepted readback is `(155_249_500 : nat64)`. If the update result
   is ambiguous, query first: that exact value means the call landed and must
   not be repeated; any other value means stop and reconcile without a blind
   retry. A wrong landed value is mechanically reversible to the exact
   pre-cursor only while the chain remains Disabled and before any subsequent
   mutation, observer work, or settlement action. Such a reversal requires its
   own approval and does not undo skipped history. Once any subsequent work has
   occurred, never rewind the cursor.

   After the exact target readback, and only while all approval evidence remains
   current, perform the other two calls covered by this same bounded approval:

   1. Re-run the immutable provider/block matrix and every unchanged safety
      invariant, including `C`/`P` existence/agreement and the canonical pair at
      `C`. For this second-call phase, require cursor exactly `155_249_500`,
      chain still Disabled, and the liquidation row/digest still absent. Then
      call the exact enabled liquidation setter already printed in this Step 4
      once. Require unambiguous `Ok`; on ambiguity, query the row and digest
      before considering any further action. Exact matching row plus the
      expected digest means it landed and must not be repeated; any other result
      means stop. Require digest
      `d7ff0d667b867f4cb3fbccabd57c05911d17eee6888a5df58e81daf8954f4f1d`.
   2. Re-run the immutable provider/block matrix and every unchanged safety
      invariant, including status, supply, inventory, and known-work checks.
      For this third-call phase, require cursor exactly `155_249_500`, chain
      still Disabled, and the exact liquidation row plus digest above present.
      Then call once:

      ```bash
      /usr/local/bin/icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
        reconcile_chain_supply '(1030 : nat32)' \
        -n ic --identity rumi_identity
      ```

      Require finalized block `155_249_500`, on-chain supply zero, recorded
      supply zero, in-flight mint zero, gap zero, and
      `unbacked_excess = false`.
   3. Repeat the complete bounded vault inventory, internal supply audit,
      consolidated launch status, provider/route readbacks, and Disabled monitor.
      Chain `1030` must remain Disabled.

   Do not change RPC endpoints, quorum floor, trust anchors, contract binding,
   price, hot-wallet cache, frontend, Stability Pool, funds, or chain status as
   part of this recovery. Runtime source changes and broader archive-provider or
   checked-reseed work are optional resilience follow-up, not prerequisites for
   this exact bounded approval.

   #### Durable one-shot successor after both fixed targets expired

   The two fixed-target approvals above are historical evidence only. The first
   (`T = 155_237_900`) and replacement (`T = 155_249_500`) each expired before
   call one when BlockPI and Unifra stopped serving the required historical
   state. In both cases the operator stopped at the immediate provider gate and
   made **zero** cursor, liquidation, reconciliation, or other live mutations.
   Do not execute either fixed packet.

   The source-only successor is
   `scripts/conflux-disabled-recovery.py`. Its authority model is explicit:
   approval delegates one execution of the reviewed deterministic procedure;
   it does **not** pre-approve literal per-run call bytes. The script freezes
   those bytes and a canonical manifest at execution time. If governance
   requires literal-byte approval instead, run the default dry-run, approve its
   sealed manifest separately, and do not use the procedure-bound `--execute`
   authority described here. The procedure is **source-ready, not live-ready**
   until production is separately upgraded to the reviewed digest-bearing
   backend artifact, that exact deployed module hash is supplied through
   `--approved-module-sha256`, and the replicated endpoint-digest readback
   below passes. The old production module
   `14d65746d2d801347ecdb24dc54611b12cb3cca8765f5bf80f929751f1eda287`
   cannot satisfy this contract. Building, stopping, snapshotting, installing,
   starting, or restoring the backend is outside this recovery procedure and
   requires separate approval.

   Dry-run/preflight is the default. Before selection, and again before each of
   the three update phases, the runner calls
   `get_chain_rpc_endpoint_set_digest(1030)` as `rumi_identity` through
   replicated ingress execution (`icp canister call` with **no** `--query`
   flag). It strictly requires `Ok`, chain `1030`, endpoint count `3`, effective
   quorum `2`, and the canonical V1 digest
   `c57af2bf81cf047aeeb94ecd463f612263abbcc3de93f1ec143488f32c951f31`.
   That digest is calculated from the exact UTF-8 Confura/BlockPI/Unifra URLs
   under the merged V1 contract; URLs are never accepted as execute-mode
   overrides or printed in the sanitized transcript. Each successful readback
   and raw-response hash is sealed into the manifest/journal evidence. Any
   `Unauthorized`, missing method, malformed response, ordinary-query path,
   endpoint/count/quorum drift, or digest mismatch stops before dispatch.

   Selection then makes one no-retry head batch and four bounded no-retry
   matrix batches (no batch exceeds four entries) to each exact configured
   public path (Confura, BlockPI, and Unifra),
   requires all three chain IDs to equal `1030`, all three head timestamps to
   be no more than 300 seconds old, the whole sample to finish within 60
   seconds, and head skew to be at most 128 blocks. It then freezes:

   ```text
   H = min(Confura head, BlockPI head, Unifra head)
   T = H - 2_048
   F = T + 400
   C = T + 1_024
   P = C + 400 = H - 624
   ```

   Arithmetic is checked for underflow and ordering. `T` must be strictly above
   the exact starting cursor `154_966_240` and no more than `1_000_000` blocks
   above it. At selection, all three providers must return identical canonical
   headers at `H/T/F/C/P`, the pinned WCFX/USDC pair at both `T` and `C`, zero
   IcUSD `totalSupply()` at `T`, and true admin/minter role readbacks for the
   canister settlement address. Confura must additionally return zero supply at
   the old cursor. These values, provider attribution, timestamps, raw-response
   hashes, exact canister/network/identity/module/controller anchors, the clean
   approved Git commit/tree, the three approved file hashes, the pinned
   Python/`icp`/`didc`/`git`/`curl` binary fingerprints and tool versions, and the exact
   `didc`-serialized Candid argument bytes enter one mode-0600 sealed manifest.

   Before **each** update phase, the script makes a new no-retry fixed-matrix
   request to each provider and repeats the entire live Disabled/zero-state
   gate: exact approved digest-bearing module/controllers/cycles floor, chain
   status and cursor for that phase, true
   `collateral_config_matches_expected` and `debt_config_matches_expected`, all
   other anchor/config projections, complete one-page Vault `#1` Closed/zero
   inventory, zero internal/global/on-chain supply, reserve, pending burn, bad
   debt and pending mint, exact threshold/untripped breakers, and empty active
   settlement/proof/burn inventories. At least two configured independent
   providers must return the complete exact positive matrix. An unavailable
   third provider is recorded. Every successful field is checked against the
   frozen expectation before that provider can be classified unavailable: any
   observed successful conflict invalidates the run even when a different
   field from that provider is missing/error and the other two providers
   agree. The canary/every-receipt and broader no-unresolved-work proof remains
   bound to the approved evidence plus these live supporting queries because
   the backend exposes no complete global queue-inventory query.

   Only these three methods and their frozen argument bytes are dispatchable,
   once each and in this order: `set_last_observed_block(1030, T)`, the exact
   enabled liquidation row printed above, and
   `reconcile_chain_supply(1030)`. The script journals `dispatching` with an
   atomic `fsync` before every call, caps the run at three updates, never retries,
   never reverses, never automatically resumes, and leaves chain `1030`
   Disabled. Ambiguous call one may be classified landed only by cursor `T`;
   ambiguous call two only by the complete exact row plus digest
   `d7ff0d667b867f4cb3fbccabd57c05911d17eee6888a5df58e81daf8954f4f1d`.
   An ambiguous reconciliation has no durable landed marker and permanently
   stops that execution unresolved even if all zero-state readbacks remain safe.

   Both dry-run and execution require a clean checkout whose `HEAD` exactly
   equals the approved merge commit plus the three approved file SHA-256 values
   and exact deployed module hash. Run the post-upgrade read-only preflight by
   omitting `--execute` and `--execution-id` from the command below. Execute
   only with separate procedure authority and the literal one-use ID:

   ```bash
   python3 scripts/conflux-disabled-recovery.py \
     --execute \
     --execution-id conflux-disabled-recovery-v1 \
     --approved-commit APPROVED_MERGE_SHA \
     --approved-script-sha256 APPROVED_SCRIPT_SHA256 \
     --approved-runbook-sha256 APPROVED_RUNBOOK_SHA256 \
     --approved-evidence-sha256 APPROVED_EVIDENCE_SHA256 \
     --approved-module-sha256 APPROVED_DEPLOYED_DIGEST_MODULE_SHA256
   ```

   The same module argument is mandatory in default dry-run mode. Before the
   separately approved backend upgrade, default dry-run is expected to stop
   fail-closed because the digest method/module binding is not live. Do not
   weaken that expected failure into a source-readiness pass, and do not run a
   live dry-run until the upgrade owner authorizes post-deployment verification.

   The one-use journal lives under the operator's local state directory. A
   pre-existing execution directory makes every rerun fail. This prevents
   resend on the same host/state directory; it is not a global CAS across
   copied checkouts or different hosts. The production canister has no recovery
   nonce, so cross-host exact-once remains an explicit residual. Raw responses
   remain private mode `0600`; the sanitized transcript contains only
   allowlisted values, hashes, timestamps, phases, and verdicts. No endpoint,
   quorum, status, price, hot-wallet, frontend, Stability Pool, funds, build,
   install, deploy, or activation action is in this procedure.

   - Rebaseline the CFX/USD price while chain `1030` remains Disabled, after the
     liquidation row is staged and before the final freshness/monitor checks.
     The price is an execution-time market value and **cannot be frozen in
     source**. Fetch the existing monitor's three default sources — CoinGecko
     `conflux-token/USD`, Kraken `CFXUSD`, and OKX `CFX-USDT` — and retain each
     raw response plus its request/response timestamp (and the source timestamp
     when the response supplies one). Apply the monitor's existing defaults
     exactly: discard non-finite/non-positive quotes, require at least two valid
     sources, reject quotes more than 5% from the provisional median, require at
     least two survivors with spread no greater than 3%, then recompute the
     survivor median. Convert that USD median to e8 with
     `PRICE_E8 = round(median_usd * 100_000_000)`, matching `usdToE8`; if the
     aggregate refuses, do not write a price. Replace the literal `PRICE_E8`
     token in the command below with that positive unsigned integer.

     Before writing, query the current value and record its `set_at_ns` as
     `PRE_WRITE_SET_AT_NS` (`0` if the getter returns `null`). This timestamp is
     the proof that a same-rounded-price write actually landed:

     ```bash
     icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
       get_manual_collateral_price '(1030 : nat32, "CFX")' \
       -n ic --identity anonymous --query
     ```

     ```bash
     icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
       set_manual_collateral_price \
       '(1030 : nat32, "CFX", PRICE_E8 : nat64)' \
       -n ic --identity rumi_identity
     ```

     Require an unambiguous `Ok`, then immediately query both the exact price
     readback and the consolidated launch status:

     ```bash
     icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
       get_manual_collateral_price '(1030 : nat32, "CFX")' \
       -n ic --identity anonymous --query

     icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
       get_chain_public_launch_status '(1030 : nat32)' \
       -n ic --identity anonymous --query
     ```

     Require the getter's `price_e8` to equal `PRICE_E8`, its `set_at_ns` to be
     strictly greater than `PRE_WRITE_SET_AT_NS`, the consolidated
     price/timestamp to match that readback, `collateral_price_is_fresh = true`,
     and price age no greater than the configured `1_800_000_000_000ns`
     (30-minute) limit. If the update result is ambiguous, do not blindly retry:
     query those readbacks first; treat the write as landed only when both the
     exact value and strictly advanced timestamp match, and otherwise stop and
     reconcile before collecting a new three-source aggregate and considering
     another write. The evidence expires after 30 minutes, so a prolonged
     disabled-stage pause requires a new aggregate, write, and readback before
     monitoring or activation.

5. **Deploy and verify the public frontend.**
   - The checked-in recipe is `conflux_public_frontend` in the one-canister
     `conflux-production-public` environment. Its allocation mapping contains
     only `a52ri-naaaa-aaaas-qgy4a-cai`; it deliberately does not include a
     deployable artifact. Do not substitute
     the existing `conflux_espace_frontend` / `mainnet-staging` rail, and never
     use the implicit `ic` environment.
   - Before the dedicated canister was provisioned, only the README's following
     local/ephemeral verification commands were valid:

     ```bash
     cd src/conflux_espace_frontend
     npm run test:production-public
     npm run build:production-public
     npm run dev:production-public
     npm run verify:production-public:deploy-build
     npm run verify:production-public:recipe
     ```

     These commands cannot deploy. The build verifiers write only to temporary
     directories and remove their output; the local Vite server is fixed to
     `127.0.0.1:5174`; and the deployment-shape verifier uses a denylisted test
     Principal. Do not claim that they produce a deployable artifact.
   - Canister creation and 2T-cycle funding require an explicit irreversible
     approval. The approved payer is `robvector` (Principal
     `ft3ml-xex6k-ppiwj-ie6tc-zwkgb-ybm2x-eat4a-5p2jg-auzl3-latf4-aae`), whose
     read-only cycles-ledger balance was `5_005_940_070_754` cycles at the
     preflight. Refresh that balance immediately before approval and require at
     least `2_000_100_000_000` cycles: 2T for the creation plus the current
     100M cycles-ledger update fee. The exact phase-1 action is:

     ```bash
     env ICP_ENVIRONMENT=conflux-production-public \
       /usr/local/bin/icp canister create conflux_public_frontend \
       -e conflux-production-public \
       --identity robvector \
       --controller fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae \
       --cycles 2t
     ```

     `robvector` is the payer only and must not be a controller. The explicit
     sole controller is `rumi_identity` at
     `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`.
     No subnet is pinned; for an empty environment, icp-cli randomly selects an
     available application subnet. The created canister pays the subnet creation
     charge from the attached 2T: currently 500B on a 13-node application subnet or
     approximately 1.308T on a 34-node subnet, leaving approximately 1.5T to
     0.692T before subsequent usage.
   - If creation returns an ambiguous result, do not retry. Reconcile the
     cycles-ledger result, command output, and mapping first so a successful
     remote create cannot be duplicated after a local mapping-write failure.
     Stop after an unambiguous creation. Require exactly one entry named
     `conflux_public_frontend` in
     `.icp/data/mappings/conflux-production-public.ids.json`; verify Running,
     the exact sole controller above, the selected application subnet, actual
     cycles, and the empty/uninstalled state. Verify `robvector` is not a
     controller. Allocation completed unambiguously on 2026-08-27 as
     `a52ri-naaaa-aaaas-qgy4a-cai` on the 13-node application subnet
     `4utr6-xo2fz-v7fsb-t3wsg-k7sfl-cj2ba-ghdnd-kcrfo-xavdb-ebean-mqe`, Running
     and uninstalled with `rumi_identity` as sole controller. Commit the mapping
     so the allocated authority is not lost.
     Creation is not deployment: this phase performs no frontend build, seal,
     Wasm install, asset sync, or deploy.
   - The separately reviewed sole canonical certified origin is
     `https://a52ri-naaaa-aaaas-qgy4a-cai.icp0.io`. The guarded
     build accepts no caller-supplied origin and rejects management, anonymous,
     non-canister, raw, `ic0.app`, custom-domain, IP, port, path, query,
     fragment, extra mapping keys, and its deterministic verification
     Principal.
   - After mapping/origin approval, build without installing or syncing:

     ```bash
     env ICP_ENVIRONMENT=conflux-production-public \
       /usr/local/bin/icp build conflux_public_frontend \
       -e conflux-production-public
     ```

     The recipe runs the `production-public` Vite mode and emits only
     `src/conflux_espace_frontend/dist-production-public`. It copies
     `.ic-assets.production-public.json` to the canonical `.ic-assets.json`
     filename, requires `allow_raw_access: false` on every rule, enables the
     SPA catch-all, and writes the deterministic
     `rumi-conflux-production-public.json` manifest. The scanner requires the
     exact canonical origin, production backend/IcUSD/RPC/chain/finality pins
     and rejects dev-signer, production-canary, chain-71, testnet backend/RPC/
     contract, and verification-Principal material.
   - Seal the reviewed artifact without contacting the IC:

     ```bash
     cd src/conflux_espace_frontend
     env ICP_ENVIRONMENT=conflux-production-public \
       npm run seal:production-public-release
     ```

     Review the exact origin, mapping, ICP recipe, asset policy, deployment
     manifest, exact `icp.yaml` digest, sorted complete asset hashes, built asset-canister Wasm hash,
     and the printed release-seal sha256. Commit the mapping and deterministic
     `.icp/data/reviews/conflux-production-public.release.json`. This
     is the artifact-construction proof state, not deployment.
   - Installation and asset sync require a separate explicit production
     approval. Use the already reviewed build output; do not invoke a pipeline
     that rebuilds between review and execution:

     ```bash
     cd src/conflux_espace_frontend
     env ICP_ENVIRONMENT=conflux-production-public \
       npm run install:reviewed-production-public -- \
       --mode install \
       --approved-release-sha256 <exact-approved-release-seal-sha256>
     ```

     The first install uses `--mode install`; subsequent upgrades use
     `--mode upgrade`, never reinstall. The wrapper targets the sealed
     Principal directly for install, supplies the sealed Wasm path, rechecks
     the exact ICP recipe/target/manifest/assets/Wasm before install and again before sync, and
     never attempts sync after an install failure or any intervening drift.
     Verify the exact module hash,
     controllers, cycles, listed assets, canonical manifest and JS hashes,
     successful certified-origin root and SPA-fallback responses, and refusal
     from `https://<principal>.raw.icp0.io`. Do not report the frontend
     deployed if install succeeded but sync or any read-back failed.
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

7. **Refresh final expiring evidence and make the read-only monitor green.**

   After the frontend and bounded vault inventory are complete, refresh the
   cached settlement gas balance while the chain remains Disabled. Keep this as
   the final state-changing staging call before the disabled monitor because its
   evidence expires after five minutes. This is a developer-only production
   update; it reads the chain balance and updates only the cached balance and
   timestamp.

   First query launch status and record the current
   `hot_wallet_balance_refreshed_at_ns` as `PRE_REFRESHED_AT_NS` (`0` if absent):

   ```bash
   icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
     get_chain_public_launch_status '(1030 : nat32)' \
     -n ic --identity anonymous --query
   ```

   Then perform the refresh exactly once:

   ```bash
   icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
     refresh_chain_hot_wallet_balance '(1030 : nat32)' \
     -n ic --identity rumi_identity
   ```

   Record an unambiguous `Ok` response, then promptly read status again. Require
   `hot_wallet_ready = opt true`,
   `hot_wallet_balance_refreshed_at_ns > PRE_REFRESHED_AT_NS`, a present
   `hot_wallet_balance_age_ns` no greater than
   `hot_wallet_balance_max_age_ns`, and `hot_wallet_balance_is_fresh = true`.
   Do not treat a nonzero balance alone as current evidence. If the update result
   is ambiguous, do not blindly retry: query status first and treat the refresh
   as landed only when its timestamp strictly advanced and its balance is both
   sufficient and fresh; otherwise stop and reconcile. The monitor itself
   remains anonymous and query-only; it never performs this refresh.

   While staging with the chain disabled:

   ```bash
   scripts/check-conflux-public-launch.sh --expect-disabled
   ```

   If the five-minute hot-wallet proof expires before activation, do not enable.
   Repeat the pre-read, refresh, post-read, and disabled monitor; an older green
   monitor output is not current evidence. Likewise, if the 30-minute price proof
   expires, collect a new three-source aggregate, write and verify it first, then
   refresh the hot wallet last and rerun the disabled monitor.

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
   - Perform the separately approved chain-1030 enable action exactly once:

     ```bash
     /usr/local/bin/icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
       enable_chain '(1030 : nat32)' \
       -n ic --identity rumi_identity
     ```

   - Require an unambiguous `Ok` response. If the update result is ambiguous,
     do not retry; reconcile live status and events first.
   - Then require the public-active monitor to pass before announcing public
     availability:

     ```bash
     scripts/check-conflux-public-launch.sh --expect-public-active
     ```

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
- invariant, reorg, bad-debt, or protocol freeze breaker, including any
  chain-1030 bad-debt threshold other than exactly `10_000_000` e8s;
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

### 2026-08-26 rolled-back backend upgrade attempt

The exact frozen gzip artifact above was installed while the backend was
stopped. The stopped-canister read-back returned `module_hash =
14d65746d2d801347ecdb24dc54611b12cb3cca8765f5bf80f929751f1eda287`, but
the execution approval incorrectly expected the decompressed content hash
`44d13c58f20d53dda91030f2c6c038e9db976b5e83cd2cb019b56219b744654e`.
The mismatch therefore triggered the approved conservative rollback before
the new module was ever started. Snapshot
`00000000000000040000000001f088fe0101` was restored, the old module
`a8dffdb9e4fbb41de9f7f23a38d1e2b4e4aeb172a823004462e18ff22540e2e3`
was verified, and the backend was restarted on that old module.

Do not reuse that snapshot for another attempt: production resumed after it
was taken. A retry requires fresh explicit approval, a fresh preflight, and a
new snapshot. Its stopped pre-start and running post-start module-hash gates
must both require the uploaded gzip SHA-256
`14d65746d2d801347ecdb24dc54611b12cb3cca8765f5bf80f929751f1eda287`;
a read-back of
`44d13c58f20d53dda91030f2c6c038e9db976b5e83cd2cb019b56219b744654e`
is not success.

## Evidence packet for the launch record

Capture, redact, and retain:

- merged commit and green CI checks;
- uploaded gzip artifact SHA-256, decompressed Wasm content SHA-256, and the
  production `module_hash` read-back matching the uploaded gzip SHA-256;
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
