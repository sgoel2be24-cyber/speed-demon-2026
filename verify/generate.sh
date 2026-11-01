#!/bin/bash
# Generates every dataset the verification levels use, with the reference's output for each.
#
#     verify/generate.sh <contest-repo> <data-folder> [python]
#
# Finished datasets are skipped, so it is safe to run again.
set -euo pipefail
export CONTEST="$1" DATA="$2" PY="${3:-python3}"

one() {
  local dir="$DATA/$1-seed$2"
  [ -f "$dir/oracle/results.txt" ] && return 0
  "$PY" "$CONTEST/generators/b_index.py" --seed "$2" --scale "$1" --out "$dir" &&
    "$PY" "$CONTEST/reference/b_index.py" --data "$dir" --out "$dir/oracle"
}
export -f one

{
  for seed in $(seq 1 50); do echo "tiny $seed"; done
  for seed in 1 2 3 4 5 6 7 8 9; do echo "sample $seed"; done
  for seed in 1 2 3; do echo "full $seed"; done
} | xargs -P "$(getconf _NPROCESSORS_ONLN)" -L 1 bash -c 'one "$0" "$1"'
