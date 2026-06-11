#!/usr/bin/env bash
# F1 SATB-young lever — default-env GC cycle census (experiment #12 telemetry).
#
# Runs the given index binary on the default-env PLN workloads with
# METTATRON_INDEX_GC_REPORT=2 and reports the per-kind cycle counts the lever
# targets: pre-lever the rendezvous cycles are 100% `satb-major`
# (96/96 young-budget-triggered at 42180ca9); post-lever young-budget-only
# triggers must show up as `rendezvous minor` with the MAJOR_CADENCE (16)
# backstop keeping ~1/16 `satb-major`.
#
#   Usage: scripts/f1_satb_young_census.sh <index-binary> [out-dir]
#   Env:   AFFINITY (default 8-15), WORKLOADS ("Toothbrush Robot"), PLN
set -uo pipefail
BIN="${1:?usage: f1_satb_young_census.sh <index-binary> [out-dir]}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
PLN="${PLN:-$REPO_PARENT/PLN-main}"
OUT="${2:-$REPO/target/gc-logs/satb_young_census}"
AFFINITY="${AFFINITY:-8-15}"
WORKLOADS="${WORKLOADS:-Toothbrush Robot}"
mkdir -p "$OUT"

echo "===== F1 SATB-young census  bin=$BIN ====="; date
for wl in $WORKLOADS; do
  fx="$PLN/examples/$wl.metta"
  if [ ! -f "$fx" ]; then echo "### SKIP $wl (missing: $fx)"; continue; fi
  log="$OUT/census_${wl}.log"
  echo "### $wl"
  /usr/bin/time -v env -u METTATRON_PARALLEL_FANOUT_DEPTH METTATRON_INDEX_GC_REPORT=2 \
    taskset -c "$AFFINITY" "$BIN" "$fx" > "$OUT/census_${wl}_stdout.txt" 2> "$log"
  rc=$?
  echo "  rc=$rc"
  # Cycle-kind census ("{phase} {kind} cycle" is contiguous by design — the
  # REPORT=2 format the existing grep gates rely on).
  grep -oE '\[index_gc\] [a-z]+ (satb-major|major|minor) cycle' "$log" \
    | sed 's/\[index_gc\] //; s/ cycle//' | sort | uniq -c | sed 's/^/  /'
  grep -E 'Elapsed \(wall clock\)|Maximum resident' "$log" | sed 's/^\s*/  /'
done
echo "===== census done ====="
