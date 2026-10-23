# User-funded ckSOL deployment — 2026-10-23

User authorization: merge and deploy the caller-funded ckSOL flow for Plug, Internet Identity, and OISY, with sponsorship disabled and unused attached cycles refunded to the payer.

## Source preservation

PR #587 passed CI at `aa9e39a42e8dd4fcc076488ccf60823cb9b4f261` and merged as `32358d23d98b6430cab0236a3245519c9322fe90`; the Git trees are identical. Its implementation evidence remains in `docs/coordination/cksol-user-funded-2026-10-20.md`.

Fresh backend status instead found deployed gzip `aad0b7e133a27938ed25ebebbd5d4de5d7bb1793cf0d5ccbad5210c4644da259`. This exactly matches the retained inspection release artifact at source `c98db171c7d7722e50ee8ed8f41313a1f4350800`. That source differs from the original ckSOL base only by four Rust lines and corresponding generated bindings: the `route` field in `TreasuryInterestMintRecoveryRow` and its cloned projection. Deploying PR587 alone would remove a required live query response field; `didc check` reproduced that incompatibility. The integration branch ports those exact four existing live lines and regenerates bindings. It changes no Treasury accounting, state, auth, or notification behavior. The resulting service passes compatibility against freshly retrieved deployed Candid.

The previous inspection release explicitly recorded a 262,144-byte code reserve and two independent reviewers verified its size/headroom. This deployment retains that margin rather than lowering it. Artifact size and optimization evidence will be recorded below when complete.

## Live targets and authority

- Network: ICP mainnet.
- Identity: `rumi_identity`, observed principal `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`, an existing controller on both canisters.
- Backend: `tfesu-vyaaa-aaaap-qrd7a-cai`; initial module hash above.
- Frontend: `tcfua-yaaaa-aaaap-qrd7q-cai`; assets-only sync. Preserve Wasm `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62` and controllers.
- Preserve mode, mint cap, emergency holds, collateral settings, vaults/debt, pending liabilities, and receipt/reconciliation records. Fresh pre/post query sets are retained privately; no credit replay or financial test is authorized by deployment.
- One 1T top-up was made from the previously user-supplied cycles to the same backend to maintain its protected operating reserve. The identity's Cycles Ledger decreased from 4,051,050,044,313 to 3,050,950,044,313, exactly 1T plus the 100M fee. This did not allocate public sponsorship; its readback remains zero.

## Upgrade and backup procedure

The oldest occupied snapshot4 was completely downloaded and every payload length/hash verified before any slot reuse. Only after final artifact checks and independent review, stop the backend, replace that archived slot with a fresh snapshot, verify its payloads against metadata and the currently installed module, upgrade once with `Upgrade { mode = null; description = ... }`, and restart. Record the original request and reconcile it before any retry after an uncertain result. No automatic stale snapshot restoration is authorized after intervening traffic.

After upgrade, verify the uploaded gzip hash, deployed Candid, unchanged settings/controllers, paid quote and receipt endpoints, and zero sponsorship. Synchronize the built frontend assets without replacing its canister Wasm; compare served index/route/chunks with the build manifest. Actual wallet-funded minting, token approvals, SOL transactions, and refund settlement are separate and remain unverified unless explicitly evidenced.

## Host resources

The required guarded worktree cleanup found zero eligible removals and removed zero worktrees. All protected worktrees were preserved. Under disk pressure, only this task's disposable 840MiB Wasm-check cache was removed; source, backups, live evidence, and shared build artifacts were retained.
