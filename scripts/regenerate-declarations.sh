#!/usr/bin/env bash
#
# Regenerate per-canister Candid bindings under src/declarations/.
#
# Replaces `dfx generate` (removed in icp-cli). For each canister we produce:
#   - <name>.did.js     (idlFactory)         via didc bind --target js
#   - <name>.did.d.ts   (type declarations)  via didc bind --target ts
#   - index.js          (createActor + canisterId env wiring)
#   - index.d.ts        (typed re-exports)
#
# The output format matches what `dfx generate` produced so the existing
# frontend imports keep working without code changes.
#
# Usage:
#   scripts/regenerate-declarations.sh                 # regenerate all
#   scripts/regenerate-declarations.sh <canister_name> # regenerate one
#
# Notes:
#   - The top-level `declarations/` directory is a symlink to `src/declarations/`,
#     so writes to src/declarations are visible at both paths.
#   - Two aliases (icp_ledger, icpswap_pool) have hand-customized index files
#     and we preserve those rather than overwriting. The `.did.js` and
#     `.did.d.ts` bindings are still regenerated.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if ! command -v didc >/dev/null 2>&1; then
  echo "error: didc not found on PATH" >&2
  echo "install: brew install dfinity/tap/didc" >&2
  exit 1
fi

# Parallel arrays: canister name | source .did path (relative to repo root).
# The plan's hard-coded list is verified against src/declarations/.
canister_names=(
  "icp_ledger"
  "icp_ledger_canister"
  "icpswap_pool"
  "icusd_index"
  "icusd_ledger"
  "liquidation_bot"
  "rumi_3pool"
  "rumi_amm"
  "rumi_analytics"
  "rumi_protocol_backend"
  "rumi_stability_pool"
  "rumi_treasury"
  "vault_frontend"
)

# icp_ledger and icp_ledger_canister share the ICRC-1 ledger.did.
# icpswap_pool and vault_frontend keep their .did file alongside the generated
# bindings (no other source-of-truth in the repo).
canister_did_paths=(
  "src/ledger/ledger.did"
  "src/ledger/ledger.did"
  "src/declarations/icpswap_pool/icpswap_pool.did"
  "src/ledger/index-ng.did"
  "src/ledger/ledger.did"
  "src/liquidation_bot/liquidation_bot.did"
  "src/rumi_3pool/rumi_3pool.did"
  "src/rumi_amm/rumi_amm.did"
  "src/rumi_analytics/rumi_analytics.did"
  "src/rumi_protocol_backend/rumi_protocol_backend.did"
  "src/stability_pool/stability_pool.did"
  "src/rumi_treasury/rumi_treasury.did"
  "src/declarations/vault_frontend/vault_frontend.did"
)

# Canisters whose index.js / index.d.ts diverge from the standard dfx template
# and must be left intact (they ship hand-written re-export shims).
preserve_index_canisters=" icp_ledger icpswap_pool "

# uppercase helper: portable replacement for ${var^^} (bash 4+) and case
# conversion incompatible with bash 3.2 on macOS.
to_upper() {
  printf '%s' "$1" | tr '[:lower:]' '[:upper:]'
}

write_did_js() {
  local name="$1" did="$2" out_dir="$3"
  didc bind --target js "$did" > "$out_dir/$name.did.js"
}

write_did_ts() {
  local name="$1" did="$2" out_dir="$3"
  didc bind --target ts "$did" > "$out_dir/$name.did.d.ts"
}

write_index_js() {
  local name="$1" out_dir="$2"
  local upper
  upper="$(to_upper "$name")"
  cat > "$out_dir/index.js" <<EOF
import { Actor, HttpAgent } from "@dfinity/agent";

// Imports and re-exports candid interface
import { idlFactory } from "./$name.did.js";
export { idlFactory } from "./$name.did.js";

/* CANISTER_ID is replaced by webpack based on node environment
 * Note: canister environment variable will be standardized as
 * process.env.CANISTER_ID_<CANISTER_NAME_UPPERCASE>
 * beginning in dfx 0.15.0
 */
export const canisterId =
  process.env.CANISTER_ID_$upper;

export const createActor = (canisterId, options = {}) => {
  const agent = options.agent || new HttpAgent({ ...options.agentOptions });

  if (options.agent && options.agentOptions) {
    console.warn(
      "Detected both agent and agentOptions passed to createActor. Ignoring agentOptions and proceeding with the provided agent."
    );
  }

  // Fetch root key for certificate validation during development
  if (process.env.DFX_NETWORK !== "ic") {
    agent.fetchRootKey().catch((err) => {
      console.warn(
        "Unable to fetch root key. Check to ensure that your local replica is running"
      );
      console.error(err);
    });
  }

  // Creates an actor with using the candid interface and the HttpAgent
  return Actor.createActor(idlFactory, {
    agent,
    canisterId,
    ...options.actorOptions,
  });
};

export const $name = canisterId ? createActor(canisterId) : undefined;
EOF
}

write_index_dts() {
  local name="$1" out_dir="$2"
  cat > "$out_dir/index.d.ts" <<EOF
import type {
  ActorSubclass,
  HttpAgentOptions,
  ActorConfig,
  Agent,
} from "@dfinity/agent";
import type { Principal } from "@dfinity/principal";
import type { IDL } from "@dfinity/candid";

import { _SERVICE } from './$name.did';

export declare const idlFactory: IDL.InterfaceFactory;
export declare const canisterId: string;

export declare interface CreateActorOptions {
  /**
   * @see {@link Agent}
   */
  agent?: Agent;
  /**
   * @see {@link HttpAgentOptions}
   */
  agentOptions?: HttpAgentOptions;
  /**
   * @see {@link ActorConfig}
   */
  actorOptions?: ActorConfig;
}

/**
 * Intializes an {@link ActorSubclass}, configured with the provided SERVICE interface of a canister.
 * @constructs {@link ActorSubClass}
 * @param {string | Principal} canisterId - ID of the canister the {@link Actor} will talk to
 * @param {CreateActorOptions} options - see {@link CreateActorOptions}
 * @param {CreateActorOptions["agent"]} options.agent - a pre-configured agent you'd like to use. Supercedes agentOptions
 * @param {CreateActorOptions["agentOptions"]} options.agentOptions - options to set up a new agent
 * @see {@link HttpAgentOptions}
 * @param {CreateActorOptions["actorOptions"]} options.actorOptions - options for the Actor
 * @see {@link ActorConfig}
 */
export declare const createActor: (
  canisterId: string | Principal,
  options?: CreateActorOptions
) => ActorSubclass<_SERVICE>;

/**
 * Intialized Actor using default settings, ready to talk to a canister using its candid interface
 * @constructs {@link ActorSubClass}
 */
export declare const $name: ActorSubclass<_SERVICE>;
EOF
}

regenerate_one() {
  local name="$1" did="$2"
  local out_dir="src/declarations/$name"

  if [ ! -f "$did" ]; then
    echo "error: $name: did source not found at $did" >&2
    return 1
  fi

  mkdir -p "$out_dir"
  write_did_js "$name" "$did" "$out_dir"
  write_did_ts "$name" "$did" "$out_dir"

  if [[ " $preserve_index_canisters " == *" $name "* ]]; then
    echo "ok: $name (bindings only; index files preserved)"
  else
    write_index_js "$name" "$out_dir"
    write_index_dts "$name" "$out_dir"
    echo "ok: $name"
  fi
}

target="${1:-}"
total=${#canister_names[@]}
matched=0

i=0
while [ "$i" -lt "$total" ]; do
  name="${canister_names[$i]}"
  did="${canister_did_paths[$i]}"
  if [ -z "$target" ] || [ "$target" = "$name" ]; then
    regenerate_one "$name" "$did"
    matched=$((matched + 1))
  fi
  i=$((i + 1))
done

if [ -n "$target" ] && [ "$matched" -eq 0 ]; then
  echo "error: unknown canister '$target'" >&2
  echo "known: ${canister_names[*]}" >&2
  exit 1
fi
