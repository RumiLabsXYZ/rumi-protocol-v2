# Conflux Disabled-cursor reseed evidence

Status: **sanitized approval evidence; no execution authority**

Observation window: `2026-08-26T09:48:57Z` through
`2026-08-26T10:16:25Z`

Replacement-target observation window: `2026-08-26T14:20:00Z` through
`2026-08-26T14:25:50Z`

Production backend: `tfesu-vyaaa-aaaap-qrd7a-cai`

Chain: Conflux eSpace mainnet `1030`

This record preserves the credential-free evidence used by the bounded cursor
reseed addendum in
`docs/plans/2026-06-18-conflux-gated-mainnet-launch-runbook.md`. It records
observations only. It does not authorize a canister call, endpoint change,
quorum change, activation, frontend action, or funds movement.

## Live backend observations

- Backend status: Running.
- Live module hash:
  `14d65746d2d801347ecdb24dc54611b12cb3cca8765f5bf80f929751f1eda287`.
- Controllers:
  - `cpbhu-5iaaa-aaaad-aalta-cai`;
  - `mi66c-zqlu4-4kxd6-2gtp7-szg5v-6a62a-geoty-fahu5-4trje-xyfby-wqe`; and
  - `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`.
- Cycles were above the reviewed `5_000_000_000_000` floor.
- Chain status: Disabled; `public_open_ready = false`.
- Effective EVM-RPC principal:
  `7hfb6-caaaa-aaaar-qadga-cai`; `overridden = false`.
- Threshold-ECDSA key: `key_1`.
- Bound IcUSD:
  `0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff`.
- Protocol: GeneralAvailability, not frozen; invariant and chain reorg halts
  false.
- Finality depth: `400`.
- Pre-recovery burn cursor: `154_966_240`.
- Internal chain supply, reserve backing, pending burn, and bad debt: zero.
- Bad-debt threshold: exactly `10_000_000` e8s; circuit untripped.
- RPC projection: three distinct endpoints, configured floor two, effective
  agreement requirement two, configuration sufficient.
- Complete bounded vault inventory reached `done = true` and returned only
  Vault `#1`, Closed, with zero debt, collateral, and pending mint.

The successful threshold update returned unambiguous `Ok`. Durable event index
`421_808` records the exact chain-1030 threshold value. The readback remained
zero/untripped.

The exact enabled liquidation setter was then called twice. Each call returned
an explicit error before persistence:

```text
factory pair sanity getPair failed: eth_call RPC error:
{"code":-32016,"message":"Error processing request: state is not ready"}
```

After each failure, the public config getter returned no liquidation row and no
digest; chain status remained Disabled. There was no ambiguous result and no
partial config write.

One subsequent `reconcile_chain_supply(1030)` outcall returned explicitly:

```text
totalSupply read failed; retry: eth_call(totalSupply) RPC error:
{"code":-32016,"message":"Error processing request: state is not ready"}
```

It produced no reconciliation report and did not mutate stable state. No
additional setter or reconciliation call was made after the diagnostic stop.

## Credential-free provider evidence

The reviewed provider mapping contains three public, credential-free paths:

- Confura: `https://evm.confluxrpc.com`;
- BlockPI: `https://conflux-espace.blockpi.network/v1/rpc/public`; and
- Unifra: `https://conflux-espace-public.unifra.io`.

No secret query parameter, API key, authorization header, paid URL, or private
endpoint is recorded here. Commit `04b5304be1a02f6cebb3ede79f1cf872cd91f9c6`
records these as the exact three configured Conflux providers used by the live
probe repair. The current public status projection confirms a live count of
three but deliberately does not expose URLs. The execution preflight must
therefore re-confirm this mapping against the operator's current redacted
configuration record; a count of three alone is not proof of endpoint identity
or operator independence.

Each path returned chain ID `0x406` (`1030`). At the stale cursor lineage, all
three returned matching block headers, but their historical state capability
differed:

| Provider | Header `154,967,264` | Header `154,967,664` | `getPair` at `154,967,264` | IcUSD supply at `154,966,240` |
|---|---:|---:|---|---|
| Confura | present | present | canonical pair | `0` |
| BlockPI | present | present | `-32016 state is not ready` | `-32016 state is not ready` |
| Unifra | present | present | `-32016 state is not ready` | `-32016 state is not ready` |

The backend uses two-of-three semantic agreement. BlockPI and Unifra's
identical well-formed JSON-RPC errors therefore reached quorum and were then
rejected by the downstream `eth_call` parser. Confura's single successful
state result could not meet the financial-read floor. This is fail-closed
behavior, not evidence that the official EVM-RPC canister changed the result.

## Fixed recovery blocks

The first approved target (`T = 155_237_900`) expired before execution. The
immediate pre-call gate still found identical headers from Confura, BlockPI,
and Unifra, but BlockPI and Unifra both returned explicit
`-32016 state is not ready` errors for factory `getPair` at the old `T` and
`C` and for IcUSD `totalSupply()` at the old `T`. Confura was the only
successful state provider, below the required floor of two. The operator
therefore stopped before call one. No `set_last_observed_block`, liquidation
setter, supply reconciliation, retry, reversal, endpoint/quorum change, or
other live mutation occurred. The old target and approval are void.

The replacement is a new fixed target, not a dynamic or rolling value. At
selection, the three reported heads were `155_252_211` (Confura),
`155_252_209` (BlockPI), and `155_252_211` (Unifra). The slowest provider was
already 1,285 blocks beyond `P`. All three paths independently returned the
same replacement results:

```text
T = 155_249_500 (0x940eb5c)
T header hash = 0xfdb455b5e2e8fb8fc20d58709f98d94a082232bd105d83786afda7205ea9f2ef
factory getPair(WCFX, USDC) at T = 0x0736b3384531cda2f545f5449e84c6c6bcd6f01b
IcUSD totalSupply() at T = 0
F = T + 400 = 155_249_900 (0x940ecec)
F header hash = 0x47307952a58607bcd5cf6af9078bf10abc842b8a926d57ed7bda632da29b8086
```

The cursor setter stores `T`, but the subsequent liquidation setter calls
`fetch_block_numbers`. To remove timing-dependent behavior, its exact derived
blocks are also frozen:

```text
C = T + 1_024 = 155_250_524 (0x940ef5c)
C header hash = 0xfa50433742b5f87654563cb3d8edc3c22d35900ae4a0ff0b05cfb6e8ceafd1fd
P = C + 400 = 155_250_924 (0x940f0ec)
P header hash = 0xff2f7eba0c362295a051f682a39bbeb6054fb126d2d017d0e65c3d662a547592
factory getPair(WCFX, USDC) at C = 0x0736b3384531cda2f545f5449e84c6c6bcd6f01b
```

Before approval and again immediately before execution, require at least two
configured independent providers to return matching immutable headers for `C`
and `P`, require `P` to exist, and require factory `getPair(WCFX, USDC)` at `C`
to return the canonical pair. With `P` present, `fetch_block_numbers` will
return `C` for the liquidation setter's pinned read. Supply reconciliation
still reads exactly at stored cursor `T`; `fetch_block_numbers` does not mutate
the cursor.

The replacement observation directly proved that `F`, `C`, and `P` already
existed and that all three providers returned their identical canonical
headers. It also directly proved the canonical pair at both `T` and `C` and
zero IcUSD supply at `T` from all three providers. These observations do not
waive the same fixed-matrix expiry check immediately before each approved call.

## Provenance and limits

The root-cause source paths are:

- observer dispatch filters out Disabled chains:
  `src/rumi_protocol_backend/src/main.rs:309-329`;
- liquidation setter block derivation and factory validation:
  `src/rumi_protocol_backend/src/main.rs:3414-3436`;
- candidate/finality probe:
  `src/rumi_protocol_backend/src/chains/evm/evm_rpc.rs:1415-1450`;
- supply reconciliation pinned to the stored cursor:
  `src/rumi_protocol_backend/src/main.rs:5094-5186`; and
- unvalidated developer cursor overwrite and skipped-history contract:
  `src/rumi_protocol_backend/src/main.rs:4668-4692`.

The evidence supports only the exact zero-state, Disabled recovery described
in the canonical runbook. It is not a generic justification for skipping burn
history on a live or nonzero chain. Complete queue emptiness is not exposed by
the current backend; `chain_has_active_settlement_op = false` remains supporting
evidence only. Receipt reconciliation, complete vault inventory, zero internal
and on-chain supply, current role provenance, exact endpoint mapping, and the
fixed-block provider matrix must all be refreshed before execution.
