# Demo-first delivery policy

Effective October 29, 2026, by Rob's direct instruction. This changes the delivery milestone to working, demonstrable features. It supersedes older agent-created requirements for optional hardening, speculative scenarios, repeated reviews, redundant test passes, and extra infrastructure before a demo. It does not change product authorization or custody semantics.

## What to do now

Choose one concrete user flow, name the smallest change needed to make it work, and carry it through the existing authorized release path. Reuse the current team, tools, and valid evidence. Prefer a working feature over more preparation documents, new test frameworks, or another review committee.

Run the smallest useful functional/smoke check and the checks actually required by the repository or deployment mechanism. Reuse passing results unless the change invalidates them. Do not require a full local suite merely to pre-qualify the same coverage in required exact-head CI. Do not default to second or third independent review rounds; use focused review only for a concrete changed risk or a required repository approval.

Defer hypothetical attack research, exhaustive edge-case matrices, additional fault injection, broad audits, optional performance work, wrapper/framework improvements, and repetitive verification that do not resolve an actual demo or release blocker. A proposed blocker needs a concrete broken demo step, an accepted finding with actual impact, a cited existing invariant, or an enforced external requirement. An uncited worry goes in the revisit log.

Do not spend another tranche proving that an optional check can be run. In particular, an optional compile-cost measurement or its unused runner is not a feature gate. If it is unnecessary for the selected delivery path, retire it from the demo critical path and record it rather than repairing it just to keep working.

## What remains essential

- The selected feature must build/load and perform the claimed demo flow. Report what was actually demonstrated and what was not.
- Keep basic authorization, client/associate separation, secret protection, and the integrity of the action actually approved/signed. Fix or contain a known defect that exposes existing assets, permits unauthorized actions, loses state, or prevents the demo. Do not replace this with a new general security audit.
- Existing money-bearing services and real production incidents retain their safety and correctness obligations. Use an appropriately isolated demo/test/operator context for unfinished features; this policy does not authorize risking existing client assets.
- Preserve source, artifacts, actual resource bounds, mandatory branch checks and normal release controls. Do not disable required checks, hide failures, falsify proof, or retry a rejected action through another route.
- Existing financial, signing, governance, controller, installation, publication/private-source and other exact action holds remain separate. This policy is not permission for a transaction, a permission change, or a denied action.

If an optional check fails, determine whether it exposes a concrete essential failure. Do not silently waive an essential failure; equally, do not turn every unrelated test failure or theoretical concern into a release blocker.

## Revisit before client money

Maintain the project ledger linked from [DEMO_DEFERRED_WORK.md](DEMO_DEFERRED_WORK.md). Record each actual shortcut or omitted investigation with a stable ID, feature, owner, concern, reason for deferral, demo limitation, and the verification/remediation needed later. Link existing findings and artifacts rather than copying private logs. A supervisor may own a separate lane file and link it from the ledger to avoid overlapping edits.

Logging must be brief and accompany delivery, not become another planning workstream. No fictional risks or completed work should be invented to populate the log.

Before a feature is offered for client money, revisit every open entry affecting it, perform the relevant hardening/verification, and record a disposition with evidence and reviewer. Material residual risks require an explicit decision, not a blanket demo-policy waiver. Demo-ready, merged, deployed, and client-money-ready are distinct claims.

Supervisors must give this policy to their existing Juniors/workers and apply it to their next concrete delivery action. Keep the existing boards authoritative, the current priority order, shared resource scheduling and quiet monitoring cadence. No new team, timer, standing committee, approval ritual, or delivery framework is required.

## Active queue pruning — October 29 follow-up

Remove nonessential planned testing from active cards, dependencies, next actions and ranked priorities now; labeling a gate optional while leaving it queued is insufficient. Keep its stable ID, owner, previous scope/evidence, reason and revisit-before-client-money condition in the existing deferred ledger or recoverable archive. Never mark omitted tests Done or add/refresh completion timestamps.

A retained planned check must name the actual selected demo step, known essential defect/existing-asset risk, or enforced repository/release requirement it addresses. Old agent-authored DoD, generic defense in depth, extra reviewer counts and speculative uncertainty do not justify an active test gate. Retain existing tests as code and already-passed proof; do not weaken enforced checks or interrupt healthy required CI.

An unresolved transaction must be reconciled before retrying that attempt. Its unknown outcome does not make additional speculative tests a gate on unrelated source/UI delivery. Keep financial/action/resource holds separate.
