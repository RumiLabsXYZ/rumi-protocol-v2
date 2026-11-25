# Season 2 history: two-owner opening inputs and interest rules — November 24, 2026

Owner: Orin, S2-HISTORY-01. Follow-up to the [221 ledger-bound interest-mint provenance result](season2-history-interest-provenance-20261124.md).

## Result and stopped calculation

**The inspected retained records do not establish either owner's November1 opening icUSD balance. Numerical per-owner interest allocation is stopped; no baseline was invented.** Both owners' later positive balances remain included as required pool participants, not assigned zero or dropped because they are outside the15-person Points cohort.

One saved position's first-deposit timestamp predates the first retained SP event. There is no owner-addressed deposit/withdraw/correction record for it in the inspected0–2595 stream. Global interest and liquidation events can still change its balance; absence of an owner-addressed event is not zero balance.

The other owner has one recorded deposit, with timestamp matching the saved position's first-deposit timestamp. That establishes an actual deposit input, not its November1 balance: earlier per-owner interest and any loss effects must still be accounted for. Neither owner has a matching original late-November1 position anchor. The11 other cohort position anchors are late-day observations, not a complete midnight pool checkpoint.

A bounded check of21 existing primary-checkout recovery snapshot/module pairs found no SP snapshot in that set. Backend backups provide mint journals, not the SP depositor state. This is a scoped input-availability result, not a claim that no opening checkpoint can exist in another retained owner archive.

## Eligibility evidence that is available

Both saved November10 endpoints have empty opted-out sets and empty native payout-address maps. Each explicitly lists five of the six interval collateral routes as eligible. The missing route is verified by the exact `rumi-xrp-native` principal identity, not the XRP symbol; pinned native-XRP eligibility requires a stored payout address.

No interval OptIn/OptOut event for either owner is present in the retained stream. That trace is consistent with unchanged preferences under the pinned event-emitting API paths. It does not supply a direct opening preference/registry snapshot or independently prove every historical eligibility transition. **Endpoint eligibility is established; opening eligibility is not promoted to fact from it.**

## Pinned allocation rule

Source pin: `4117ba1b2dab87dd03994feada05fd5e62e1d648`.

- `src/stability_pool/src/types.rs:188–218`: weight is the raw icUSD balance selected by the stablecoin registry. Other stablecoin/3USD holdings alone do not earn this interest. Ordinary collateral uses the opted-out set; chain sentinels use explicit opt-in.
- `src/stability_pool/src/state.rs:1575–1659`: the exact native-XRP principal requires a payout address; chain sentinels and ordinary collateral use their respective predicates.
- `src/stability_pool/src/state.rs:1706–1785`: for payment amount A, take each eligible owner's **pre-credit** icUSD balance b_i, with D=sum(b_j) across the **whole eligible pool**. Credit floor(A*b_i/D), then give the remaining rounding dust to the first eligible key in the pinned Rust Principal BTreeMap iteration. This is not Points rank, query-file order or largest balance. The updated balances affect later interest weights. If D=0, the distribution function returns without owner credit; surrounding receipt/treasury routing is separate evidence and remains held.
- `src/stability_pool/src/state.rs:4854–4867`: the endpoint eligibility list is computed from the registry and current position predicates; it is not a recorded historical eligibility timeline.

These are pinned source semantics, not a fresh simulation or a claim that every historical deployment byte has been re-established. The mint receipts prove delivery tuples, not b_i, D, dust recipients or per-owner distributions. Using only the15-person Points cohort for D would omit actual SP depositors and contradict this rule.

## Exact missing inputs

A numerical allocation needs the two owners' opening icUSD balances from an authentic November1 checkpoint, or an earlier complete checkpoint with every intervening balance transition; their opening collateral preference/registry state; and the full eligible pool's pre-credit balances at each payment, including preceding interest and liquidation effects. None is replaced by a deposit principal, an eventual balance, a lifetime interest display or an assumed default.

The next independent retained-input check found a complete later13-position global read set. Its local file writes fall in a no-recorded-event window before the next SP event; the48 subsequent source records through T0 are all InterestReceived. Those facts describe a **later endpoint/proxy**, not a November1 opening checkpoint. No new allocation or replay was run from it, and it was not substituted for missing earlier state.

No fresh simulation, repeated receipt extraction, build, live canister call, credit, compensation, private export or policy change occurred in this follow-up. Existing prospective scoring and scheduled captures remain independent. Next useful accepted work is locating authentic earlier SP state in recorded owner/release evidence; the stopped calculation resumes only when its required inputs are established. All existing action holds remain unchanged.


## Next ready retained-source check: analytics capability

The next accepted input-availability check inspected unchanged analytics source at75db04935a9e1a6119af75d625e4e0c68fa815e3. Its daily SP collector stores pool aggregates (`collectors/stability.rs:6–27`, `storage/mod.rs:335–343`), not per-owner balance checkpoints. `queries/live.rs:807–853` computes the depositor `current_balance_e8s` field from deposits minus withdrawals. `queries/address_value.rs:617–642` builds the address SP timeline the same way, and its stored StabilityAction variants are Deposit/Withdraw/ClaimReturns (`storage/events.rs:63–67`). These projections do not include per-owner interest or liquidation balance changes. They are not an authentic November1 opening balance for this calculation. No analytics query, simulation or product change was performed; this source result is not a claim about deployed byte identity or every possible external archive.
