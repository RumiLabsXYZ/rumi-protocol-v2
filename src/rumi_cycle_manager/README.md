# Cycle Manager observability

Rumi canisters expose a narrow self-reporting surface for Cycle Manager. Cycle Manager should discover targets through `rumi_analytics.cycle_manager_targets()` and then call each target directly. It must not be added as a controller and must not depend on management-canister `canister_status` for Rumi monitoring.

## Self-report interface

Every monitored canister exposes:

```did
type CycleManagerCyclesStatus = record {
  balance : nat;
  low_watermark : nat;
  healthy : bool;
  freeze_threshold_secs : nat64;
  stable_memory_bytes : opt nat64;
  heap_memory_bytes : opt nat64;
  idle_burn_cycles_per_day : opt nat;
};

type CycleManagerMetric = record {
  key : text;
  count : nat64;
  value : nat;
  label : opt text;
};

cycles_status : () -> (CycleManagerCyclesStatus) query;
cycle_manager_metrics : () -> (vec CycleManagerMetric) query;
```

`rumi_analytics` also exposes:

```did
type CycleManagerTarget = record {
  canister_id : principal;
  name : text;
  project : text;
  environment : variant { Production; Staging; Test; Local; Archived };
  criticality : variant { Critical; Important; Standard; Experimental };
  kind : variant { SelfReport; Controlled };
  low_threshold_cycles : nat;
  topup_cycles : nat;
  owner : opt text;
  tags : vec text;
  expected_controllers : vec principal;
  expected_freeze_threshold_secs : opt nat64;
  metrics_schema_version : nat32;
};

cycle_manager_targets : () -> (vec CycleManagerTarget) query;
```

All current Rumi targets are `SelfReport`. `expected_controllers` is intentionally empty for this integration because Cycle Manager should not control Rumi canisters.

## Metric keys

Metric keys use `domain:subject:measure` with lower-case ASCII segments. Segments may contain `_` when the subject is a compound name.

- `op:*:count` for operational object counts, queues, and discovery counts.
- `op:*:rejects` for cumulative operational reject/error counters.
- `ledger:*:count` for ledger or supply counters.
- `call:*:count` for pending or failed external call counters.

Use stable keys. Prefer adding a new key over changing the meaning of an existing key.

## Discovery

`rumi_analytics.cycle_manager_targets()` is the canonical source for monitored Rumi canisters. The target list includes analytics itself and the configured backend, 3pool, stability pool, and AMM source principals. On production it also includes the known treasury, liquidation bot, and points canisters.

New Rumi canisters become visible to Cycle Manager by:

1. Adding `cycles_status` and `cycle_manager_metrics` to the canister.
2. Adding the shared `rumi_cycle_manager` crate dependency.
3. Adding the canister to the analytics discovery builder when it is a fixed project canister, or to `SourceCanisterIds` when it is environment-configured.
4. Regenerating Candid and TypeScript declarations.

External ledgers, indexes, frontends, and vault records are not direct self-report targets in this pass. Rumi backend state and aggregate metrics cover vault operations.

## Black-holing and observers

The current observability methods are public query methods and return aggregate operational data only. They do not expose secrets, per-user balances, or controller powers.

Before black-holing a canister or making these methods access-controlled, bake the Cycle Manager observer principal and the analytics canister principal into the allowed observer set. Do this before removing upgrade authority; otherwise Cycle Manager discovery can remain intact while direct target polling is permanently blocked.
