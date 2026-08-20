#!/usr/bin/env bash
# Safe, agent-owned reclamation for worktrees registered to this repository.
set -euo pipefail

usage() {
  printf 'Usage: %s [--remove-eligible]\n' "${0##*/}"
}

remove_eligible=0
case "${1:-}" in
  '') ;;
  --remove-eligible) remove_eligible=1 ;;
  -h|--help) usage; exit 0 ;;
  *) usage >&2; exit 2 ;;
esac

repo_root=$(git rev-parse --show-toplevel)
current_worktree=$repo_root
cd "$repo_root"

free_percent() {
  df -P "$repo_root" | awk 'NR == 2 { gsub(/%/, "", $5); print 100 - $5 }'
}

if (( remove_eligible )); then
  git fetch origin --prune
fi

paths=()
locks=()
record_path=''
record_locked=0

flush_record() {
  if [[ -n "$record_path" ]]; then
    paths+=("$record_path")
    locks+=("$record_locked")
  fi
  record_path=''
  record_locked=0
}

while IFS= read -r line || [[ -n "$line" ]]; do
  case "$line" in
    'worktree '*)
      flush_record
      record_path=${line#worktree }
      ;;
    locked*) record_locked=1 ;;
    '') flush_record ;;
  esac
done < <(git worktree list --porcelain)
flush_record

primary_worktree=${paths[0]}
printf 'Registered worktrees: %s; free space: %s%%\n' "${#paths[@]}" "$(free_percent)"

lsof_bin=$(command -v lsof || true)
eligible=()
blocked=0

classify_worktree() {
  local candidate_path=$1
  local candidate_locked=$2
  local git_state
  local head
  local lsof_output
  local lsof_exit

  classification_reason=''
  if [[ ! -d "$candidate_path" ]]; then
    classification_reason='missing worktree path'
    return 1
  fi
  if (( candidate_locked )); then
    classification_reason='Git-locked'
    return 1
  fi
  if ! git_state=$(git -C "$candidate_path" status --porcelain 2>/dev/null); then
    classification_reason='unable to inspect status'
    return 1
  fi
  if [[ -n "$git_state" ]]; then
    classification_reason='dirty'
    return 1
  fi
  if ! head=$(git -C "$candidate_path" rev-parse HEAD 2>/dev/null); then
    classification_reason='unable to resolve HEAD'
    return 1
  fi
  if ! git merge-base --is-ancestor "$head" origin/main; then
    classification_reason='unmerged HEAD'
    return 1
  fi
  if [[ -z "$lsof_bin" ]]; then
    classification_reason='lsof unavailable'
    return 1
  fi

  set +e
  lsof_output=$("$lsof_bin" -t +D "$candidate_path" 2>&1)
  lsof_exit=$?
  set -e
  if (( lsof_exit == 0 )); then
    classification_reason='process using worktree'
    return 1
  fi
  if (( lsof_exit != 1 )) || [[ -n "$lsof_output" ]]; then
    classification_reason='lsof scan incomplete'
    return 1
  fi
}

worktree_is_locked() {
  git worktree list --porcelain | awk -v target="$1" '
    $1 == "worktree" { selected = (substr($0, 10) == target); next }
    selected && $1 == "locked" { locked = 1 }
    END { exit !locked }
  '
}

for index in "${!paths[@]}"; do
  path=${paths[$index]}
  locked=${locks[$index]}

  if [[ "$path" == "$primary_worktree" ]]; then
    printf 'PROTECTED primary checkout: %s\n' "$path"
    continue
  fi
  if [[ "$path" == "$current_worktree" ]]; then
    printf 'PROTECTED current checkout: %s\n' "$path"
    continue
  fi
  if ! classify_worktree "$path" "$locked"; then
    printf 'BLOCKED %s: %s\n' "$classification_reason" "$path"
    blocked=$((blocked + 1))
    continue
  fi
  eligible+=("$path")
  printf 'ELIGIBLE: %s\n' "$path"
done

if (( ! remove_eligible )); then
  printf 'Dry run only: %s eligible, %s blocked. Re-run with --remove-eligible only for disk pressure or an explicit cleanup task.\n' "${#eligible[@]}" "$blocked"
  exit 0
fi

removed=0
removed_kb=0
unknown_size=0
if (( ${#eligible[@]} )); then
  for path in "${eligible[@]}"; do
    if worktree_is_locked "$path"; then
      printf 'PRESERVED Git-locked since inventory: %s\n' "$path"
      blocked=$((blocked + 1))
      continue
    fi
    if ! classify_worktree "$path" 0; then
      printf 'PRESERVED changed since inventory (%s): %s\n' "$classification_reason" "$path"
      blocked=$((blocked + 1))
      continue
    fi
    if worktree_kb=$(du -sk "$path" 2>/dev/null | awk 'NR == 1 { print $1 }'); then
      :
    else
      worktree_kb=0
      unknown_size=1
    fi
    if git worktree remove "$path"; then
      printf 'REMOVED: %s\n' "$path"
      removed=$((removed + 1))
      removed_kb=$((removed_kb + worktree_kb))
    else
      printf 'PRESERVED removal refused: %s\n' "$path" >&2
    fi
  done
fi

if (( removed )); then
  git worktree prune
fi

if (( unknown_size )); then
  size_suffix=' (one or more sizes unavailable)'
else
  size_suffix=''
fi
printf 'Cleanup complete: %s removed, %s eligible before removal, %s blocked, approximately %s KiB of worktree data removed%s.\n' \
  "$removed" "${#eligible[@]}" "$blocked" "$removed_kb" "$size_suffix"
