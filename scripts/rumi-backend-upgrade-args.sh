#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE' >&2
Usage:
  scripts/rumi-backend-upgrade-args.sh --description "PR #123: concise upgrade reason" [--protocol-mode MODE]

MODE:
  none | null | preserve          -> mode = null (default)
  read-only | ReadOnly            -> mode = opt variant { ReadOnly }
  general-availability | GeneralAvailability
                                  -> mode = opt variant { GeneralAvailability }
  recovery | Recovery             -> mode = opt variant { Recovery }

The output is Candid suitable for:
  icp canister install rumi_protocol_backend --mode upgrade --args "$UPGRADE_ARGS"
USAGE
}

description=""
mode="none"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --description)
      if [[ $# -lt 2 ]]; then
        echo "error: --description requires a value" >&2
        usage
        exit 64
      fi
      description="$2"
      shift 2
      ;;
    --protocol-mode)
      if [[ $# -lt 2 ]]; then
        echo "error: --protocol-mode requires a value" >&2
        usage
        exit 64
      fi
      mode="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage
      exit 64
      ;;
  esac
done

if [[ -z "$description" ]]; then
  echo "error: --description is required and must not be empty" >&2
  usage
  exit 64
fi

if [[ "$description" == *$'\n'* || "$description" == *$'\r'* ]]; then
  echo "error: description must be a single line" >&2
  exit 64
fi

if [[ "$description" =~ [[:cntrl:]] ]]; then
  echo "error: description must not contain control characters" >&2
  exit 64
fi

case "$description" in
  *TODO*|*TBD*|*PLACEHOLDER*|*placeholder*)
    echo "error: description looks like a placeholder" >&2
    exit 64
    ;;
esac

case "$mode" in
  none|null|preserve)
    mode_candid="null"
    ;;
  read-only|ReadOnly)
    mode_candid="opt variant { ReadOnly }"
    ;;
  general-availability|GeneralAvailability)
    mode_candid="opt variant { GeneralAvailability }"
    ;;
  recovery|Recovery)
    mode_candid="opt variant { Recovery }"
    ;;
  *)
    echo "error: unsupported mode: $mode" >&2
    usage
    exit 64
    ;;
esac

escaped_description="${description//\\/\\\\}"
escaped_description="${escaped_description//\"/\\\"}"

cat <<ARGS
(variant {
  Upgrade = record {
    mode = ${mode_candid};
    description = opt "${escaped_description}"
  }
})
ARGS
