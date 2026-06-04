#!/usr/bin/env bash
# R-FL forced-GC repetition gate.
#
# Runs the bounded evaluator-level R-FL regression (`tests/rfl_forced_gc.rs`) N
# times under the index collector with the free-list detector enabled by the
# test itself. Each cargo invocation is capped and foregrounded; per-run logs
# are kept under /tmp for audit.
#
# Usage:
#   scripts/rfl_forced_gc_x20.sh [runs]
set -uo pipefail

REPO="${REPO:-/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler}"
RUNS="${1:-${RUNS:-20}}"
LABEL="${LABEL:-rfl_forced_gc_x${RUNS}}"
P="${P:-/tmp/${LABEL}_$(date +%Y%m%d_%H%M%S)}"
CAP=(
  systemd-run --user --scope
  -p MemoryMax="${MEMORY_MAX:-16G}"
  -p MemorySwapMax=0
  -p CPUQuota="${CPU_QUOTA:-300%}"
  --quiet
)

mkdir -p "$P"
cd "$REPO" || exit 1

echo "===== R-FL FORCED-GC ×${RUNS} ====="
date
echo "repo=$REPO"
echo "logs=$P"

failures=0
for i in $(seq 1 "$RUNS"); do
  log="$P/run_${i}.log"
  echo "### run $i/$RUNS"
  "${CAP[@]}" cargo test --test rfl_forced_gc --features index-gc -- --nocapture >"$log" 2>&1
  rc=$?
  if [ "$rc" -ne 0 ]; then
    failures=$((failures + 1))
    echo "run $i: FAIL rc=$rc log=$log"
    tail -40 "$log"
    continue
  fi
  if ! grep -q "test result: ok. 1 passed" "$log"; then
    failures=$((failures + 1))
    echo "run $i: FAIL missing cargo-test success sentinel log=$log"
    tail -40 "$log"
    continue
  fi
  echo "run $i: PASS log=$log"
done

echo "===== R-FL FORCED-GC SUMMARY ====="
echo "runs=$RUNS failures=$failures logs=$P"
date

if [ "$failures" -ne 0 ]; then
  exit 1
fi
