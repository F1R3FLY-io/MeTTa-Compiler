#!/usr/bin/env bash
# C1.b FANOUT=0 generational benchmark — the minor's win is confined to the
# single-threaded collector (Phase C finding), so this is FANOUT=0 with the
# collector ON, NOT parallel. A/B on ONE binary (C1.b):
#   BASELINE (minors OFF, == C1.a all-majors): YOUNG_MIN_BYTES huge ⇒ the young
#            watermark never trips ⇒ every collection is a full major sweep.
#   MINORS   (generational): YOUNG_MIN_BYTES low ⇒ minors fire on young churn,
#            majors only on total committed pressure.
# Workload: PLN Robot (large persistent atomspace = OLD, query churn = YOUNG) — the
# realistic multi-segment shape where a generational minor can help (the small
# conformance fixtures are single-segment ⇒ all-majors ⇒ byte-identical, no signal).
# Measures wall + peak RSS (/usr/bin/time) + the minor/major cycle split (REPORT=2).
set -uo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
PLN="${PLN:-$REPO_PARENT/PLN-main}"
MTT="$REPO/target/release/mettatron"
WORKLOAD="${1:-$PLN/examples/Robot.metta}"
REPLICATES="${2:-3}"
cd "$REPO"
# CPU-pin to one CCD's physical cores for stable single-threaded timing.
PIN=(taskset -c "${CPUSET:-0-3}")
CAP=(systemd-run --user --scope -p MemoryMax=8G -p MemorySwapMax=0 -p CPUQuota=400%)
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="${LOG_DIR:-$(mktemp -d -p "$LOG_ROOT" "c1b_bench.XXXXXXXX")}"

run_one() { # $1=label $2=young_min $3=logprefix
  local label="$1" ymin="$2" pfx="$3"
  echo "--- $label (YOUNG_MIN_BYTES=$ymin) x$REPLICATES ---"
  for i in $(seq 1 "$REPLICATES"); do
    "${CAP[@]}" env METTATRON_PARALLEL_FANOUT_DEPTH=0 \
      METTATRON_INDEX_GC_MIN_BYTES=131072 METTATRON_INDEX_GC_YOUNG_MIN_BYTES="$ymin" \
      METTATRON_INDEX_GC_REPORT=2 \
      /usr/bin/time -v "${PIN[@]}" "$MTT" "$WORKLOAD" >"${pfx}_${i}.out" 2>"${pfx}_${i}.err"
    local wall rss minor major
    wall=$(grep -oE "wall clock.*" "${pfx}_${i}.err" | grep -oE "[0-9:.]+$")
    rss=$(grep -oE "Maximum resident set size.*: [0-9]+" "${pfx}_${i}.err" | grep -oE "[0-9]+$")
    minor=$(grep -c "minor cycle" "${pfx}_${i}.err")
    major=$(grep -c "major cycle" "${pfx}_${i}.err")
    echo "  run $i: wall=$wall  peakRSS_kb=$rss  minors=$minor  majors=$major  result=$(tail -1 "${pfx}_${i}.out")"
  done
}

echo "===== C1.b FANOUT=0 BENCHMARK ($WORKLOAD, ${REPLICATES} replicates) ====="; date
echo "binary: $MTT"
echo "repo: $REPO"
echo "pln: $PLN"
echo "logs: $LOG_DIR"
run_one "BASELINE (minors OFF = C1.a all-majors)" 2000000000 "$LOG_DIR/c1b_bench_base"
run_one "MINORS (generational young sweep)"      131072     "$LOG_DIR/c1b_bench_minor"
echo "===== C1.b BENCHMARK DONE ====="; date
