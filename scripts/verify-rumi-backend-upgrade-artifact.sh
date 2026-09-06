#!/usr/bin/env bash
# Verify both identities of the frozen gzip-compressed backend release.
set -euo pipefail

EXPECTED_UPLOAD_SHA256="14d65746d2d801347ecdb24dc54611b12cb3cca8765f5bf80f929751f1eda287"
EXPECTED_CONTENT_SHA256="44d13c58f20d53dda91030f2c6c038e9db976b5e83cd2cb019b56219b744654e"
ARTIFACT=".icp/cache/artifacts/rumi_protocol_backend"
LIVE_MODULE_HASH=""
SELF_TEST="0"

usage() {
  cat <<'USAGE'
Usage: scripts/verify-rumi-backend-upgrade-artifact.sh [options]

Options:
  --artifact PATH           Frozen gzip artifact (default: .icp/cache/artifacts/rumi_protocol_backend)
  --live-module-hash HASH   Require canister_status.module_hash to equal the uploaded gzip SHA-256
  --self-test               Run deterministic compressed/content/live-hash fixtures
  -h, --help                Show this help

For this frozen release, canister_status.module_hash is the SHA-256 of the
uploaded gzip file (14d657...), not the SHA-256 of its decompressed Wasm
content (44d13c...). The content hash is build-reproducibility evidence only.
USAGE
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

sha256_stdin() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum | awk '{print $1}'
  else
    shasum -a 256 | awk '{print $1}'
  fi
}

normalize_hash() {
  local lowercase
  lowercase="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
  printf '%s' "${lowercase#0x}"
}

verify_release() {
  local artifact="$1"
  local expected_upload="$2"
  local expected_content="$3"
  local live_hash="${4:-}"
  local upload_sha content_sha normalized_live

  [[ -f "$artifact" ]] || { printf 'FAIL artifact not found: %s\n' "$artifact" >&2; return 1; }
  gzip -t -- "$artifact"
  upload_sha="$(sha256_file "$artifact")"
  content_sha="$(gzip -dc -- "$artifact" | sha256_stdin)"

  [[ "$upload_sha" == "$expected_upload" ]] || {
    printf 'FAIL uploaded gzip SHA-256: expected %s, got %s\n' "$expected_upload" "$upload_sha" >&2
    return 1
  }
  [[ "$content_sha" == "$expected_content" ]] || {
    printf 'FAIL decompressed Wasm SHA-256: expected %s, got %s\n' "$expected_content" "$content_sha" >&2
    return 1
  }

  if [[ -n "$live_hash" ]]; then
    normalized_live="$(normalize_hash "$live_hash")"
    [[ "$normalized_live" == "$expected_upload" ]] || {
      printf 'FAIL live module_hash: expected uploaded gzip SHA-256 %s, got %s\n' "$expected_upload" "$normalized_live" >&2
      return 1
    }
  fi

  printf 'PASS uploaded gzip SHA-256: %s\n' "$upload_sha"
  printf 'PASS decompressed Wasm content SHA-256: %s\n' "$content_sha"
  printf 'LIVE module_hash expectation: %s (uploaded gzip SHA-256)\n' "$expected_upload"
  [[ -z "$live_hash" ]] || printf 'PASS live module_hash: %s\n' "$(normalize_hash "$live_hash")"
}

self_test() {
  local test_dir artifact upload_sha content_sha
  test_dir="$(mktemp -d "${TMPDIR:-/tmp}/rumi-backend-hash-test.XXXXXX")"
  trap 'rm -r -- "$test_dir"' RETURN
  printf 'semantic wasm fixture\n' >"$test_dir/module.wasm"
  gzip -n -9 -c "$test_dir/module.wasm" >"$test_dir/module.wasm.gz"
  artifact="$test_dir/module.wasm.gz"
  upload_sha="$(sha256_file "$artifact")"
  content_sha="$(sha256_file "$test_dir/module.wasm")"

  verify_release "$artifact" "$upload_sha" "$content_sha" "$upload_sha" >/dev/null
  verify_release "$artifact" "$upload_sha" "$content_sha" "0X$(printf '%s' "$upload_sha" | tr '[:lower:]' '[:upper:]')" >/dev/null
  if verify_release "$artifact" "$upload_sha" "$content_sha" "$content_sha" >/dev/null 2>&1; then
    printf 'FAIL self-test accepted decompressed content SHA as live module_hash\n' >&2
    return 1
  fi
  printf 'PASS self-test accepts normalized upload hashes and rejects decompressed content SHA\n'
}

while (($#)); do
  case "$1" in
    --artifact) ARTIFACT="${2:?--artifact requires a path}"; shift ;;
    --live-module-hash) LIVE_MODULE_HASH="${2:?--live-module-hash requires a hash}"; shift ;;
    --self-test) SELF_TEST="1" ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'FAIL unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

if [[ "$SELF_TEST" == "1" ]]; then
  self_test
  exit
fi

verify_release "$ARTIFACT" "$EXPECTED_UPLOAD_SHA256" "$EXPECTED_CONTENT_SHA256" "$LIVE_MODULE_HASH"
