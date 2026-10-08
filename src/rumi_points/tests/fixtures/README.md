# Fiat-stable policy-absent upgrade fixture

`pocket_ic_ingest::baseline_points_upgrade_initializes_fiat_policy_and_migrates_prefix`
needs a populated pre-policy stable layout.  Build it from the exact source
anchor, not from a deployed Wasm:

```sh
mkdir -p /private/tmp/rumi-points-baseline-src
git archive 4b4b1680a89b37ab82641ae1e1cb6addf98dcdb5 | tar -x -C /private/tmp/rumi-points-baseline-src
git -C /private/tmp/rumi-points-baseline-src apply \
  /absolute/path/to/src/rumi_points/tests/fixtures/fiat-stable-policy-absent-4b4b1680.patch
CARGO_TARGET_DIR=/private/tmp/rumi-points-baseline-target \
  cargo build --manifest-path /private/tmp/rumi-points-baseline-src/Cargo.toml \
  --locked --target wasm32-unknown-unknown --release -p rumi_points --bin rumi_points
cp /private/tmp/rumi-points-baseline-target/wasm32-unknown-unknown/release/rumi_points.wasm \
  /private/tmp/rumi-points-baseline-fixture.wasm
candid-extractor /private/tmp/rumi-points-baseline-fixture.wasm | rg 'FiatStable'
```

The final command must produce no output.  The patch adds only a fixture-only
admin endpoint which writes an existing `PrincipalState` and `PointEntry` shape;
it does not allocate the candidate's MemoryId 14 or add any FiatStable type.
The focused test then upgrades that old layout into the candidate Wasm.
