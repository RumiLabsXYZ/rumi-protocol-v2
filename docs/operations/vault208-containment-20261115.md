# Vault 208 — proposed Stability Pool admission hold

Rob approved this exact containment in human message
`01a1e262-209f-7e62-ba26-6b3ea42cff83`. **Executed and independently verified.** It is not
a deployment, liquidation, recovery payment, unlock or approval of Vault 202.

## Proposed exact action

- Network: `ic`; identity: `rumi_identity`.
- Existing backend: `tfesu-vyaaa-aaaap-qrd7a-cai`.
- One controller call: `set_sp_writedown_disabled(true)`; Candid arguments `(true)`.
- Required reply: `variant { Ok }`; independently query
  `get_sp_writedown_disabled()` and require `true`.
- Read back Stability Pool `tmhzi-dqaaa-aaaap-qrd6q-cai` using
  `get_pool_status()`; its emergency-pause flag should remain unchanged/false.
- Preserve existing claims, proof journals, admitted preflights, balances and
  settlement records. Do not execute a liquidation, retry, transfer or unlock.
- Keep the admission hold until a separately accepted remedy/decision. This
  packet does not authorize clearing the flag or resuming a stopped process.

## Exact scope and reason

The existing switch blocks **new Stability Pool write-down/admission across
collateral types**, including native-XRP preflight. It is not an XRP-only switch.
It leaves manual and bot liquidation paths available. It does not change pool
deposit, withdrawal, collateral-gain claim or existing XRP-settlement controls.
Already admitted/reserved or burned work may finish its existing proof-bound
settlement; this is not a cancellation or rollback of those effects.

The pool-wide `emergency_pause()` is broader: current source checks that flag
in deposits, withdrawals, gain claims and the native-XRP settlement sweep. It
is therefore not the proposed action. XRP collateral status `Paused` also
allows liquidation and would not contain this incident.

## Observed evidence and current source

Read-only observations on November 15, 2026 Pacific:

- `get_sp_writedown_disabled()` = `false`.
- `get_liquidation_frozen()` = `false`.
- Stability Pool `get_pool_status().emergency_paused` = `false`.
- Backend module SHA256:
  `59a64f78e0e652d0e48413aba587d3f826c1dc25258f56ab09575c59de444111`.
- Pool module SHA256:
  `d8385398e5b92ca26b029b6b76ab78d634f40ced65d1f0af5b991591332329e9`.
- One pool-history record for Vault 208 consumes 2,812.45001091 icUSD,
  credits 3,217.077995 XRP to six depositors, and has timestamp
  `1794689067323215836` nanoseconds. Its current vault has zero debt and
  275.174474 XRP remaining. These are query observations, not yet the complete
  incident reconstruction or a statement of XRPL payout finality.

Bram supplied current module/source provenance at `f72ede81`; inspection of
that exact Git source confirms the switch before native-XRP preflight:
[preflight guards](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/f72ede81/src/rumi_protocol_backend/src/vault.rs#L4706).
`state.rs::recommended_liquidation_amount_for` explicitly returns full live
debt for native XRP; ordinary collateral uses the partial sizing helper.
`vault.rs::xrp_sp_absorb_sizing` requires the burn to equal full live XRP debt.
This makes recurrence possible when the pool covers another eligible XRP
vault's debt. Pausing the ICP-only bot would not contain this path.

Refresh the module identities and flag immediately before an approved call.
If material scope/source drift appears, hold and report it. Unknown replies
require readback before any further write. No snapshot, installation, funds
movement, customer-setting change or pool-wide emergency pause is included.

## Actual execution and durable release hold

Exactly one setter returned `variant { Ok }` at 2026-11-16T03:25:23.313627Z
(November 15, 7:25 p.m. Pacific). Independent readback at03:25:23.954997Z
returned `true`; pool readback at03:25:24.687202Z kept emergency_paused=false.
See [execution receipt](evidence/vault208-containment-20261115/receipt.json).

**Do not resume Stability Pool liquidations until both the XRP partial-liquidation
fix is verified AND Rob explicitly approves resuming. No upgrade, restart,
automatic reconciliation or other release authorizes clearing this hold.**
The present setter is snapshot-serialized but is not event-recorded; fallback
event replay can lose its true value. Any later approved backend release must
prevent an admission window, verify true before and after installation/start,
and include the scoped durable hold-preservation correction. No upgrade is
authorized by this containment receipt.
