# Phase 0: dfx-to-icp-cli Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate the rumi-protocol-v2 deployment workflow from dfx to icp-cli, preserving all production canister IDs, identities, and state. Set up `mainnet-live` and `mainnet-staging` environments in `icp.yaml` as the foundation for Phase 1 multi-chain work.

**Architecture:** Same wasm artifacts, same canister IDs, same state. Only the CLI driving deploys changes. icp-cli's native multi-environment support (`icp deploy --environment X`) replaces dfx's `--network ic` plus per-canister bash scripts. Identity store migrates via PEM export/import. Frontend declarations regenerate via a `didc bind --target ts` script.

**Tech Stack:**
- icp-cli 0.2+
- didc (Candid TypeScript bindings)
- dfx (kept installed as fallback for 2 weeks post-migration)
- Existing project: Rust workspace, SvelteKit frontend

**Reference docs:**
- Spec: `docs/superpowers/specs/2026-05-27-multi-chain-rumi-design.md` (Section "Phase 0: icp-cli migration")
- icp-cli docs: https://cli.internetcomputer.org/0.2/
- Migration guide: https://cli.internetcomputer.org/0.2/migration/from-dfx/

**Branch:** Work on `feat/icp-cli-migration` (create from main when starting).

---

## Pre-flight Spikes

Before authoring `icp.yaml` for real, two ~30-minute investigations to confirm icp-cli covers the project's edge cases. If either fails, escalate before continuing.

### Task 1: Spike — Upgrade-arg Expressiveness

**Files:**
- Create: `/tmp/icp-cli-spike-upgrade-args/icp.yaml`
- Create: `/tmp/icp-cli-spike-upgrade-args/src/test_canister/src/lib.rs`
- Create: `/tmp/icp-cli-spike-upgrade-args/src/test_canister/test_canister.did`
- Create: `docs/superpowers/notes/2026-05-27-icp-cli-upgrade-args-spike.md`

The canisters that need verification (real-world patterns from `MEMORY.md`):
- `rumi_protocol_backend`: requires `(variant { Upgrade = record { mode = null; description = opt "..." } })` on every upgrade
- `rumi_3pool`, `liquidation_bot`: require dummy `InitArgs` on upgrade (post_upgrade ignores them but Candid decode requires the shape)
- `rumi_stability_pool`: requires full `InitArgs` on upgrade

- [ ] **Step 1: Create a throwaway test project**

```bash
mkdir -p /tmp/icp-cli-spike-upgrade-args
cd /tmp/icp-cli-spike-upgrade-args
icp new test_project --no-git
cd test_project
```

Expected: `icp new` scaffolds a minimal project.

- [ ] **Step 2: Add a test canister with an upgrade variant**

Replace `src/test_canister/src/lib.rs` with:

```rust
use candid::CandidType;
use serde::Deserialize;

#[derive(CandidType, Deserialize)]
pub enum Mode {
    Reinstall,
}

#[derive(CandidType, Deserialize)]
pub struct UpgradeArgs {
    pub mode: Option<Mode>,
    pub description: Option<String>,
}

#[derive(CandidType, Deserialize)]
pub enum InitOrUpgrade {
    Init(()),
    Upgrade(UpgradeArgs),
}

#[ic_cdk::init]
fn init(_: InitOrUpgrade) {}

#[ic_cdk::post_upgrade]
fn post_upgrade(arg: InitOrUpgrade) {
    if let InitOrUpgrade::Upgrade(args) = arg {
        if let Some(desc) = args.description {
            ic_cdk::println!("Upgrade description: {}", desc);
        }
    }
}
```

Replace `src/test_canister/test_canister.did` with:

```candid
type Mode = variant { Reinstall };
type UpgradeArgs = record {
    mode : opt Mode;
    description : opt text;
};
type InitOrUpgrade = variant {
    Init : null;
    Upgrade : UpgradeArgs;
};

service : (InitOrUpgrade) -> {}
```

- [ ] **Step 3: Try expressing the upgrade arg in icp.yaml**

Write `icp.yaml`:

```yaml
canisters:
  - name: test_canister
    recipe:
      type: "@dfinity/rust"
      package: test_canister

environments:
  - name: local
    network: local
    canisters: [test_canister]
    settings:
      test_canister:
        init_args:
          type: candid
          value: '(variant { Init = null })'
```

- [ ] **Step 4: Deploy and verify init-arg works**

```bash
icp network start -d
icp deploy --environment local
icp canister status test_canister --environment local
```

Expected: deploy succeeds, canister status returns Running.

- [ ] **Step 5: Try upgrading with the upgrade-arg shape**

Look in icp-cli docs for: per-deploy argument override (`--argument` flag or environment setting). If not at deploy time, check whether `init_args` in icp.yaml accepts a different value for "upgrade." If neither, document the workaround (likely: `icp canister install --mode upgrade --argument '...'` directly).

Try one or both of these:

```bash
# Option A: --argument flag at deploy time
icp deploy test_canister --environment local --argument '(variant { Upgrade = record { mode = null; description = opt "test upgrade" } })'

# Option B: canister install with explicit mode
icp canister install test_canister --mode upgrade --argument '(variant { Upgrade = record { mode = null; description = opt "test upgrade" } })'
```

Expected: at least one path works without changing `icp.yaml` between deploys.

- [ ] **Step 6: Document findings**

Create `docs/superpowers/notes/2026-05-27-icp-cli-upgrade-args-spike.md`:

```markdown
# Spike: icp-cli Upgrade-arg Expressiveness

**Date:** 2026-05-27
**Question:** Can icp.yaml + icp-cli handle Rumi's required upgrade-arg patterns?

## Patterns tested

### Pattern A: backend Upgrade variant
Required: `(variant { Upgrade = record { mode = null; description = opt "..." } })`

[Document: did it work? Via which command? Any workarounds needed?]

### Pattern B: dummy InitArgs on upgrade
Required by 3pool, liquidation_bot, stability_pool: the same InitArgs struct must
be passed on every upgrade even though post_upgrade ignores most fields.

[Document the result.]

## Verdict
[GO / NO-GO. If NO-GO, what would be needed?]

## Recommended icp.yaml pattern for each canister
[Concrete config snippets.]
```

- [ ] **Step 7: Commit findings**

```bash
cd /Users/robertripley/coding/rumi-protocol-v2
git add -f docs/superpowers/notes/2026-05-27-icp-cli-upgrade-args-spike.md
git commit -m "docs: icp-cli upgrade-args spike findings"
```

**Decision gate:** If verdict is NO-GO, stop here and discuss with user. Migration cannot proceed without a workable upgrade-arg pattern.

---

### Task 2: Spike — External Canister Equivalent (deps pull)

**Files:**
- Modify: `/tmp/icp-cli-spike-upgrade-args/icp.yaml`
- Create: `docs/superpowers/notes/2026-05-27-icp-cli-deps-pull-spike.md`

dfx's `dfx deps pull` fetches `icp_ledger_canister`, `internet_identity`, `xrc` from mainnet for local replica use. Verify icp-cli has an equivalent and that XRC's system-subnet requirement is preserved.

- [ ] **Step 1: Read the icp.yaml schema for external/dependency canisters**

WebFetch `https://cli.internetcomputer.org/0.2/reference/configuration/` and look for: how to declare external canisters, mainnet pulls, or remote canister IDs.

- [ ] **Step 2: Try declaring the three external canisters in test icp.yaml**

Append to `/tmp/icp-cli-spike-upgrade-args/icp.yaml`:

```yaml
# Append the external/dependency canister section per what step 1 documented.
# Likely candidates (verify against schema):
#   - "external: true" flag on canister entries
#   - separate "pulled" or "remote_canisters" top-level key
#   - a recipe that fetches from mainnet
```

The three to declare:
- `icp_ledger_canister`: `ryjl3-tyaaa-aaaaa-aaaba-cai`
- `internet_identity`: `rdmx6-jaaaa-aaaaa-aaadq-cai`
- `xrc`: `uf6dk-hyaaa-aaaaq-qaaaq-cai`

- [ ] **Step 3: Start local network and verify pulls succeed**

```bash
icp network start -d
icp deploy --environment local
icp canister status icp_ledger_canister --environment local
icp canister status internet_identity --environment local
icp canister status xrc --environment local
```

Expected: all three return Running. If XRC requires a system subnet locally, verify the local network was started with system-subnet mode (per dfx convention, this is `dfx start --subnet-type system`).

- [ ] **Step 4: Document the local-network system-subnet workflow for XRC**

If icp-cli's local network does NOT support system-subnet mode out of the box, document the workaround (custom network config, fallback to dfx for local XRC testing, etc.).

- [ ] **Step 5: Document findings**

Create `docs/superpowers/notes/2026-05-27-icp-cli-deps-pull-spike.md`:

```markdown
# Spike: icp-cli External Canister (deps pull) Equivalent

**Date:** 2026-05-27
**Question:** Can icp-cli replace `dfx deps pull` for icp_ledger_canister, internet_identity, xrc? Does XRC work on the local replica?

## Pull mechanism
[Document the exact icp.yaml syntax and CLI commands]

## XRC system-subnet local-replica behavior
[Document: did XRC respond correctly on local? Did we need any special network mode?]

## Verdict
[GO / NO-GO]

## Recommended icp.yaml pattern for external canisters
[Concrete config snippet.]
```

- [ ] **Step 6: Commit findings**

```bash
cd /Users/robertripley/coding/rumi-protocol-v2
git add -f docs/superpowers/notes/2026-05-27-icp-cli-deps-pull-spike.md
git commit -m "docs: icp-cli deps-pull spike findings"
```

**Decision gate:** If verdict is NO-GO, stop here and discuss with user.

---

## Setup Tasks

### Task 3: Install icp-cli and Verify Coexistence with dfx

**Files:**
- None modified. Tool installation only.

- [ ] **Step 1: Install icp-cli**

```bash
# Verify the install command from https://cli.internetcomputer.org/0.2/guides/installation/
# Likely one of:
curl -fsSL https://internetcomputer.org/install-icp-cli | bash
# OR
brew install dfinity/tap/icp-cli
```

- [ ] **Step 2: Verify both tools work side-by-side**

```bash
icp --version
dfx --version
which icp
which dfx
```

Expected: both return a version. icp-cli is on PATH. (If not on PATH, set up an alias in `~/.zshrc` similar to the existing `DFX` variable pattern from MEMORY.md.)

- [ ] **Step 3: Test that running both simultaneously does not conflict on port 8000**

```bash
dfx start --background --clean
icp network start -d --port 8001  # If port flag exists; else verify icp.yaml override
dfx stop
icp network stop
```

Expected: both can run simultaneously without port conflict. (If conflict, document the icp.yaml port override pattern for later.)

- [ ] **Step 4: Commit any tooling notes**

If you added an alias or PATH adjustment, document it:

```bash
echo "# icp-cli location
export ICP=\"\$(which icp)\"  # or absolute path if not on PATH" >> ~/.zshrc.local
# (only if such a file exists; otherwise just note for the user)
```

No commit needed unless you modified project files.

---

### Task 4: Migrate the rumi_identity Identity

**Files:**
- None modified in the repo. Identity store migration only.

- [ ] **Step 1: Verify rumi_identity exists in dfx**

```bash
DFX="$HOME/Library/Application Support/org.dfinity.dfx/bin/dfx"
"$DFX" identity list | grep rumi_identity
"$DFX" identity get-principal --identity rumi_identity
```

Expected: identity appears in list, principal is returned. Note the principal for later verification.

- [ ] **Step 2: Export the rumi_identity PEM**

```bash
"$DFX" identity export rumi_identity > /tmp/rumi_identity.pem
ls -la /tmp/rumi_identity.pem
```

Expected: PEM file written. Size should be a few hundred bytes.

- [ ] **Step 3: Import into icp-cli**

```bash
icp identity import rumi_identity --from-pem /tmp/rumi_identity.pem
icp identity list | grep rumi_identity
```

Expected: identity now appears in icp-cli's list.

- [ ] **Step 4: Verify principal matches**

```bash
ICP_PRINCIPAL=$(icp identity principal --identity rumi_identity)
DFX_PRINCIPAL=$("$DFX" identity get-principal --identity rumi_identity)
[ "$ICP_PRINCIPAL" = "$DFX_PRINCIPAL" ] && echo "MATCH" || echo "MISMATCH"
```

Expected: `MATCH`. If `MISMATCH`, halt and investigate (likely indicates PEM export problem).

- [ ] **Step 5: Set as default in icp-cli**

```bash
icp identity default rumi_identity
icp identity whoami  # or whatever the equivalent verb is
```

Expected: `rumi_identity` is the active default.

- [ ] **Step 6: Securely delete the temporary PEM**

```bash
shred -u /tmp/rumi_identity.pem 2>/dev/null || rm -P /tmp/rumi_identity.pem
ls /tmp/rumi_identity.pem 2>&1 | grep "No such file"
```

Expected: file gone. (Identity is now in icp-cli's keystore; no need to keep the export.)

No commit needed.

---

### Task 5: Author Baseline icp.yaml Structure

**Files:**
- Create: `icp.yaml` (project root)

This task writes the SKELETON of icp.yaml. Each canister gets added in its own migration task later. The skeleton has the two environments and the external-canister section, applying the patterns confirmed in spikes 1 and 2.

- [ ] **Step 1: Read existing dfx.json for reference**

```bash
cd /Users/robertripley/coding/rumi-protocol-v2
cat dfx.json | head -60
```

Note the canister entries, their types, the networks block, and the external/pulled canister section.

- [ ] **Step 2: Write the baseline icp.yaml**

Create `/Users/robertripley/coding/rumi-protocol-v2/icp.yaml`:

```yaml
# icp.yaml - Internet Computer build & deploy configuration
# Replaces dfx.json. See https://cli.internetcomputer.org/0.2/ for full schema.
#
# Phase 0 migration in progress. Canisters are added one at a time.
# See docs/superpowers/plans/2026-05-27-icp-cli-migration-phase-0.md.

canisters: []

# External/dependency canisters (pulled from mainnet for local development).
# Pattern confirmed in docs/superpowers/notes/2026-05-27-icp-cli-deps-pull-spike.md.
# REPLACE THIS BLOCK with the syntax confirmed by Task 2's spike.
# Likely shape:
#
# external_canisters:
#   - name: icp_ledger_canister
#     canister_id: ryjl3-tyaaa-aaaaa-aaaba-cai
#   - name: internet_identity
#     canister_id: rdmx6-jaaaa-aaaaa-aaadq-cai
#   - name: xrc
#     canister_id: uf6dk-hyaaa-aaaaq-qaaaq-cai

environments:
  - name: mainnet-live
    network: ic
    # canisters: filled in as each canister migrates
    # settings: per-canister overrides authored in each migration task

  - name: mainnet-staging
    network: ic
    # Empty until Phase 1 creates the staging canister IDs.
    # Phase 0 does not deploy to staging.

  - name: local
    network: local
    # canisters: filled in alongside mainnet-live as each canister migrates
```

- [ ] **Step 3: Replace external_canisters block with the spike-confirmed syntax**

Use the exact syntax from `docs/superpowers/notes/2026-05-27-icp-cli-deps-pull-spike.md`. The above is a placeholder; the spike output is canonical.

- [ ] **Step 4: Validate the yaml parses**

```bash
icp config validate
# OR if no validate command:
icp canister list --environment mainnet-live 2>&1 | head -5
```

Expected: no parse errors. (Empty canister list is fine; we have not added any yet.)

- [ ] **Step 5: Commit**

```bash
git checkout -b feat/icp-cli-migration
git add icp.yaml
git commit -m "feat: add baseline icp.yaml with mainnet-live + mainnet-staging environments"
```

---

### Task 6: Replace `dfx generate` with a didc-bind Declarations Script

**Files:**
- Create: `scripts/regenerate-declarations.sh`
- Modify: `package.json` (top-level, add npm script)

`dfx generate` is removed in icp-cli. The frontend depends on `declarations/<canister>/<canister>.did.js` (and `.did.d.ts`) for every canister. We replace it with a `didc bind --target ts` script that mirrors the existing output structure.

- [ ] **Step 1: Verify didc is installed**

```bash
didc --version
```

Expected: version string. If not installed:

```bash
brew install dfinity/tap/didc
# OR build from source per https://github.com/dfinity/candid
```

- [ ] **Step 2: Inspect the existing declarations format**

```bash
ls declarations/
ls declarations/rumi_protocol_backend/
head -20 declarations/rumi_protocol_backend/rumi_protocol_backend.did.js
head -20 declarations/rumi_protocol_backend/rumi_protocol_backend.did.d.ts
```

Note: dfx generates `index.js`, `index.d.ts`, `<canister>.did.js`, `<canister>.did.d.ts`. We need to mirror that.

- [ ] **Step 3: Write the regeneration script**

Create `scripts/regenerate-declarations.sh`:

```bash
#!/bin/bash
# scripts/regenerate-declarations.sh
# Replaces `dfx generate`. Regenerates declarations/ from each canister's .did file
# using `didc bind --target ts/js`.
#
# Usage: ./scripts/regenerate-declarations.sh [canister_name]
# If canister_name omitted, regenerates all canisters listed below.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Canister name -> .did file path
declare -A CANISTERS
CANISTERS[rumi_protocol_backend]="src/rumi_protocol_backend/rumi_protocol_backend.did"
CANISTERS[icusd_ledger]="src/icusd_ledger/icusd_ledger.did"
CANISTERS[icusd_index]="src/icusd_index/icusd_index.did"
CANISTERS[rumi_stability_pool]="src/rumi_stability_pool/rumi_stability_pool.did"
CANISTERS[rumi_treasury]="src/rumi_treasury/rumi_treasury.did"
CANISTERS[rumi_3pool]="src/rumi_3pool/rumi_3pool.did"
CANISTERS[threeusd_index]="src/threeusd_index/threeusd_index.did"
CANISTERS[rumi_amm]="src/rumi_amm/rumi_amm.did"
CANISTERS[liquidation_bot]="src/liquidation_bot/liquidation_bot.did"
CANISTERS[rumi_analytics]="src/rumi_analytics/rumi_analytics.did"
CANISTERS[flaky_ledger]="src/flaky_ledger/flaky_ledger.did"

generate_one() {
    local name="$1"
    local did_path="${CANISTERS[$name]}"
    if [ -z "$did_path" ]; then
        echo "ERROR: unknown canister '$name'"
        return 1
    fi
    if [ ! -f "$did_path" ]; then
        echo "ERROR: .did file not found: $did_path"
        return 1
    fi
    local out_dir="declarations/$name"
    mkdir -p "$out_dir"
    echo "Generating $name from $did_path -> $out_dir/"
    didc bind --target ts "$did_path" > "$out_dir/$name.did.d.ts"
    didc bind --target js "$did_path" > "$out_dir/$name.did.js"

    # Generate index.js + index.d.ts barrel files matching dfx convention.
    cat > "$out_dir/index.js" <<EOF
export { idlFactory } from './$name.did.js';
export * from './$name.did.js';
EOF
    cat > "$out_dir/index.d.ts" <<EOF
export { idlFactory } from './$name.did';
export type * from './$name.did';
EOF
}

if [ $# -eq 0 ]; then
    for name in "${!CANISTERS[@]}"; do
        generate_one "$name"
    done
else
    generate_one "$1"
fi

echo "Done."
```

- [ ] **Step 4: Make it executable**

```bash
chmod +x scripts/regenerate-declarations.sh
```

- [ ] **Step 5: Run it and verify output matches the existing format**

```bash
./scripts/regenerate-declarations.sh rumi_protocol_backend
ls declarations/rumi_protocol_backend/
diff <(head -20 declarations/rumi_protocol_backend/rumi_protocol_backend.did.js) /tmp/old_did_js_head.txt 2>/dev/null || echo "Compare output manually if needed"
```

Expected: 4 files in `declarations/rumi_protocol_backend/` (`.did.js`, `.did.d.ts`, `index.js`, `index.d.ts`). Format approximately matches what dfx generated (some byte-level diffs are OK as long as the exported symbols match).

- [ ] **Step 6: Add npm script to package.json**

Read `package.json` first to find the right scripts block, then add:

```json
"regenerate-declarations": "bash scripts/regenerate-declarations.sh"
```

inside the `"scripts"` object. Don't remove or modify other entries.

- [ ] **Step 7: Test the npm script**

```bash
npm run regenerate-declarations
```

Expected: regenerates all canisters' declarations without error.

- [ ] **Step 8: Verify frontend still imports correctly**

```bash
cd vault_frontend
npm run build 2>&1 | tail -20
```

Expected: build succeeds. (If TypeScript errors appear citing missing exports from `declarations/`, the bindings don't match what dfx produced and the script needs adjustment.)

- [ ] **Step 9: Commit**

```bash
cd /Users/robertripley/coding/rumi-protocol-v2
git add scripts/regenerate-declarations.sh package.json
git commit -m "feat: replace 'dfx generate' with didc-bind declarations script"
```

---

### Task 7: Update the Pre-deploy Test Hook

**Files:**
- Modify: `.claude/hooks/pre-deploy-test.sh`

The hook currently fires on `dfx deploy`. Extend it to also fire on `icp deploy`. Do not break the existing `dfx deploy` detection (we'll be running both during migration).

- [ ] **Step 1: Read the existing hook**

```bash
cat .claude/hooks/pre-deploy-test.sh
```

Note: how does it detect a deploy command? Probably via the Bash hook input JSON, matching the command string.

- [ ] **Step 2: Identify the detection logic**

The hook is a PreToolUse hook on Bash. It receives stdin JSON like `{"tool_name":"Bash","tool_input":{"command":"dfx deploy ..."}}`. It probably uses `jq -r '.tool_input.command'` then `grep -q "dfx deploy"`.

- [ ] **Step 3: Extend the detection regex to also match `icp deploy`**

Edit `.claude/hooks/pre-deploy-test.sh`. Find the line that matches `dfx deploy` and change to:

```bash
# Old:
# if echo "$cmd" | grep -qE 'dfx deploy'; then

# New:
if echo "$cmd" | grep -qE '(dfx|icp) deploy'; then
```

(Adapt the actual change to match the exact existing code; the principle is "match dfx OR icp deploy.")

- [ ] **Step 4: Test the hook against an `icp deploy` invocation**

```bash
echo '{"tool_name":"Bash","tool_input":{"command":"icp deploy --environment mainnet-live"}}' | bash .claude/hooks/pre-deploy-test.sh
echo "Exit: $?"
```

Expected: hook runs (you'll see tests start, since there are unchanged files). Exit code 0 if tests pass, non-zero if they fail.

- [ ] **Step 5: Test it still works for `dfx deploy`**

```bash
echo '{"tool_name":"Bash","tool_input":{"command":"dfx deploy --network ic"}}' | bash .claude/hooks/pre-deploy-test.sh
echo "Exit: $?"
```

Expected: same behavior. The hook does not break for dfx deploys.

- [ ] **Step 6: Test it does NOT fire for unrelated commands**

```bash
echo '{"tool_name":"Bash","tool_input":{"command":"ls -la"}}' | bash .claude/hooks/pre-deploy-test.sh
echo "Exit: $?"
```

Expected: hook exits 0 immediately, no test run.

- [ ] **Step 7: Commit**

```bash
git add .claude/hooks/pre-deploy-test.sh
git commit -m "chore: pre-deploy hook detects 'icp deploy' in addition to 'dfx deploy'"
```

---

## Canister Migration Tasks

Each canister gets:
1. Its entry added to `icp.yaml`
2. An `icp build` invocation verifying the wasm builds
3. A wasm-hash comparison against the current production hash (to confirm no-op)
4. An `icp deploy --environment mainnet-live` upgrade
5. Post-deploy verification (wasm hash on chain matches expected)
6. Functional smoke test (a few sanity queries)

Order is risk-ascending: `flaky_ledger → icusd_index → liquidation_bot → rumi_3pool → rumi_stability_pool → rumi_treasury → rumi_amm → rumi_analytics → rumi_protocol_backend`.

### Task 8: Migrate flaky_ledger

**Files:**
- Modify: `icp.yaml` (add flaky_ledger entry)

`flaky_ledger` is a test-only mock ledger for simulating ledger failures. Lowest risk because it has no production responsibility.

- [ ] **Step 1: Read flaky_ledger's current dfx.json entry**

```bash
jq '.canisters.flaky_ledger' dfx.json
```

Note the `type`, `package`, `candid`, `wasm`, `dependencies` (if any).

- [ ] **Step 2: Translate to icp.yaml**

Read the current `icp.yaml` (Task 5) and add to the `canisters: []` list:

```yaml
canisters:
  - name: flaky_ledger
    recipe:
      type: "@dfinity/rust"
      package: flaky_ledger
    candid: src/flaky_ledger/flaky_ledger.did
```

Then add `flaky_ledger` to the `canisters: []` array in the `mainnet-live` and `local` environment blocks.

- [ ] **Step 3: Build with icp-cli**

```bash
icp build flaky_ledger
```

Expected: build succeeds. Produces `target/wasm32-unknown-unknown/release/flaky_ledger.wasm` (or wherever icp-cli puts it, verify path).

- [ ] **Step 4: Compare the wasm hash with the currently-deployed production hash**

```bash
# icp-cli build output:
ICP_HASH=$(sha256sum target/wasm32-unknown-unknown/release/flaky_ledger.wasm | awk '{print $1}')

# Currently deployed on mainnet:
PROD_HASH=$(dfx canister info flaky_ledger --network ic 2>/dev/null | grep -i "Module hash" | awk '{print $NF}' | tr -d '0x')

echo "icp-cli build: $ICP_HASH"
echo "Production:    $PROD_HASH"
[ "$ICP_HASH" = "$PROD_HASH" ] && echo "MATCH (no-op upgrade)" || echo "MISMATCH (real upgrade)"
```

Expected: `MATCH` if no code changes, `MISMATCH` if there were code changes since last deploy. If MISMATCH and we did not intend changes, halt and investigate.

If `flaky_ledger` is not currently deployed to mainnet (it's test-only and may not be on mainnet), skip this step. Note that in your commit message.

- [ ] **Step 5: Skip deploy if flaky_ledger is not on mainnet, otherwise deploy**

```bash
# If not on mainnet, the prod_hash will be empty; skip deploy.
if [ -z "$PROD_HASH" ]; then
  echo "flaky_ledger not deployed to mainnet; skipping mainnet upgrade. icp-cli build verified."
else
  icp deploy flaky_ledger --environment mainnet-live --identity rumi_identity
fi
```

Expected: either skip message (flaky_ledger isn't on mainnet) or successful deploy.

- [ ] **Step 6: Smoke test (if deployed)**

If deploy occurred, run a basic query:

```bash
icp canister call flaky_ledger icrc1_name --environment mainnet-live
```

Expected: returns the ledger name string.

- [ ] **Step 7: Commit**

```bash
git add icp.yaml
git commit -m "feat(icp-cli): migrate flaky_ledger to icp.yaml"
```

---

### Task 9: Migrate icusd_index

**Files:**
- Modify: `icp.yaml` (add icusd_index entry)

`icusd_index` (`6niqu-siaaa-aaaap-qrjeq-cai`) is the index canister for icUSD. Simple pulled canister or local Rust build (verify which).

- [ ] **Step 1: Read icusd_index's current dfx.json entry**

```bash
jq '.canisters.icusd_index' dfx.json
```

- [ ] **Step 2: Translate to icp.yaml**

If it's a pulled canister (using ICRC index canister wasm), use external_canister syntax. If it's locally built, use the recipe pattern. Add to `canisters:` list and the `mainnet-live` + `local` environments.

- [ ] **Step 3: Build (if locally built)**

```bash
icp build icusd_index
```

Skip if pulled.

- [ ] **Step 4: Hash compare against mainnet**

```bash
PROD_HASH=$(dfx canister info icusd_index --network ic | grep -i "Module hash" | awk '{print $NF}' | tr -d '0x')
echo "Production: $PROD_HASH"
# For pulled canisters, just verify it's deployed.
```

- [ ] **Step 5: Deploy via icp-cli**

```bash
icp deploy icusd_index --environment mainnet-live --identity rumi_identity
```

- [ ] **Step 6: Smoke test**

```bash
icp canister call icusd_index status --environment mainnet-live
```

Expected: returns status struct.

- [ ] **Step 7: Commit**

```bash
git add icp.yaml
git commit -m "feat(icp-cli): migrate icusd_index to icp.yaml"
```

---

### Task 10: Migrate liquidation_bot (First Quirky Canister)

**Files:**
- Modify: `icp.yaml`

`liquidation_bot` (`nygob-3qaaa-aaaap-qttcq-cai`) is the first canister requiring the dummy-`InitArgs`-on-upgrade pattern. Per MEMORY.md `feedback_liquidation_bot_deploy.md`.

- [ ] **Step 1: Look up the current dummy-init-args used by dfx**

Check the project's deploy scripts (likely under `scripts/`, or check recent commits' commit messages):

```bash
grep -r "BotInitArgs" scripts/ 2>/dev/null
grep -rE "liquidation_bot.*--argument" .claude/ scripts/ docs/ 2>/dev/null | head
```

Note the exact dummy-init-args string that's been used for past upgrades.

- [ ] **Step 2: Add to icp.yaml with the spike-confirmed upgrade-arg pattern**

Add to `canisters:`:

```yaml
  - name: liquidation_bot
    recipe:
      type: "@dfinity/rust"
      package: liquidation_bot
    candid: src/liquidation_bot/liquidation_bot.did
```

In the `mainnet-live` environment block, add a `settings` override for liquidation_bot with the dummy InitArgs (use the EXACT pattern confirmed in Task 1's spike findings document):

```yaml
environments:
  - name: mainnet-live
    network: ic
    canisters: [flaky_ledger, icusd_index, liquidation_bot]
    settings:
      liquidation_bot:
        upgrade_args:
          type: candid
          value: '(record { /* fields from BotInitArgs, with default values; copy from past deploys */ })'
```

If icp.yaml does NOT support `upgrade_args` (per the spike), use the alternative pattern (likely a deploy-time `--argument` flag — document the actual command).

- [ ] **Step 3: Build and hash-compare**

```bash
icp build liquidation_bot
ICP_HASH=$(sha256sum target/wasm32-unknown-unknown/release/liquidation_bot.wasm | awk '{print $1}')
PROD_HASH=$(dfx canister info liquidation_bot --network ic | grep -i "Module hash" | awk '{print $NF}' | tr -d '0x')
echo "icp-cli: $ICP_HASH"
echo "Prod:    $PROD_HASH"
```

Expected: hashes should match if no code changes have been made.

- [ ] **Step 4: Deploy via icp-cli (use the upgrade-args pattern from spike)**

If icp.yaml-driven:

```bash
icp deploy liquidation_bot --environment mainnet-live --identity rumi_identity
```

If deploy-time arg required:

```bash
icp deploy liquidation_bot --environment mainnet-live --identity rumi_identity \
  --argument '(variant { Upgrade = record { /* dummy fields */ } })'
```

- [ ] **Step 5: Verify post-deploy module hash matches**

```bash
dfx canister info liquidation_bot --network ic | grep "Module hash"
```

Expected: same hash as before, or new hash matching `ICP_HASH` from step 3.

- [ ] **Step 6: Smoke test**

```bash
icp canister call liquidation_bot get_status --environment mainnet-live
```

Expected: returns bot status without errors.

- [ ] **Step 7: Commit**

```bash
git add icp.yaml
git commit -m "feat(icp-cli): migrate liquidation_bot with dummy-init-args upgrade pattern"
```

---

### Task 11: Migrate rumi_3pool

**Files:**
- Modify: `icp.yaml`

`rumi_3pool` (`fohh4-yyaaa-aaaap-qtkpa-cai`) requires `ThreePoolInitArgs` on upgrade (per MEMORY.md). Same pattern as liquidation_bot. **Extra care:** this is the AMM, and the 2026-05-18 AMM state-wipe incident is the cautionary tale. Verify wasm hash matches production before deploying.

- [ ] **Step 1: Look up the dummy-init-args used for 3pool**

```bash
grep -r "ThreePoolInitArgs" scripts/ src/rumi_3pool/ 2>/dev/null | head
```

Note the exact field defaults used for upgrade calls.

- [ ] **Step 2: Add to icp.yaml**

Pattern identical to liquidation_bot but with `ThreePoolInitArgs`:

```yaml
  - name: rumi_3pool
    recipe:
      type: "@dfinity/rust"
      package: rumi_3pool
    candid: src/rumi_3pool/rumi_3pool.did
```

Add 3pool's upgrade_args to the mainnet-live environment settings.

- [ ] **Step 3: Build and hash-compare**

```bash
icp build rumi_3pool
ICP_HASH=$(sha256sum target/wasm32-unknown-unknown/release/rumi_3pool.wasm | awk '{print $1}')
PROD_HASH=$(dfx canister info rumi_3pool --network ic | grep -i "Module hash" | awk '{print $NF}' | tr -d '0x')
echo "icp-cli: $ICP_HASH"
echo "Prod:    $PROD_HASH"
[ "$ICP_HASH" = "$PROD_HASH" ] && echo "OK to no-op upgrade" || echo "WARNING: hashes differ. DO NOT proceed without understanding the diff."
```

**Decision gate:** If hashes differ and you didn't intend code changes, STOP. Investigate before deploying. The AMM state-wipe history makes silent code drift on rumi_3pool a major concern.

- [ ] **Step 4: Deploy via icp-cli**

```bash
icp deploy rumi_3pool --environment mainnet-live --identity rumi_identity
```

- [ ] **Step 5: Verify post-deploy module hash and that state survived**

```bash
dfx canister info rumi_3pool --network ic | grep "Module hash"
icp canister call rumi_3pool get_pool_status --environment mainnet-live
```

Expected: hash matches `ICP_HASH`. `get_pool_status` returns the SAME LP supply and reserves as before the upgrade. **This is the critical UPG-002 check.** If reserves or LP supply changed unexpectedly, halt immediately.

- [ ] **Step 6: Commit**

```bash
git add icp.yaml
git commit -m "feat(icp-cli): migrate rumi_3pool with ThreePoolInitArgs upgrade pattern"
```

---

### Task 12: Migrate rumi_stability_pool

**Files:**
- Modify: `icp.yaml`

`rumi_stability_pool` (`tmhzi-dqaaa-aaaap-qrd6q-cai`) requires `--argument` with full InitArgs even on upgrade (per MEMORY.md `feedback_stability_pool_deploy.md`).

- [ ] **Step 1: Look up the InitArgs used for SP upgrades**

```bash
grep -r "stability_pool.*--argument\|StabilityPoolInitArgs" scripts/ .claude/ 2>/dev/null | head
```

- [ ] **Step 2: Add to icp.yaml**

```yaml
  - name: rumi_stability_pool
    recipe:
      type: "@dfinity/rust"
      package: rumi_stability_pool
    candid: src/rumi_stability_pool/rumi_stability_pool.did
```

Add SP's upgrade_args to mainnet-live environment settings using the exact InitArgs shape from Step 1.

- [ ] **Step 3: Build and hash-compare**

```bash
icp build rumi_stability_pool
ICP_HASH=$(sha256sum target/wasm32-unknown-unknown/release/rumi_stability_pool.wasm | awk '{print $1}')
PROD_HASH=$(dfx canister info rumi_stability_pool --network ic | grep -i "Module hash" | awk '{print $NF}' | tr -d '0x')
echo "icp-cli: $ICP_HASH"
echo "Prod:    $PROD_HASH"
```

- [ ] **Step 4: Deploy via icp-cli**

```bash
icp deploy rumi_stability_pool --environment mainnet-live --identity rumi_identity
```

- [ ] **Step 5: Verify SP state survived**

```bash
dfx canister info rumi_stability_pool --network ic | grep "Module hash"
icp canister call rumi_stability_pool get_pool_status --environment mainnet-live
```

Expected: hash matches, pool status shows expected total_deposits and depositor count.

- [ ] **Step 6: Commit**

```bash
git add icp.yaml
git commit -m "feat(icp-cli): migrate rumi_stability_pool with full InitArgs upgrade pattern"
```

---

### Task 13: Migrate rumi_treasury

**Files:**
- Modify: `icp.yaml`

`rumi_treasury` (`tlg74-oiaaa-aaaap-qrd6a-cai`). Simple canister, no known quirks.

- [ ] **Step 1: Read dfx.json entry**

```bash
jq '.canisters.rumi_treasury' dfx.json
```

- [ ] **Step 2: Add to icp.yaml**

```yaml
  - name: rumi_treasury
    recipe:
      type: "@dfinity/rust"
      package: rumi_treasury
    candid: src/rumi_treasury/rumi_treasury.did
```

- [ ] **Step 3: Build and hash-compare**

```bash
icp build rumi_treasury
ICP_HASH=$(sha256sum target/wasm32-unknown-unknown/release/rumi_treasury.wasm | awk '{print $1}')
PROD_HASH=$(dfx canister info rumi_treasury --network ic | grep -i "Module hash" | awk '{print $NF}' | tr -d '0x')
echo "icp-cli: $ICP_HASH"
echo "Prod:    $PROD_HASH"
```

- [ ] **Step 4: Deploy via icp-cli**

```bash
icp deploy rumi_treasury --environment mainnet-live --identity rumi_identity
```

- [ ] **Step 5: Smoke test**

```bash
icp canister call rumi_treasury get_treasury_stats --environment mainnet-live
```

Expected: returns treasury stats matching pre-upgrade state.

- [ ] **Step 6: Commit**

```bash
git add icp.yaml
git commit -m "feat(icp-cli): migrate rumi_treasury to icp.yaml"
```

---

### Task 14: Migrate rumi_amm

**Files:**
- Modify: `icp.yaml`

`rumi_amm` (`ijlzs-2yaaa-aaaap-quaaq-cai`). Per MEMORY.md, AMM1 is live with sensitive LP positions. **Extra care:** wasm hash MUST match production before deploying, given the 2026-05-18 state-wipe history.

- [ ] **Step 1: Read dfx.json entry**

```bash
jq '.canisters.rumi_amm' dfx.json
```

- [ ] **Step 2: Add to icp.yaml**

```yaml
  - name: rumi_amm
    recipe:
      type: "@dfinity/rust"
      package: rumi_amm
    candid: src/rumi_amm/rumi_amm.did
```

- [ ] **Step 3: Build and hash-compare**

```bash
icp build rumi_amm
ICP_HASH=$(sha256sum target/wasm32-unknown-unknown/release/rumi_amm.wasm | awk '{print $1}')
PROD_HASH=$(dfx canister info rumi_amm --network ic | grep -i "Module hash" | awk '{print $NF}' | tr -d '0x')
echo "icp-cli: $ICP_HASH"
echo "Prod:    $PROD_HASH"
[ "$ICP_HASH" = "$PROD_HASH" ] && echo "OK" || echo "STOP: hashes differ"
```

**Decision gate:** If hashes differ, STOP and discuss with user. The AMM state-wipe history makes silent drift catastrophic.

- [ ] **Step 4: Deploy via icp-cli**

```bash
icp deploy rumi_amm --environment mainnet-live --identity rumi_identity
```

- [ ] **Step 5: Verify AMM state**

```bash
dfx canister info rumi_amm --network ic | grep "Module hash"
icp canister call rumi_amm get_pool_status '(record { token_a = principal "fohh4-yyaaa-aaaap-qtkpa-cai"; token_b = principal "ryjl3-tyaaa-aaaaa-aaaba-cai"; fee_bps = 30 })' --environment mainnet-live
```

Expected: hash matches; pool reserves and LP shares match pre-upgrade values exactly.

- [ ] **Step 6: Commit**

```bash
git add icp.yaml
git commit -m "feat(icp-cli): migrate rumi_amm to icp.yaml"
```

---

### Task 15: Migrate rumi_analytics

**Files:**
- Modify: `icp.yaml`

`rumi_analytics` (`dtlu2-uqaaa-aaaap-qugcq-cai`) has two quirks per MEMORY.md:
- Needs full `InitArgs` on upgrade
- Wasm is 2.4MB which exceeds the 2MiB ingress limit; must `ic-wasm shrink` + `gzip` before install

icp-cli needs to support either:
(a) a post-build hook that shrinks/gzips the wasm
(b) an explicit gzipped wasm path in the canister config

- [ ] **Step 1: Look up the current InitArgs and the wasm post-processing pipeline**

```bash
grep -r "rumi_analytics.*--argument\|AnalyticsInitArgs" scripts/ .claude/ 2>/dev/null
grep -r "ic-wasm shrink\|gzip.*analytics" scripts/ .claude/ 2>/dev/null
```

Note the exact InitArgs shape and the shrink+gzip commands.

- [ ] **Step 2: Determine how to express wasm post-processing in icp.yaml**

Options (verify against icp.yaml schema):
- A `build.steps` array that runs `cargo build`, then `ic-wasm shrink`, then `gzip`
- A `recipe: { type: "@dfinity/rust"; post_build: "..." }` if such a key exists
- A custom recipe that wraps the steps

Use whichever pattern icp-cli supports. Test with `icp build rumi_analytics` and verify the output is a gzipped wasm under the 2MiB threshold.

- [ ] **Step 3: Add to icp.yaml**

```yaml
  - name: rumi_analytics
    build:
      steps:
        - type: script
          commands:
            - cargo build --target wasm32-unknown-unknown --release --package rumi_analytics
            - ic-wasm target/wasm32-unknown-unknown/release/rumi_analytics.wasm -o target/wasm32-unknown-unknown/release/rumi_analytics.wasm shrink
            - gzip -f target/wasm32-unknown-unknown/release/rumi_analytics.wasm
    candid: src/rumi_analytics/rumi_analytics.did
    wasm: target/wasm32-unknown-unknown/release/rumi_analytics.wasm.gz
```

Add full InitArgs to the mainnet-live environment settings.

- [ ] **Step 4: Build and verify wasm is under 2MiB after gzip**

```bash
icp build rumi_analytics
ls -lh target/wasm32-unknown-unknown/release/rumi_analytics.wasm.gz
```

Expected: gzipped wasm under 2 MB.

- [ ] **Step 5: Hash-compare**

```bash
ICP_HASH=$(sha256sum target/wasm32-unknown-unknown/release/rumi_analytics.wasm.gz | awk '{print $1}')
PROD_HASH=$(dfx canister info rumi_analytics --network ic | grep -i "Module hash" | awk '{print $NF}' | tr -d '0x')
echo "icp-cli: $ICP_HASH"
echo "Prod:    $PROD_HASH"
```

Note: the production hash is the hash of the GZIPPED wasm on chain; icp-cli should produce a matching hash.

- [ ] **Step 6: Deploy via icp-cli**

```bash
icp deploy rumi_analytics --environment mainnet-live --identity rumi_identity
```

- [ ] **Step 7: Verify state survived**

```bash
dfx canister info rumi_analytics --network ic | grep "Module hash"
icp canister call rumi_analytics get_protocol_summary --environment mainnet-live
```

Expected: hash matches, summary returns the same totals as pre-upgrade.

- [ ] **Step 8: Commit**

```bash
git add icp.yaml
git commit -m "feat(icp-cli): migrate rumi_analytics with shrink+gzip build pipeline"
```

---

### Task 16: Migrate rumi_protocol_backend (Highest Stakes, Last)

**Files:**
- Modify: `icp.yaml`

`rumi_protocol_backend` (`tfesu-vyaaa-aaaap-qrd7a-cai`). The crown jewel. Every upgrade requires the `(variant { Upgrade = record { mode = null; description = opt "..." } })` pattern.

- [ ] **Step 1: Look up the canonical upgrade-arg shape**

The shape is documented in MEMORY.md and prior backend deploys. Verify with:

```bash
grep -rE "rumi_protocol_backend.*Upgrade.*description" scripts/ .claude/ 2>/dev/null | head
```

- [ ] **Step 2: Add to icp.yaml**

```yaml
  - name: rumi_protocol_backend
    recipe:
      type: "@dfinity/rust"
      package: rumi_protocol_backend
    candid: src/rumi_protocol_backend/rumi_protocol_backend.did
```

For mainnet-live, the upgrade_args MUST be provided at deploy time, not in icp.yaml (because every upgrade needs a fresh description). Document the deploy command:

```bash
# Use this template every time you deploy rumi_protocol_backend:
# icp deploy rumi_protocol_backend --environment mainnet-live --identity rumi_identity \
#   --argument '(variant { Upgrade = record { mode = null; description = opt "<short summary of change>" } })'
```

Add a comment in icp.yaml referencing this requirement.

- [ ] **Step 3: Build and hash-compare**

```bash
icp build rumi_protocol_backend
ICP_HASH=$(sha256sum target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm | awk '{print $1}')
PROD_HASH=$(dfx canister info rumi_protocol_backend --network ic | grep -i "Module hash" | awk '{print $NF}' | tr -d '0x')
echo "icp-cli: $ICP_HASH"
echo "Prod:    $PROD_HASH"
[ "$ICP_HASH" = "$PROD_HASH" ] && echo "OK to no-op upgrade" || echo "STOP: hashes differ"
```

**Decision gate:** If hashes differ, STOP. This is the highest-stakes canister. Investigate before deploying.

- [ ] **Step 4: Deploy via icp-cli with the description**

```bash
icp deploy rumi_protocol_backend --environment mainnet-live --identity rumi_identity \
  --argument '(variant { Upgrade = record { mode = null; description = opt "Phase 0 icp-cli migration: deploy via icp-cli, no code change" } })'
```

Expected: deploy succeeds. The pre-deploy test hook fires (Task 7) and tests pass.

- [ ] **Step 5: Verify backend state**

```bash
dfx canister info rumi_protocol_backend --network ic | grep "Module hash"
icp canister call rumi_protocol_backend get_protocol_status --environment mainnet-live
icp canister call rumi_protocol_backend get_collateral_totals --environment mainnet-live
```

Expected: hash matches `ICP_HASH`; protocol status returns expected total_debt and vault count; collateral totals match pre-upgrade values.

- [ ] **Step 6: Commit**

```bash
git add icp.yaml
git commit -m "feat(icp-cli): migrate rumi_protocol_backend to icp.yaml (Phase 0 complete)"
```

---

## End-of-Phase Verification

### Task 17: Full Phase 0 Verification

**Files:**
- None modified.

Smoke-test the entire system after the migration. Every canister on mainnet should have been deployed at least once via icp-cli, all state preserved, all queries returning expected values.

- [ ] **Step 1: Verify every canister's module hash on mainnet**

```bash
for canister in flaky_ledger icusd_index liquidation_bot rumi_3pool rumi_stability_pool rumi_treasury rumi_amm rumi_analytics rumi_protocol_backend; do
    echo "=== $canister ==="
    dfx canister info "$canister" --network ic 2>/dev/null | grep -E "Module hash|Controllers"
    echo ""
done
```

Expected: every canister has a Module hash and the expected controllers list.

- [ ] **Step 2: Run the frontend build to verify declarations still work**

```bash
cd vault_frontend
npm run regenerate-declarations
npm run build
```

Expected: build succeeds.

- [ ] **Step 3: Verify the pre-deploy hook still fires for both tools**

```bash
echo '{"tool_name":"Bash","tool_input":{"command":"dfx deploy --network ic"}}' | bash .claude/hooks/pre-deploy-test.sh
echo "dfx exit: $?"
echo '{"tool_name":"Bash","tool_input":{"command":"icp deploy --environment mainnet-live"}}' | bash .claude/hooks/pre-deploy-test.sh
echo "icp exit: $?"
```

Expected: both exit 0 (assuming tests pass).

- [ ] **Step 4: Verify the supply / debt invariant on the live backend**

```bash
icp canister call rumi_protocol_backend get_protocol_status --environment mainnet-live | grep -E "total_debt|total_collateral"
icp canister call icusd_ledger icrc1_total_supply --environment mainnet-live
```

Note both values. They should match (both represent ICP-side icUSD supply pre-multi-chain, since Phase 0 doesn't change accounting).

- [ ] **Step 5: Update MEMORY.md note about dfx vs icp-cli**

Add to `/Users/robertripley/.claude/projects/-Users-robertripley-coding-rumi-protocol-v2/memory/MEMORY.md` under "## Deployment":

```markdown
- **Project deploys via icp-cli as of 2026-05-27.** dfx is still installed as fallback for 2 weeks. Use `icp deploy <canister> --environment mainnet-live --identity rumi_identity` for production upgrades. Backend deploys still require the `(variant { Upgrade = record { mode = null; description = opt "..." } })` argument at deploy time. Deploy commands per canister are in icp.yaml comments.
```

(This is a user-instruction memory file edit; Claude should ask the user to do this manually since it's outside the project repo.)

- [ ] **Step 6: Final commit + PR**

```bash
git push -u origin feat/icp-cli-migration
gh pr create --title "Phase 0: migrate dfx → icp-cli" --body "$(cat <<'EOF'
## Summary

Migrates the project from dfx to icp-cli per `docs/superpowers/specs/2026-05-27-multi-chain-rumi-design.md` Phase 0.

- `icp.yaml` with `mainnet-live` and `mainnet-staging` environments
- All 9 production canisters deployed once via icp-cli (no-op upgrades, state preserved)
- `scripts/regenerate-declarations.sh` replaces `dfx generate`
- `.claude/hooks/pre-deploy-test.sh` detects both `dfx deploy` and `icp deploy`
- Two spike findings docs (upgrade-args, deps-pull) committed for future reference

## Verification

- [ ] Every canister's Module hash unchanged after migration
- [ ] Frontend builds against regenerated declarations
- [ ] Pre-deploy hook fires for both tools
- [ ] Protocol status unchanged: total_debt and ICP-side icUSD supply match pre-migration

## Test plan

- [ ] Verify backend protocol status after each deploy
- [ ] Verify AMM and SP state preserved (no UPG-002 / state-wipe events)
- [ ] Verify rumi_3pool LP supply preserved

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Do NOT merge the PR; user reviews and merges manually.

**Phase 0 is complete when this PR is merged.**

---

## Phase 0 → Phase 1 Handoff

After Phase 0 PR merges:
- icp.yaml is the canonical build config
- dfx remains installed for 2 weeks as fallback
- `mainnet-staging` environment exists but has no canisters yet
- Phase 1 starts: a separate brainstorm + plan + implementation cycle, with its own spec section already in the umbrella design

To start Phase 1, invoke the writing-plans skill with the same spec doc and Phase 1 scope.
