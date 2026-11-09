# SEC12 deployed result — November 8, 2026

The saved vault-pull restore/continue and explicit cancellation repair is deployed from source `f1d2e77e2387ab6b9fc9a5b5676047b25d7175ff`. Backend is Running with exact module `2ca84c209d537def37f793588e469e34b3ee7301d672e3963ce7d88803c0fd13`. The primary domain serves frontend version `1794195655486`, and its root identity/gzip, version and both authentication files match the frozen build.

The single backend/frontend build passed; 69 focused source checks and both corrected-source reviews passed. Global typecheck retains 30 pre-existing errors and 66 warnings, with no introduced diagnostics; it is not claimed clean. Actual embedded public API and all 396 exports match, and the previous API is backward compatible.

Stage batch 684 added 303 keys before backend mutation; activation batch 685 changed only the six index/version identities plus one associated gzip encoding. Final stable double-read: 3,687 keys and 3,881 encodings. All original keys/properties and untouched encodings remain. All 305 stage, nine final canister and five primary-domain served checks passed. Commit/Prepare permissions and both canister settings are unchanged.

The backend was stopped once, controller Stopped verified, and the oldest fully archived slot replaced by a fresh stopped snapshot. The fresh snapshot was downloaded and verified against the previous module; all other nine snapshots remain. Upgrade mode was null. Exact stopped new module/API was checked before one start. No restore, reinstall or write retry occurred.

Retained-state comparison: **10 byte-equal scoped projections plus one independently reconciled pre-install XRP claim transition**, with normalized fixed configuration equal. The claim count and IDs are unchanged; one existing legacy claim has a fee and replacement-settlement change. The replacement hash and all changed fields were present in the old backend stopped heap before installation. The source owner/case coordinator independently checked local Candid, snapshot and source evidence. No financial fields were omitted to pass comparison. This is not 11 byte-equal reads or external payout-finality proof. Detailed state/snapshots remain private.

No customer request was continued, cancelled, acknowledged or cleared, and this release made no loan, approval or real collateral movement. Caller-scoped operation status is the operator view and cannot establish customer phase. The customer's intent and actual phase remain unknown pending their authenticated readback. Restore displays the exact saved request; Continue and confirmed cancellation remain separate user decisions. Cancellation retires the ID without implying no prior effects or revoking an allowance.

Future writers must preserve the existing receipt maps/private journal and new cancellation replay fence. Older writers are not a safe rollback after these events. The existing case owner retains customer communications and Lookout publication. Demo-ready, deployed and client-money-ready remain distinct.

An optional external XRPL lookup was rejected by automatic approval review because it would transmit private transaction hashes to an external service without explicit egress authorization. It did not run and was not retried; the local stopped-snapshot reconciliation above resolved the release hold.
