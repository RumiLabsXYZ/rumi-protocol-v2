# Narrow backup-capacity amendment

## Reason

The approved retained-route inspection proposal at `d34b00e2b39267f2dd6035662109d216ed20061b` requires a fresh protected backup and says not to prune older backups. All ten managed snapshot slots are occupied. Replacing a managed snapshot deletes that network snapshot identity after the new snapshot is successfully created, even when its complete payload is preserved locally. The existing backend-upgrade risk authorization is not treated as permission for that exception.

## Exact additional action proposed

Authorize replacement of **one** managed snapshot on the already-authorized production backend `tfesu-vyaaa-aaaap-qrd7a-cai`, network `ic`, using the operator's authorized identity:

- Selected snapshot: the oldest entry in the verified inventory.
- Preserve its complete downloaded module, Wasm memory, stable memory and metadata in the protected recovery store. Verify every named payload's length against its metadata, its snapshot timestamp against the managed inventory, and every retained SHA256 before replacement. The declared chunk store is empty.
- Only after that preservation passes, use the normal snapshot `create --replace` operation for this exact ID during the bounded stopped-backend backup. The old network identity and direct network restore path will disappear; the complete verified local backup payloads remain protected. No restore roundtrip has been performed or is claimed.
- Do not delete or replace any other snapshot, discard any downloaded backup, clear a marker, restore stale financial state, or repeat a failed/unknown snapshot creation. Compare the entire live snapshot inventory with the fresh preflight inventory before replacement, and verify the other nine entries remain identical afterward. If the inventory or selected snapshot differs, halt and reassess.
- Verify every fresh snapshot payload, then restore the old backend's Running availability before the one previously authorized upgrade can be submitted. An unknown backup outcome halts upgrade submission.

The snapshot service's aggregate size is not assumed to equal the sum of the three binary files. The initial offline aggregate assertion failed with a128-byte difference; the original download succeeded, all named payload lengths and timestamp match their metadata, and the difference is preserved as a diagnostic. No download retry, replacement or production update occurred during that correction.

## Unchanged authority and limits

This amendment permits only the fully preserved backup-slot replacement described above. It does not grant a second backend upgrade, a pool/Treasury install, global resume, timer/setter call, credit, mint, financial replay, journal deletion or legacy promotion. The existing single backend inspection-upgrade authorization remains unconsumed and still requires normal approval review, fresh preflight and verified backup. All pool and other financial holds remain in force.

## Review status

Two independent reviewers passed amendment preparation, conditional on its separate explicit authorization. The operator requires literal JSON true for that authority and remains fail-closed without it. No snapshot replacement, backup stop or new upgrade has been submitted.
