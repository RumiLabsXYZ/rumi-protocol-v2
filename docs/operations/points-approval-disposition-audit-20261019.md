# Points approval disposition audit and proposed reconsideration

## Verified result

Two independent read-only reviewers inspected the original points upgrade request and found the submitted wrapper terminally rejected. The rejection refers to an earlier points request without identifying it; that request's status remains unknown and the rejection is not a cancellation of it.




## Separate new precedent, not points authority

A separate ckSOL release encountered the same class of duplicate-request rejection and a later reviewed request succeeded. That does not clear the points hold, establish cancellation of an unidentified request, or authorize an alternate executor.

## Concrete proposed action, not yet submitted

The proposed revised points source is PR583 commit
`ef953f0e4d9b44081fc768e294bc02eb2893836e`, described in
`points-late-recovery-v2-20261018.md`. Exact candidate:

- Raw SHA256 `1b568068d14c43e89ec90f3958c39bdbcdd17defe86a455e9f86976a2a3cfe7e`.
- Gzip SHA256 `a7a1599965c10b99ba4f2b3fe6bf176e47b6a16a6258055774ddebf75ab63787`.
- Target `bfnu3-6aaaa-aaaab-qhanq-cai`, network `ic`.

If the owner explicitly authorizes reconsideration despite the remaining unidentified
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
