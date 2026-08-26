# Conflux Disabled-cursor reseed evidence

Status: **sanitized approval evidence; no execution authority**

Observation window: `2026-08-26T09:48:57Z` through
`2026-08-26T10:16:25Z`

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

The three paths independently returned the same results at the reviewed recent
target:

```text
T = 155_237_900 (0x940be0c)
T header hash = 0xe107566329fbc0526e7105d3cb788daff8588b950abcd9546603c6884c72e78d
factory getPair(WCFX, USDC) at T = 0x0736b3384531cda2f545f5449e84c6c6bcd6f01b
IcUSD totalSupply() at T = 0
F = T + 400 = 155_238_300 (0x940bf9c), header present
```

The cursor setter stores `T`, but the subsequent liquidation setter calls
`fetch_block_numbers`. To remove timing-dependent behavior, its exact derived
blocks are also frozen:

```text
C = T + 1_024 = 155_238_924
P = C + 400 = 155_239_324
```

Before approval and again immediately before execution, require at least two
configured independent providers to return matching immutable headers for `C`
and `P`, require `P` to exist, and require factory `getPair(WCFX, USDC)` at `C`
to return the canonical pair. With `P` present, `fetch_block_numbers` will
return `C` for the liquidation setter's pinned read. Supply reconciliation
still reads exactly at stored cursor `T`; `fetch_block_numbers` does not mutate
the cursor.

No observation in this file claims that `C` or `P` had already been produced
during the earlier `T`/`F` probe. Their required preflight is deliberately an
expiry condition, not reconstructed evidence.

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
