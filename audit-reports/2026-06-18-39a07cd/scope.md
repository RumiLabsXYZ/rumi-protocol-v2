# Audit scope — Conflux/Monad chains rail

**Anchor:** `origin/main` @ `39a07cd1c3fcfbb0acedc975d4ba4ced2d25f71d` (branch `main`).
Includes PR #251 (Conflux eSpace rail) + PR #252 (Option-B interest accrual).
**Audit branch:** `audit/conflux-chains-rail` (worktree, read-only over `chains/`).
**Date:** 2026-06-18. **Dirty files:** none (clean checkout of the anchor).

## Why this audit
Pre-mainnet gate for a **gated eSpace-mainnet soft-launch** of the native-CFX CDP:
chain 1030, small debt ceiling, dev/allowlist-gated, manual liquidation + monitoring.
This is the "security review of the revived chains rail" mainnet gate.

## In scope
- `src/rumi_protocol_backend/src/chains/` (~12,343 non-test LoC): `vault.rs`, `supply.rs`,
  `multi_chain_state.rs`, `settlement_queue.rs`, `config.rs`, `admin.rs`, `recovery.rs`,
  `interest.rs`, `evm/*` (adapter, deposit_watch, settlement, tecdsa, evm_rpc, hardening,
  burn_proof, tx, conflux/, monad/), `solana/*` + `xrp/*` (shared-helper blast radius only).
- `foundry/src/IcUSD.sol` (51 LoC) + `foundry/test/IcUSD.t.sol`.
- The NEW Option-B interest path end-to-end (harvest → InterestMint → confirm), the synthetic
  `mint_id` disjointness, the per-chain tECDSA interest-treasury.

## Out of scope (explicit)
- ICP-native CDP engine (vaults/SP/redemption/liquidation) — separately audited (2026-06-09 @e49ed10).
- Chains **liquidation** — deferred/not built; soft-launch uses manual liquidation + caps.
- **M2 EVM-native self-serve auth** (`feat/conflux-evm-self-serve-auth`) — concurrent session, owns the
  chains source; reviewed separately. This audit does not edit chains source.
- SP cross-chain deposit; automated CFX oracle (soft-launch uses admin manual_prices + monitoring).

## Methodology
10 parallel read-only specialist passes (Explore agents) → adversarial verification of every
medium+ finding → grep-authoritative coverage cross-check → calibrated report.
Passes: supply-invariant, mint/double-mint, withdraw/custody, interest-accrual, async-races,
rpc-finality-reorg, stable/upgrade, auth/cycle-DoS, tECDSA-keys, IcUSD.sol.
Differential vs the 2026-06-03 cross-chain audit (Solana+Monad first pass).

## Hard invariant under test
`sum(multi_chain.chain_supplies) == sum(chain_vaults[*].debt_e8s)` — enforced by
`apply_supply_delta` + Timer-B `check_invariant` + `reconcile_chain_supply`.
