#!/usr/bin/env bash
# D-RLOCK.2 determinism gate. Runs a fixture N times and hashes stdout (the
# result output). The `[index_gc]` diagnostics + cycle counters go to stderr, so
# excluding stderr automatically drops them. Reports the count of UNIQUE hashes
# and exits nonzero unless it matches EXPECT_DISTINCT (default: 1). Capped 20G,
# FOREGROUND.
#
#   drlock_determinism.sh <label> <fanout> <runs> <fixture-relpath> [EXTRA_ENV...]
#
# Optional canonicalization:
#   NORMALIZE_FRESHVARS=1  rewrite `$__fr_<run>_<slot>` to `$__fr_N_<slot>`
#   SORT_OUTPUT=1          hash stdout as an order-insensitive line multiset
# Optional bounded-run diagnostics:
#   RUN_TIMEOUT=180s       send SIGUSR1 to mettatron after this per-run timeout
#   RUN_KILL_AFTER=15s     then SIGKILL if the diagnostic signal did not exit
set -uo pipefail
LABEL="${1:?label}"; FANOUT="${2:?fanout}"; RUNS="${3:?runs}"; FIX="${4:?fixture}"; shift 4
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
BIN="$REPO/target/release/mettatron"
GIB=$((1024*1024*1024))
CAP=(systemd-run --user --scope -p MemoryMax=20G -p MemorySwapMax=0 -p CPUQuota=1000% --quiet)
SAFE_LABEL="${LABEL//[^A-Za-z0-9_.-]/_}"
LOG_DIR="${LOG_DIR:-$(mktemp -d -t "drlock_det_${SAFE_LABEL}.XXXXXXXX")}"
OUT="$LOG_DIR/drlock_det_${SAFE_LABEL}"
EXPECT_DISTINCT="${EXPECT_DISTINCT:-1}"
NORMALIZE_FRESHVARS="${NORMALIZE_FRESHVARS:-0}"
SORT_OUTPUT="${SORT_OUTPUT:-0}"
RUN_TIMEOUT="${RUN_TIMEOUT:-}"
RUN_KILL_AFTER="${RUN_KILL_AFTER:-15s}"
RUNNER=()
if [ -n "$RUN_TIMEOUT" ]; then
  if ! command -v timeout >/dev/null 2>&1; then
    echo "RUN_TIMEOUT requires GNU timeout in PATH" >&2
    exit 2
  fi
  RUNNER=(timeout --signal=USR1 --kill-after="$RUN_KILL_AFTER" "$RUN_TIMEOUT")
fi
case "$FIX" in
  /*) FIXTURE="$FIX" ;;
  *) FIXTURE="$REPO/$FIX" ;;
esac
: > "${OUT}_hashes.txt"
echo "### determinism [$LABEL] fanout=$FANOUT runs=$RUNS fixture=$FIX extra=$*"; date
echo "repo=$REPO"
echo "logs=$LOG_DIR"
echo "normalize_freshvars=$NORMALIZE_FRESHVARS sort_output=$SORT_OUTPUT expect_distinct=$EXPECT_DISTINCT run_timeout=${RUN_TIMEOUT:-none}"
failures=0
for i in $(seq 1 "$RUNS"); do
  run_out="${OUT}_run_${i}.out"
  run_err="${OUT}_run_${i}.err"
  env METTATRON_PARALLEL_FANOUT_DEPTH="$FANOUT" "$@" \
    "${CAP[@]}" "${RUNNER[@]}" "$BIN" "$FIXTURE" >"$run_out" 2>"$run_err"
  rc=$?
  if [ "$rc" -ne 0 ]; then
    failures=$((failures + 1))
    echo "run $i: FAIL rc=$rc out=$run_out err=$run_err"
    tail -40 "$run_err"
    continue
  fi
  if [ "$NORMALIZE_FRESHVARS" = "1" ] && [ "$SORT_OUTPUT" = "1" ]; then
    sed -E 's/\$__fr_[0-9]+_([0-9]+)/$__fr_N_\1/g' "$run_out" | sort | sha256sum | awk '{print $1}' >> "${OUT}_hashes.txt"
  elif [ "$NORMALIZE_FRESHVARS" = "1" ]; then
    sed -E 's/\$__fr_[0-9]+_([0-9]+)/$__fr_N_\1/g' "$run_out" | sha256sum | awk '{print $1}' >> "${OUT}_hashes.txt"
  elif [ "$SORT_OUTPUT" = "1" ]; then
    sort "$run_out" | sha256sum | awk '{print $1}' >> "${OUT}_hashes.txt"
  else
    sha256sum "$run_out" | awk '{print $1}' >> "${OUT}_hashes.txt"
  fi
done
echo "unique result hashes (expect $EXPECT_DISTINCT):"
sort "${OUT}_hashes.txt" | uniq -c
distinct_count="$(sort -u "${OUT}_hashes.txt" | wc -l)"
echo "distinct count: $distinct_count"
echo "### DONE [$LABEL]"; date
if [ "$failures" -ne 0 ] || [ "$distinct_count" -ne "$EXPECT_DISTINCT" ]; then
  exit 1
fi
