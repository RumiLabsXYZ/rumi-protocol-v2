# Season 2 history: October checkpoint transition rules — November 25, 2026

Owner: Orin, S2-HISTORY-01. Continues the [earlier-state result](season2-history-earlier-state-20261125.md), without treating October23 positions as November1 opening balances.

## Concrete result

The retained release receipts bind the October23 live13-depositor checkpoint to **SP source8d7c7e7587d0e9cbb693803a90cbbbb715ed85b4**, installed gzip SHA256 **827e272ab4a72459e9eee99a24d3e1994083ddf135825b9ec2a9d92e7f6db2ca**. The actual install completed October23 **19:23:47.556865 UTC**, exit0. Both post-install management output and the19:32 live-checkpoint management read report that hash. A retained October25 copy of the gzip independently rehashes to the same value; its source/artifact binding agrees with the release manifest.

The same module is positively observed in retained October25 management receipts, latest **16:17:26.729859 UTC**, and again in the November1 preflight status output recorded at **08:33:08.236 UTC**. The latter belongs to the existing “Remove sunsetting collateral from UI” owner chat; its SP upgrade attempt starts later, at08:51UTC, after the November1 opening cutoff. This later attempt is not moved backward into the historical interval.

These are observed module identities at specific times. **They are not a complete continuous installation history and do not alone prove there was no intermediate upgrade or state restore.** No retained management change-history response was found in the bounded recovery/output inventory or inspected specific owner records.

## Rules actually bound at those observations

`state.rs` and `types.rs` at8d7c7e75 are byte-identical to those at the previously documented4117ba1b2dab87dd03994feada05fd5e62e1d648 rule pin. Their SHA256 values are respectively `be4afed462b7e264ec52733083149e074a1d0abbe2363980c5caf3dd9286e723` and `e5af573330643ef0e492dff43446630b12746fae78b0d10b4744cf172ed71f4a`. This comparison does not promote the entire later canister source to deployed identity.

At8d7c7e75:

- `state.rs:1575–1659` selects native-XRP eligibility by exact principal/payout address, chain sentinels by explicit opt-in, and ordinary collateral by opted-out membership.
- `state.rs:1706–1785` weights the entire eligible pool by pre-credit icUSD balances, floors each proportional credit, assigns dust to the first eligible Rust Principal BTreeMap key, and updates balances/totals for the next payment. Zero eligible weight produces no credit from that function.
- `lib.rs:1119–1134` keeps the old unkeyed notification method fail-closed. `lib.rs:1139–1218` authenticates V2 notifications, binds the exact mint-block payload, handles duplicates without a second distribution, and emits InterestReceived only in the newly distributed branch. The event itself omits the collateral route and source mint block; its amount alone cannot establish eligibility or a unique payment receipt.
- `lib.rs:1584–1641` emits registry-change events for the inspected stablecoin/collateral registration paths. `lib.rs:2276–2301` emits BalanceCorrected for the inspected administrator balance-correction path. No such event appears among2193–2370. That absence is meaningful under this source, not independent proof of all historical code/controller actions.

The178 source events remain170 InterestReceived and8 preference changes (four each OptOut/OptIn), through event2370 atOctober31 21:55:37.673906UTC. No replay or per-owner arithmetic was performed.

## Actual administrative action reconciled

The retained October25 legacy53 operation is backend notification cleanup, not another SP allocation. The pinned backend `treasury.rs:4512–4552` obtains an existing positive sealed recovery receipt and removes only the exact matching notification; it does not mint or distribute. The retained53 terminal replies and existing execution report document completion with zero additional credit. All13 pre/post typed SP positions independently compare equal. The completed237-packet recovery was preserved; its historical credits are not replayed as current interval interest. Existing ordinary-interest observation evidence is reused, not queried or repeated.

## Exact stopping input and scope of search

Neither owner's November1 opening balance is directly recorded in the inspected inputs. Calculation remains stopped because the October checkpoint has not been linked to that cutoff by a complete **effective state-transition history**: the missing installation/restore continuity record must identify any intervening implementation/state transitions, or an authentic November1 per-owner checkpoint must replace that need. The178 amount/preference events and matching module endpoints do not supply that input. Per-payment route/receiver binding for the170 pre-November1 payments also has not been established by the already completed221-payment November1→T0 receipt result; the intervals are disjoint.

New source checks were limited to existing October23 rails-restoration/reopening records, October25 legacy53 and interest-audit bundles, and three specific retained owner chats identified by the app inventory: the original earnings investigation, October29 missed-liquidation investigation and October31 sunset-collateral release. The prior Bram/Dorian source inventory was reused. No new continuous-history artifact or direct opening checkpoint emerged from these leads. This is an explicit bounded absence result, not a host-wide claim that the evidence cannot exist elsewhere.

A useful next input would be an already-retained SP installation/restore chronology covering October23 19:32UTC→November1 00:00UTC (especially after the October25 observation), or an authentic November1 opening-position checkpoint. No new ready retained location for that input is identified in the inspected inventory. No policy decision, estimate, forfeiture or compensation is substituted for it. This is the specific input stopping the calculation, not another test/review gate.

Only retained records/source and local comparisons were read. No production financial/release call, fresh simulation, receipt re-extraction, private export, award or compensation occurred. Existing prospective timers and all unrelated action holds are untouched.
