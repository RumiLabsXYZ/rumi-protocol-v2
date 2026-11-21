# Deferred work to revisit before client money

Apply [DEMO_FIRST_DELIVERY_POLICY.md](DEMO_FIRST_DELIVERY_POLICY.md). This ledger records actual shortcuts and omitted investigation for new demo features; it does not pause incident response or weaken an existing money-bearing service.

For each actual omission, record: **ID; feature; owner; concern/skipped work; why not essential to the demo; demo limitation/containment; before-client-money verification; status; evidence/disposition.** Use lane-specific files under `demo-deferred/` and link them below when multiple owners are active. Existing project boards remain authoritative.

No feature-specific omission has been asserted by this initial policy entry. Each supervisor records the actual work it defers while delivering; do not invent a debt inventory or redo completed reviews merely to fill this file.

## Feature ledger index

Append actual lane-log links here. Before client-money use of the affected feature, revisit every applicable open entry and record the resolution, evidence and reviewer. Existing clients and production incidents retain their current safeguards.

- [Multichain icUSD groundwork — MC-DEF-1 to MC-DEF-6](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/codex/base-icusd-bridge/docs/coordination/demo-deferred/multichain-icusd.md), owned by Nora and Miles; destinations remain disabled.
- [Sunset collateral liquidation — SUNSET-DEF-1](demo-deferred/sunset-liquidation.md), owned by the release coordinator; production deployment verification does not include a funded liquidation.

### DEMO-EXP-001 — Explorer revenue source handoff

Owner: Tamsin. Commit `2369985f` on `codex/explorer-revenue-accuracy`. Five focused tests and both Svelte component compile checks passed; full frontend build, rendered 24h/90d unavailable-versus-zero acceptance, and exact live Borrow query/deployed-version evidence were omitted from the bounded source tranche. Hosted CI was not allocated; opening a PR triggers it. Limitation: source correctness evidence only, no demonstrated live repair. Revisit relevant rendered/build/live verification through the normal allocated release path before claiming demo/live accuracy or relying on this display for client-money decisions. No broad audit or new harness is required.

DEMO-EXP-001 update: PR #620 merged at 0f40a0a8; nine focused tests now include rendered chart failure versus empty and Borrow link, and the one allocated Frontend validation run 41951365095 passed its frontend build/auth checks at exact 984813f1. Those omissions are closed. Exact live Borrow query and deployed-version/live accuracy proof remain open for the normal separate release action; no deployed fix is claimed.

- [SEC11 native-XRP deposits — SEC11-DEF-01](demo-deferred/sec11-xrp-repeat-deposits.md), owned by Miles; custody credit is in scope, public Explorer/points projection is deferred.

### DEMO-S2-001 — Sustained points resource measurement

Owner: Orin with Dorian. The Season 2 source candidate has a disposable one-row
import/upgrade/first-epoch measurement, but sustained polling with the complete
participant cohort is unmeasured. A longer load study is deferred so it does not
delay the bounded demo release. Containment: use deployed network pricing for the
explicit funding reserve, retain a memory envelope, and observe live balance and
memory after any separately authorized installation. Before relying on this
service for client-money incentives, measure sustained cohort consumption and
revisit the reserve and alert threshold. Status: open; no live installation or
funding is claimed. Source evidence: branch `codex/season2-accounting-20261110`,
`docs/coordination/season2-accounting-implementation-20261110.md`.

### DEMO-S2-002 — Repayment-token provenance in prospective points

- Owner: Orin. Feature: inherited 5x qualifying stable repayment windows.
- Known limitation: the source event decoded by Points has no stable-token attribution; do not equate missing attribution with a proven ineligible repayment. The examined historical repayment was proven icUSD by its ledger block; no eligible stable case was observed there.
- Deferral: Rob directs ongoing scoring to run independently of historical accounting. Existing categories/authorization/custody remain unchanged. No claim of automatic full 5x coverage is made.
- Follow-through: retain and reconcile any new repayment cases with ledger provenance; implement source-bound attribution and validate a qualifying case before claiming that category automatically complete or offering client-money readiness. Preserve the disjoint Nov1→T0 historical prefix and document any separately approved later correction.

- [Season 2 repair — S2-REPAIR-DEF-001](demo-deferred/season2-repair-20261121.md), Orin: live release and recovery remain held; sustained resource study stays deferred.
