# Vault 51 corrective release — November 11, 2026

Rob directly approved exact correction packet `097bf775d3181de5936058ef01e21704d8cfc0ab`,
including pinned upload `730f54713dca1938c40b661ef6bd6ddc23555d92ca4c8e1feca282e3a33f8cf9`,
the preservation procedure, automatic checks and eventual verified escrow sweep
and same-vault ICP adoption. The original packet9d02bea8 was already installed;
its raw-zero defect was contained before spend. This is the separately approved
correction, with unchanged financial scope and no rebuild.

## Execution

Corrected source `312ebe036a7651365c293294bedc8cb3a6585179` is installed in upgrade
mode on existing mainnet backend `tfesu-vyaaa-aaaap-qrd7a-cai`, using `rumi_identity`
and the exact approved mode-preserving arguments. Controller status is Running,
module hash matches the exact gzip, embedded API matches the unchanged canonical
397-method Candid, and settings/controllers are unchanged. Install and start were
each dispatched once with durable markers and known successful results.

The exact oldest snapshot11 was already fully archived and verified before its
one-slot replacement. Fresh stopped backup1b was downloaded and verified against
the current43bbbca9 module; all named lengths/hashes and metadata were verified.
All other nine snapshots, including prior release backup1a, remain unchanged.
No standalone deletion, restoration, reinstall or old-writer rollback occurred.

The first local raw-state comparator paused before install because debt accrued
between query and stop, and heap memory retained an older copied record. No
management write was replayed. The repaired read-only verifier follows actual
BTree keys, lengths, parent/index and root child links for all49vault entries,
uniquely identifies the active Vault51 record and verifies exact retained original
collateral, owner/type and bot state. Stopped-snapshot debt/interest values are
used to reconcile that pre-install accrual, rather than declaring the earlier
public query byte-equal. Source guards and pinned candidate were not changed.

## Live verification

The first automatic check reconciled the exact existing WaterNeuron request at
**2026-11-11T16:18:15.264700035Z**, passing the corrected original-vault guard.
The claim remains `ClaimConfirmed`, with `last_error = null`; the sole claim
change is its reconciliation timestamp. The same timestamp appears in the public
Vault51 claim projection. All other scoped projections match, including SEC11/XRP
receipts, SEC12 recovery holds, customer ckSOL state and completed Vault158 work.
Vault51/158 debt and accrued interest match their actual stopped-snapshot map
records, accounting explicitly for interest accrued between query and stop.
Fixed configuration matches under the existing documented price/rate exclusions.

**Waiting for WaterNeuron payout:** dedicated escrow is zero; no received ICP,
settlement block, sweep/adoption journal or owner payout is present. No financial
action was manually invoked. The ten-second automatic check is live-verified;
the hourly timer is registered by the exact deployed source. Future settlement,
verified sweep and completed adoption remain separate outcomes, not claimed here.
Adrian independently queried controller status and the corrected canonical claim
API: exact Running module, same first reconciliation timestamp, immutable anchors
preserved and no settlement/sweep/adoption. No second build/review campaign ran.

See [execution result](evidence/vault51-corrective-live-20261111/execution-result.json),
[final reconciliation](evidence/vault51-corrective-live-20261111/final-reconciliation.json),
[approved pins](evidence/vault51-corrective-live-20261111/approved-pins.json) and
[fixed configuration](evidence/vault51-corrective-live-20261111/configuration-comparison.json).
The matching Vault51 frontend is already included in published version1794386914589;
no frontend replay or replacement was performed for this backend correction.
The independent OISY acceptance failure remains open under Ellis.
