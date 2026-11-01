# XRPL icUSD lane: launch checklist

Branch `codex/xrpl-icusd`. This file is a checklist, not an authorization. Merging the software does not activate anything: the lane ships disabled (`enabled = false`, `issuance_paused = true`), and every step below that touches a live network, key or fund needs its own explicit approval.

## What ships disabled

New backend user endpoints:
- `borrow_to_xrpl`
- `issue_xrpl_from_canonical`
- `prepare_xrpl_repayment`
- `observe_xrpl_repayment`
- `resume_xrpl_operation`
- `resolve_xrpl_claim`
- `get_my_xrpl_operations`

New controller endpoints:
- `configure_xrpl_icusd`
- `set_xrpl_icusd_pauses`
- `verify_xrpl_icusd_minting_account`
- `run_xrpl_issuer_setup_step`
- `get_xrpl_icusd_coverage`
- `get_xrpl_quarantined_receipts`
- `reconcile_xrpl_quarantined_receipt`
- `get_xrpl_overflow_evidence`
- `reconcile_xrpl_overflow_receipt`
- `retry_xrpl_fee_mint`
- `reconcile_xrpl_borrow_mint`

User methods return `Disabled` until all of these hold:
- the lane is enabled;
- the network is set;
- an explicit issuer policy is stored;
- issuer setup has reached `Ready`;
- the ledger minting account has been verified.

The XRPL observer timer is idle (zero outcalls) while no intents, claims or coverage mismatches exist.

## Before any backend upgrade carrying this branch

1. **Artifact size.** The production artifact's code section must stay under the 12 MiB IC limit.
   - This branch optimises `rumi_protocol_backend` for size (`opt-level = "s"`, backend crate only).
   - Measured code sections after the production `ic-wasm shrink` step:
     - main `2c071994`: 12,153,827 bytes;
     - this branch: 11,616,810 bytes at `621dec58` (see the verification report).
   - Size-optimised code may use more instructions per message. Check post_upgrade and the heaviest timer ticks on a staging canister before mainnet.
2. **Candid.** The `Event` type gains the `xrpl_icusd` variant, and new methods and types are added.
   - The CLI may warn that this is a breaking change for the Event variant. Review the warning; do not wave it through.
   - Redeploy the frontend, so its declarations know the variant, BEFORE the lane records its first event. Old strict decoders of `get_events` throw on an unknown variant.
   - `rumi_analytics` uses a resilient per-event decoder. `rumi_points` filters by event type.
3. **One-way door.** Once the first `xrpl_icusd` event is recorded, rolling the backend back to a pre-lane wasm breaks `get_events` and the fallback replay, because the old decoder traps on the variant. The snapshot fast path still loads. Treat rollback after activation as unsupported.
4. **Upgrade argument.** Use fresh `scripts/rumi-backend-upgrade-args.sh` output, as the existing upgrade policy requires.

## Activation sequence (each step needs explicit approval)

1. **Issuer policy decision.** Choose `allow_clawback` and `no_freeze` before any issuer setup.
   - Clawback can only be enabled while the issuer has an empty owner directory, and it is irreversible.
   - NoFreeze is irreversible.
   - The two cannot both be set.
   - Local fixtures use `false/false`. That is not a production decision.
2. **Configure the lane** through `configure_xrpl_icusd`:
   - network;
   - policy;
   - caps (outstanding and per-operation);
   - `max_fee_drops`;
   - `rounding_buffer_e8s`;
   - `min_auto_refund_e8s` (default 0.01 icUSD).
3. **Verify the minting account** with `verify_xrpl_icusd_minting_account`. It must equal the backend's default account and must differ from the escrow subaccount (`sha256("rumi:xrpl-icusd:escrow:v1")`).
4. **Fund the issuer account** with XRP for the base reserve plus a fee budget. The issuer address is derived from threshold Ed25519 at `[rumi, xrpl-icusd, v1, network]` and is shown in the config.
5. **Fund the escrow surplus.** Deposit canonical icUSD into escrow for the rounding buffer plus the ledger-fee surplus that canonical returns need. `get_xrpl_icusd_coverage` reports `fee_surplus_required_e8s`. Without it, `ReceiveCanonical` refuses with "ledger-fee surplus not funded".
6. **Run issuer setup.** Call `run_xrpl_issuer_setup_step` until `Ready`. It sets DefaultRipple, RequireDest and TransferRate 1,000,000,000, plus clawback or NoFreeze only if the policy says so, and then verifies the final flags.
7. **Frontend config.**
   - `VITE_XAMAN_API_KEY`: public PKCE key from a registered Xaman app, with the app origin and redirect registered. Never use the API secret.
   - `VITE_XRPL_NETWORK`.
   - Optional `VITE_XRPL_RPC_ENDPOINTS`; it must also be added to the CSP connect-src.
8. **Unpause issuance.** Call `set_xrpl_icusd_pauses` with issuance on, starting with a small pilot cap.

## Public-testnet proof (R10), still pending

`scripts/run-xrpl-icusd-testnet.sh` builds the isolated `xrpl-testnet-fixture` artifact:
- it pairs `test_key_1` with XRPL testnet, and mainnet cannot be selected;
- it verifies the artifact marker;
- `--execute` installs only onto a local environment.

A full R10 run needs:
- an approved Xaman test app key;
- XRPL testnet faucet funding for the issuer and a test wallet;
- authorization to sign and broadcast testnet transactions.

The run records each lifecycle step separately:
1. wallet and network;
2. XRP payment validation;
3. collateral credit;
4. escrow mint block;
5. debt event;
6. issuer Payment validation;
7. token balance;
8. return Payment;
9. escrow burn block;
10. debt change;
11. reconciliation totals.

## Operational notes

- **Observer cost.** While intents or claims are open, the observer polls the issuer history every 30 s, backing off to 5 min, through a 3-provider quorum. Measure the actual cycle burn on staging; do not extrapolate.
- **State bounds.**
  - 10 active operations per owner, 10,000 globally.
  - Terminal operations are pruned after 30 days.
  - Dedup records are compacted behind the scanned history frontier.
  - Unattributed returns are aggregated per sender (2,000 slots, with dust evicted to an overflow that the controller can reconcile by receipt proof).
  - The snapshot-size test at all caps measured about 46 MB against a 64 MiB budget.
- **Controller tools.** Every recovery endpoint re-verifies ledger or XRPL proof. None accepts assertion-only adjustments.
