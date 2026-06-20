# Methodology

- **Harness:** audit-icp-cdp. **Anchor:** 39a07cd (clean). **Mode:** first chains-rail-focused audit (differential context: 2026-06-03 cross-chain first pass).
- **Passes (10, read-only Explore agents):** supply-invariant, mint/double-mint, withdraw/custody, interest-accrual,
  async-races, rpc-finality-reorg, stable/upgrade, auth/cycle-DoS, tECDSA-keys, IcUSD.sol(Solidity).
- **Rule packs loaded:** ic-rules.md (§1 async races, §2 inter-canister failure, §3 stable/upgrade, §4 cycle DoS,
  §5 caller auth, §6 controller-vs-admin), cdp-checklist.md (§1 oracle, §2 CR math, §6 debt/interest).
  ICP-native passes that DON'T apply (SP accounting, redemption/peg, ICP liquidation) were dropped — out of scope.
- **Verification:** every medium+ candidate independently re-verified by a skeptical agent (default false_positive).
  A verifier conflict on the interest-on-closed-vault concern (F-05) was resolved manually by reading
  vault.rs:424-452 (withdraw guard) + 555-594 (close shortcut) — exploit precondition (Open, collateral==0) proven unreachable.
- **Coverage:** grep-authoritative enumeration (see coverage-crosscheck.md), not code-graph.
- **Key IC-semantics calibration applied repeatedly:** a read_state immediately followed by a mutate_state (no .await
  between) is atomic on the IC; cross-message interleaving only at awaits. This collapsed 1 recurring false positive
  (the pre_total "race").
