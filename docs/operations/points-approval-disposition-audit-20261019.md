# Points approval disposition audit and proposed reconsideration

## Verified result

Two independent read-only reviewers inspected the original points request and
its exact wait. Both find the submitted wrapper terminally rejected:

- `call_92837f9a165048848d7f634dac7ede14` submitted October17 20:12:13UTC,
  yielding cell249.
- `call_3ede7ca1c7bd4638a0572701f9326b0e` was a wait on that cell, not another
  install. Its October17 20:18:30UTC result is Scriptfailed plus exec_command
  CreateProcessRejected. The rejected shell command did not start.

The rejection alleges an earlier points request but supplies no identifier.
That alleged request's internal status remains UNKNOWN_UNIDENTIFIED. No queue
lookup or cancellation receipt has been obtained. The terminal wrapper result
is not a cancellation of that unidentified alleged request.

The bounded current-chat audit found22related wrappers, with only help/read
matches before the one actual install submission. The independent precedent
audit inspected991pre-submission calls in the originating rollout and the
specific historical recovery/security rollouts. Historical successful installs
do not identify the request alleged by the October17 rejection. Absence outside
the inspected records is not proved. Zero guardian records are not queue proof.

## Separate new precedent, not points authority

The separate ckSOL release encountered the same class of duplicate-request
rejection. After a terminal rejection, bounded audits and independent review,
one request through the same automatic approval path succeeded. Its deployed
backend hash3b51aca4 is independently observed in the current production read.
This does not clear the points hold, establish an unidentified request's
cancellation or authorize an alternate executor.

## Concrete proposed action, not yet submitted

The proposed revised points source is PR583 commit
`ef953f0e4d9b44081fc768e294bc02eb2893836e`, described in
`points-late-recovery-v2-20261018.md`. Exact candidate:

- Raw SHA256 `1b568068d14c43e89ec90f3958c39bdbcdd17defe86a455e9f86976a2a3cfe7e`.
- Gzip SHA256 `a7a1599965c10b99ba4f2b3fe6bf176e47b6a16a6258055774ddebf75ab63787`.
- Target `bfnu3-6aaaa-aaaab-qhanq-cai`, identity `rumi_identity`, network `ic`.

If Rob explicitly authorizes reconsideration despite the remaining unidentified
request uncertainty, first revalidate artifact/interface hashes, identity and
controllers, sole ownership, current state/source bindings and a fresh verified
backup. Submit at most one new request through the original automatic approval
path, presenting this new evidence. No alternate executor, indirect command or
native UI execution. Honor any further rejection; authorization is not approval
system bypass or automatic permission for another request.

The original strict V1 resume window has expired. The proposed binary contains
the separately reviewed V2 recovery path. Source/compile/Candid/canonical exact
repeat checks passed; tests were not added or run. Retained-state and actual
runtime preservation still require verification before a restoration claim.

This proposal does not approve a numerical allocation. Its digest remains
unset, and missed credits remain unapplied. Actual completion still requires
continuing processing and verified applied credits for every missing interval.
