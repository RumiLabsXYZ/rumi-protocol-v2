#!/usr/bin/env bash
# Build the ISOLATED `xrpl-testnet-fixture` backend artifact (public XRPL testnet,
# test_key_1; never mainnet) and, only on explicit request, install it to a
# NON-mainnet target.
#
# Default behaviour: build + verify the testnet marker + print the intended install
# command. Nothing is installed, deployed, signed, funded or broadcast.
#
#   scripts/run-xrpl-icusd-testnet.sh                # build, verify, print command
#   scripts/run-xrpl-icusd-testnet.sh --no-build     # re-verify the existing artifact
#   scripts/run-xrpl-icusd-testnet.sh --execute      # also run the install (see below)
#
# --execute requires ALL of:
#   XRPL_FIXTURE_CANISTER     canister NAME or principal to install onto
#   XRPL_FIXTURE_IDENTITY     icp identity to use
#   XRPL_FIXTURE_ENVIRONMENT  icp environment; allowlisted to NON-mainnet: local
# and refuses any known Rumi mainnet canister id or mainnet-style environment.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARTIFACT_DIR="${XRPL_FIXTURE_ARTIFACT_DIR:-$REPO_ROOT/target/xrpl-fixture-artifact}"
ARTIFACT="$ARTIFACT_DIR/rumi_protocol_backend_xrpl_fixture.wasm.gz"
VERIFY="$REPO_ROOT/scripts/verify-xrpl-icusd-artifact.sh"
ALLOWED_ENVIRONMENTS=("local")

BUILD=1
EXECUTE=0
for arg in "$@"; do
  case "$arg" in
    --no-build) BUILD=0 ;;
    --execute) EXECUTE=1 ;;
    -h|--help) sed -n '2,19p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

fail() { printf 'REFUSED/FAIL: %s\n' "$*" >&2; exit 1; }

if [[ "$BUILD" == "1" ]]; then
  command -v ic-wasm >/dev/null 2>&1 || fail "ic-wasm is required"
  mkdir -p "$ARTIFACT_DIR"
  ( cd "$REPO_ROOT"
    # Same target dir as normal builds: only the backend crate is rebuilt.
    cargo build --release --target wasm32-unknown-unknown --package rumi_protocol_backend --features xrpl-testnet-fixture
    cp target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm "$ARTIFACT_DIR/fixture.wasm"
    ic-wasm "$ARTIFACT_DIR/fixture.wasm" -o "$ARTIFACT_DIR/fixture.wasm" metadata candid:service -f src/rumi_protocol_backend/rumi_protocol_backend.did -v public
    ic-wasm "$ARTIFACT_DIR/fixture.wasm" -o "$ARTIFACT_DIR/fixture.wasm" shrink --keep-name-section
    gzip -9 -n -c "$ARTIFACT_DIR/fixture.wasm" > "$ARTIFACT"
    rm -f "$ARTIFACT_DIR/fixture.wasm" )
fi

[[ -f "$ARTIFACT" ]] || fail "fixture artifact not found: $ARTIFACT (run without --no-build)"
# Requires the testnet marker AND the marker query; a production artifact is refused.
"$VERIFY" --require-testnet-fixture "$ARTIFACT"
# The release gate must reject this same file (defence in depth).
if "$VERIFY" --require-mainnet "$ARTIFACT" >/dev/null 2>&1; then
  fail "release gate unexpectedly accepted the fixture artifact"
fi
printf 'fixture artifact: %s (%s bytes)\n' "$ARTIFACT" "$(wc -c < "$ARTIFACT" | tr -d ' ')"

CANISTER="${XRPL_FIXTURE_CANISTER:-<set XRPL_FIXTURE_CANISTER>}"
IDENTITY="${XRPL_FIXTURE_IDENTITY:-<set XRPL_FIXTURE_IDENTITY>}"
ENVIRONMENT="${XRPL_FIXTURE_ENVIRONMENT:-<set XRPL_FIXTURE_ENVIRONMENT>}"
CMD=(icp canister install "$CANISTER" --mode upgrade --wasm "$ARTIFACT" -e "$ENVIRONMENT" --identity "$IDENTITY")

if [[ "$EXECUTE" != "1" ]]; then
  printf 'Dry run only (nothing installed). Intended command:\n  %s\n' "${CMD[*]}"
  printf 'Add --execute (plus XRPL_FIXTURE_CANISTER / _IDENTITY / _ENVIRONMENT, non-mainnet only) to run it.\n'
  exit 0
fi

# ---- --execute guards --------------------------------------------------------
[[ -n "${XRPL_FIXTURE_CANISTER:-}" && -n "${XRPL_FIXTURE_IDENTITY:-}" && -n "${XRPL_FIXTURE_ENVIRONMENT:-}" ]] \
  || fail "--execute needs XRPL_FIXTURE_CANISTER, XRPL_FIXTURE_IDENTITY and XRPL_FIXTURE_ENVIRONMENT"

env_lc="$(printf '%s' "$ENVIRONMENT" | tr '[:upper:]' '[:lower:]')"
case "$env_lc" in
  ic|mainnet|mainnet-*|*mainnet*|production|prod|*-live) fail "environment '$ENVIRONMENT' is mainnet-style" ;;
esac
allowed=0
for e in "${ALLOWED_ENVIRONMENTS[@]}"; do [[ "$ENVIRONMENT" == "$e" ]] && allowed=1; done
[[ "$allowed" == "1" ]] || fail "environment '$ENVIRONMENT' is not in the non-mainnet allowlist (${ALLOWED_ENVIRONMENTS[*]})"

# Refuse any canister id the repo knows to be live (canister_ids.json) or documented
# as Rumi mainnet infrastructure, whether given as a principal or by a name that maps
# to one.
KNOWN_MAINNET_IDS=(
  tfesu-vyaaa-aaaap-qrd7a-cai tlg74-oiaaa-aaaap-qrd6a-cai tmhzi-dqaaa-aaaap-qrd6q-cai
  fohh4-yyaaa-aaaap-qtkpa-cai ijlzs-2yaaa-aaaap-quaaq-cai nygob-3qaaa-aaaap-qttcq-cai
  t6bor-paaaa-aaaap-qrd5q-cai 6niqu-siaaa-aaaap-qrjeq-cai jagpu-pyaaa-aaaap-qtm6q-cai
  tcfua-yaaaa-aaaap-qrd7q-cai t2xrh-2aaaa-aaaap-qreaa-cai dtlu2-uqaaa-aaaap-qugcq-cai
  kvg63-wiaaa-aaaao-bbabq-cai ucdmm-6qaaa-aaaao-bbcpa-cai
)
for id in "${KNOWN_MAINNET_IDS[@]}"; do
  [[ "$XRPL_FIXTURE_CANISTER" == "$id" ]] && fail "canister $id is a known mainnet/staging canister"
done
if [[ -f "$REPO_ROOT/canister_ids.json" ]] && grep -Fq "\"$XRPL_FIXTURE_CANISTER\"" "$REPO_ROOT/canister_ids.json"; then
  fail "'$XRPL_FIXTURE_CANISTER' is a canister name registered in canister_ids.json (live ids)"
fi
if [[ -f "$REPO_ROOT/canister_ids.json" ]] && grep -Fq "$XRPL_FIXTURE_CANISTER" "$REPO_ROOT/canister_ids.json"; then
  fail "'$XRPL_FIXTURE_CANISTER' appears in canister_ids.json"
fi
if [[ "$XRPL_FIXTURE_CANISTER" == "rumi_protocol_backend" ]]; then
  fail "refusing to target the production backend canister name"
fi

printf 'Executing: %s\n' "${CMD[*]}"
exec "${CMD[@]}"
