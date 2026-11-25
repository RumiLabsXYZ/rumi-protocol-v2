# Season 2 history: earlier Stability Pool state found — November 25, 2026

Owner: Orin, S2-HISTORY-01. Follow-up to the [opening-input assessment](season2-history-opening-inputs-20261124.md).

## Result

**An authentic earlier saved SP state has been recovered. Both missing depositors are present with positive balances. This establishes an earlier input, not either November 1 opening balance. Numerical allocation remains stopped.** No simulation, replay, historical award, compensation or financial/release mutation was performed.

The existing management snapshot inventory contains five pre-November1 SP snapshots, dated October18, October21 (two), and October23 (two). Only the latest October23 snapshot was downloaded, using the read-only snapshot download operation. No snapshot was created, loaded, restored or deleted; no canister was stopped or restarted.

## Provenance and independent checks

- Canister: `tmhzi-dqaaa-aaaap-qrd6q-cai` (SP), network `ic`.
- Existing snapshot: `00000000000000040000000001f088fd0101`, outer capture October23, 2026 **15:31:36.132303355 UTC**, management size38,376,804 bytes. Download metadata matches the management listing.
- Retained module SHA256: `53ccb6adaaacf56281858d0c11a5bc7b277174a94acbd5414cf0737fcd3e0191`. Its exports include `get_user_position` and `get_pool_events`; the module was not executed.
- Directly read the observed MGR1 memory layout and SCL1 cell at virtual memory1. The Candid saved-state payload is239,625 bytes, SHA256 `1517b37b5b8874c24fb03b3d746a563fefd7dd0bf37748137f9295931fd240b1`.
- The payload contains13 depositors and2,181 events, IDs0–2180, with saved next-event ID2181. Every decoded event exactly matches the already retained T0 history prefix. The sum of depositor balances matches every saved pool total, treating an absent balance entry as zero; one extra registry total is explicitly zero.
- Both previously missing depositors have positive balances in one stablecoin each, empty opted-out sets, empty explicit chain opt-ins and empty native payout-address maps. Those fields are observed in the actual wire record, not supplied by decoder defaults. Individual balances, owners and raw state remain private.

## Clock and interpretation limits

**The outer snapshot capture time is not the timestamp of the decoded saved-state cell.** The saved event prefix ends at October23 **08:19:17.716813022 UTC**. The cell can retain an earlier pre-upgrade serialization while heap state has subsequently changed. The read path is consistent with the retained receipt-store layout, but the inspected October23 candidate manifest does not match the snapshot's module hash; it is not used to claim exact deployed-source identity. No fresh execution was used to turn saved state into an asserted live heap checkpoint.

The earlier21-pair primary recovery-directory inventory remains accurate within its original bounds. It did not cover this management-retained snapshot inventory. Existing owner release records supplied the specific SP snapshot lead; later November15 owner downloads are not opening snapshots.

## Next retained-input coverage result

The already retained event stream has **190 events, IDs2181–2370**, between this saved cursor and November1 UTC:179 InterestReceived,2 OperationsResumed,1 EmergencyPauseActivated,4 OptOutCollateral and4 OptInCollateral. There is no Deposit, Withdraw or liquidation event in that recorded interval. Neither missing owner is the caller of an intervening event. These counts were computed from existing decoded records, without extracting receipts again or replaying balances.

This is useful transition coverage, not proof of complete balance reconstruction: event absence does not exclude unrecorded or administrative changes, and historical implementation/upgrade behavior remains unbound. A numerical opening calculation still requires establishing the saved state's effective checkpoint and applicable deployed rules/transitions through November1, including preference/registry evolution and all eligible-pool effects. The pinned November rules alone do not prove October execution semantics. No opening balance, per-owner interest amount, rounding recipient or compensation is asserted.

The next genuinely ready item is source-and-receipt reconciliation of that October23→November1 transition boundary using retained owner records, without simulation, private export or financial actions. Prospective scoring/capture timers are untouched. Existing financial, historical-credit, publication and unrelated owner holds remain intact; this report and the existing sanitized incentives board use their established publication authority.

## Completed follow-on: stronger October23 live query checkpoint

The next retained-source check found the existing `reopened-live-08` SP query receipts in the October23 rails-restoration archive. All13 `get_user_position` calls succeeded and returned positions; the13 unique requested principals exactly match the independently retained `sp-owners` response and reported depositor count. Both missing owners are present. Summed per-owner stablecoin balances reconcile to the contemporaneous `sp-status` totals, including zero normalization. Raw responses remain private.

The separately receipted roster/status/position reads cover **October23 19:32:25.986839–19:32:51.318955 UTC**. The retained source stream has no event inside this window: the preceding event is2192 and the next is2193. This is a coherent complete observed read set, **not an atomic snapshot**, and does not exclude unrecorded changes. Unlike the earlier stable cell, these are actual recorded query responses, so the cell's pre-upgrade save clock is not used as their observation time.

From that later observed cursor through November1 UTC there are **178 retained events, IDs2193–2370:170 InterestReceived,4 OptOutCollateral and4 OptInCollateral**. No balance replay, fresh simulation, repeated receipt extraction or individual numerical allocation was performed. The ready follow-on is now verifying applicable deployed implementation/upgrade and administrative transitions for this narrower interval. Earlier saved-state provenance is preserved as independent corroboration; neither observation is mislabeled a November1 opening balance.
