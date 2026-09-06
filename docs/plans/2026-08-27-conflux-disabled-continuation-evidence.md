# Conflux Disabled recovery continuation evidence

Status: **sanitized source approval evidence; no execution authority**

Production backend: `tfesu-vyaaa-aaaap-qrd7a-cai`

Chain: Conflux eSpace mainnet `1030`

This record binds the terminal evidence from the consumed
`conflux-disabled-recovery-v1` execution and the source-only contract for a
distinct two-call continuation. It does not authorize execute mode, a cursor
write, liquidation configuration, reconciliation, activation, price or
hot-wallet mutation, endpoint change, frontend work, cycles or funds movement,
or any wallet signature.

## Consumed execution and exact landed state

The only actual invocation began at `2026-08-27T18:51:38Z`. It selected and
froze:

```text
H = 155_343_278
T = 155_341_230
F = 155_341_630
C = 155_342_254
P = 155_342_654
```

All three configured providers agreed at selection. The phase-one endpoint,
fixed-matrix, module/controller/cycles, Disabled, zero-state, vault inventory,
and quiescence gates passed. The runner dispatched exactly one
`set_last_observed_block(1030, 155_341_230)` update. The response was explicit
`Ok` and has SHA-256:

```text
67c0d269d550ae7d18dd7003f8b2aa577fa925e5f0680ad8fc82fefc4c170810
```

The immediate ordinary `--query` readback returned the prior cursor
`154_966_240`, so the runner stopped fail-closed. That readback has SHA-256:

```text
411df0b66cee858a7f23dc8d207dd76f078ada4fb3cd69383bf4f7f5f1df324d
```

Subsequent ordinary and replicated readbacks both returned exact `T =
155_341_230`. The backend setter writes the cursor synchronously before
returning `Ok`; no delayed commit or rollback path exists. The evidence is
therefore a safely landed cursor plus a stale immediate single-replica query,
not an uncommitted update.

The immutable consumed-execution bindings are:

```text
execution id  = conflux-disabled-recovery-v1
source commit = 102c99a508fc79163919d343762179c6efef97a4
manifest      = 92b72fd1e9fa62bdc7507522c49e53ad37db5dc3554919f17b569af212c9a955
transcript    = 209d0666b802a06f6f01e4f0d2e5acb029c38970b1ea9434e14ddabf407950bf
journal       = 7a80b877fe4185b7c4df76522b66e8929106f326f1ec2b16d0c1718d279c53f8
cursor arg    = df40d664035eddbd642a324fa7eaa4c22b97ef4875f395cbe68658521a62f915
```

The terminal journal state is `stopped`, phase `phase1`, `update_count = 1`,
reason `cursor readback mismatch`. The raw inventory contains exactly one
update response. No liquidation setter or supply reconciliation was
dispatched. The old approval is consumed and must never be reused. Do not
delete, move, edit, bypass, resume, or copy its journal to make a rerun appear
unused, and never resend or reverse the landed cursor.

Post-stop readbacks confirmed:

- backend Running on module
  `835bc12041222bc6acd4ffa65746a27b9d16cdec63a0e2bd28d9536c28752c54`;
- controllers unchanged and cycles above `5_000_000_000_000`;
- chain `1030` Disabled, unregistered, and publicly unready;
- cursor exactly `155_341_230`;
- liquidation row absent and digest null;
- internal chain/global/on-chain supply, reserve, pending burn, and bad debt
  zero;
- bad-debt threshold exactly `10_000_000`, circuit untripped;
- Vault `#1` Closed with zero debt, collateral, and pending mint; and
- no active settlement operation.

This is a safe partial state. It is not a completed recovery or launch-ready
state.

## Source correction

The general runner's critical post-update cursor and liquidation row/digest
readbacks now use `replicated_read`, which invokes `icp canister call` without
`--query`. A single ordinary query is no longer treated as authoritative proof
immediately after an update. Its update allowlist, ordering, journaling,
ambiguity, and no-retry/no-resume contract are otherwise unchanged.

The distinct continuation is
`scripts/conflux-disabled-recovery-continuation.py`. Dry-run is the default.
It uses the new literal one-use execution id:

```text
conflux-disabled-recovery-continuation-v1
```

It binds the five prior evidence hashes above, the exact landed target and
header hashes, the prior three-call manifest, the explicit cursor `Ok`, and an
inventory proving the prior run contains only one update response. Any change
to those files or structures stops before dispatch. The old execution
directory is read-only evidence; the continuation writes only to its distinct
state directory.

At selection it requires fresh all-three chain-1030 heads within the existing
age/skew/sample bounds and requires all three providers to re-serve the exact
frozen `H/T/F/C/P` headers, canonical WCFX/USDC pair at `T/C`, zero IcUSD
`totalSupply()` at `T`, and the expected admin/minter roles. Historical-state
expiry is a stop condition; the continuation never chooses a replacement
target and never changes the cursor.

Immediately before each remaining update it rebinds the replicated endpoint
set digest and requires at least two configured providers to return the full
exact fixed matrix. Every successful conflicting field invalidates the run,
even if another field from that provider is unavailable. It repeats the exact
module/controllers/cycles, Disabled, zero supply/reserve/pending/bad-debt,
Closed-vault, no-settlement/proof/burn, configuration-anchor, and complete
inventory gates. It also performs replicated cursor, liquidation-row,
launch-status, and supply readbacks for the phase.

The only dispatchable methods are, once each and in this order:

1. the exact enabled `set_chain_liquidation_config` row whose digest is
   `d7ff0d667b867f4cb3fbccabd57c05911d17eee6888a5df58e81daf8954f4f1d`;
2. `reconcile_chain_supply(1030)` pinned to stored cursor `155_341_230`.

The exact reviewed argument SHA-256 values remain:

```text
set_chain_liquidation_config = a4ccce984ffdaa69f09fe31cf8d7b9ed5c28848ad6b0a979fa2d2ac1eb543060
reconcile_chain_supply       = ee95dff5c236fb605552cd49583910dc34e9f1260b9e8d7b7829b638d9bec5e8
```

The continuation has a two-update maximum and no cursor method. It journals
`dispatching` before each call. Any transport failure, timeout, malformed
response, or ambiguous result permanently stops; even a later matching
readback does not authorize the next call. Explicit non-`Ok`, replicated
readback mismatch, provider drift, state drift, historical expiry, or any gate
failure also stops. There is no retry, resume, resend, reversal, bypass,
automatic repair, second-host execution, or copied-state exception.

The runner also binds this physical Mac without publishing its hardware UUID.
It pins `/usr/sbin/ioreg`, extracts `IOPlatformUUID` privately, and commits its
lowercase value under the domain `rumi.conflux-recovery-host.v1`. Only this
SHA-256 is accepted and sealed:

```text
353a2c68134d012cfc89fc69187cb8f121493b08837c815f2e884be75ec432da
```

Different hardware fails before selection. This is still a client-side guard,
not a canister CAS: deliberately substituting the state root under another
account on the same hardware remains a residual and is forbidden by approval.

After an explicit liquidation-setter `Ok`, the exact row and digest must be
confirmed through replicated ingress before phase three. The reconciliation
response must be the exact typed zero-state `Ok(record)` pinned to block
`155_341_230`; any ambiguity permanently stops. Final cursor, row/digest,
supply, and Disabled status are again read through replicated ingress before
completion is recorded.

## Approval boundary

Source merge and a passing default dry-run are not execution authority. A
fresh approval must bind the merged commit, continuation script, corrected
base script, this evidence file, canonical runbook, exact live module, and the
approved host SHA-256 plus the new one-use execution id. It authorizes only one
host-local invocation and the two ordered calls above. The approval is consumed
when that invocation begins, even if it stops before its first update.

No cursor update, chain activation, manual price, hot-wallet refresh, endpoint
or quorum change, backend upgrade, frontend action, Stability Pool action,
cycles or funds movement, or wallet signature belongs to this continuation.
Chain `1030` must remain Disabled throughout.
