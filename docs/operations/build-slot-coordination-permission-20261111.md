# Exact cross-project build-slot coordination permission

Status: messages unsent; no XLM allocation or build issued by Dorian. This is a
communication permission request, not a new approval gate for Mara's existing
implementation/test scope and not a new scheduler or team.

Dorian's composed frontend build closed PASS at08:48:50 UTC on November11.
During the next XLM admission preflight, the existing shared host ledger acquired
another LOCAL reservation, so no admission was written. That reservation was
subsequently released. Capacity alone is not a delivered scheduler grant.
Mara's inspected local-only candidate remains
`f26c10d65ebf35636ad6bb89e3e47f3a35a31c2d`, unbuilt.

Automatic approval review rejected the attempted cross-project coordination
message for unestablished destination/payload authorization. It was not sent or
retried, and no alternative sender was used. The tool's reason referred to a
separate delegated-vault scheduling destination. Existing slot coordination
must be completed through established owners; Dorian will not modify their
allocation ledger or launch a competing build unilaterally.

## Permission requested

Allow Dorian to exchange the following bounded scheduling/source/status updates
with existing **Cody | protocol support fleet supervisor**
(`01a0eeee-d768-71b1-8090-0ca4e680e5b3`),
**Mara | Enable native XLM collateral**
(`01a17f0e-495a-7df1-b590-8ef1bf5dc4d5`) and
**Thayer | Delegated Vault CTO**
(`01a1a4b6-4885-7170-9fc9-8c8a68e1ab80`).
Keep Thayer informed. Include only the named source pin, batch scope, actual
resource occupancy/headroom, allocation identity/start deadline/limits,
acknowledgement and native result/process-group closure. No customer identities,
balances, signing payloads, transaction hashes or unrelated private data.

Initial messages:

- **To Cody:** Dorian's preceding OISY frontend build closed. Please reconcile
  the existing shared queue and admit the next eligible single bounded Mara
  batch at f26c10d6: focused test Wasm → xlm_confirm_pic → production compile
  guard check. Preserve swap/active-release precedence, no concurrent heavy
  build or paid-CI expansion. Return actual admission/limits or the real queue
  blocker; do not treat this request as an allocation.
- **To Mara:** The source pin and bounded three-step batch are ready. Wait for
  the actual shared-scheduler admission, acknowledge its identity/start deadline
  and limits before dispatch, then retain native results and owned-process
  closure. No production activation, release, financial action or stale grant
  replay follows. Correct concrete test failures under the existing project
  scope and obtain ordinary fresh scheduling as needed.
- **To Thayer:** Dorian is coordinating the above existing cross-project host
  slot with Cody/Mara after the urgent frontend build. No financial or deployed
  scope is expanded; the existing shared resource record remains authoritative.

This does not authorize retrying or relaying any rejected Perry allocation or
other denied payload, hosted CI, production/publication changes, fund movement,
controller changes, new workers or a new standing process. Resource admission
and actual executor acknowledgement remain separately evidenced.
