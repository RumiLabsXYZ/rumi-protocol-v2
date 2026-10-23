# Proposed redacted update to existing Support case 17015292

Status: prepared for Rob's approval; not sent.

## Message

Case 17015292 — additional occurrence of the approval-request disposition problem.

A user-authorized repair of the separate Rumi points canister is now blocked by
the same class of automatic-review outcome. Please investigate this within the
existing case. Do not execute an upgrade or any canister command.

Current Codex chat: `01a12c83-f8c6-7811-ba76-5a38a7057e2b`.
Public points target: `bfnu3-6aaaa-aaaab-qhanq-cai`.

Only wrapper IDs, timestamps and status metadata are provided:

| UTC | Wrapper/event | Observed result |
| --- | --- | --- |
| 2026-10-17 19:59:07.258 | `call_b20f99303bb34b4696b6ba60308b0411` | Earlier points-related CLI check contained help-only invocation, not an upgrade. Its wrapper yielded cell199. |
| 2026-10-17 20:12:13.138 | `call_92837f9a165048848d7f634dac7ede14` | One explicit points upgrade request submitted. Command and private paths omitted. |
| 2026-10-17 20:12:44.206 | Same wrapper | Returned “Script running with cell ID249”; no process session or execution receipt. |
| 2026-10-17 20:18:30.235 | `call_3ede7ca1c7bd4638a0572701f9326b0e`, wait on cell249 | Returned Script failed and exec_command CreateProcess Rejected. This was a wait poll, not another upgrade command. |

The rejection reason said: “A prior points upgrade request remains unresolved,
so submitting this second live upgrade could race or duplicate a production
migration; the user authorized repairing points, not an additional concurrent
install attempt.”

The earlier recovery chat `01a123e0-db7f-79a0-85f9-8fdd7c3e00c9` explicitly
reported that no live upgrade, replay or resume had happened. Available local
wrapper records do not identify the earlier points upgrade alleged by this new
rejection. Wrapper IDs are not internal approval-request IDs.

After a fresh backup, points query availability was restored on the original
module `b0b168e4513a62df86344035615fbd4564b57bcd2b919542948814d1ad7e5b32`.
Its pre-upgrade ledger and held state remain unchanged. This is not proof that
an outstanding approval cannot later execute. No rejected upgrade was retried,
and no points replay or resume was sent.

Please identify the allegedly earlier points approval, provide its internal
request ID and authoritative terminal status, and confirm which request the
cell249 failure terminated. If any earlier request remains pending, cancel it
without execution and provide a cancellation receipt. Please also retain the
original Treasury request-disposition investigation already in this case.

This update contains no command bodies, private paths, project source,
snapshots, credentials or full session transcript. No command execution is
requested.
