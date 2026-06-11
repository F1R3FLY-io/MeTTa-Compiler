#!/usr/bin/env bash
# F1 — Welch A/B benchmark: index+JIT (--features index-gc) vs slab+JIT (default).
#
# THE MIGRATION GATE (experiment gc-substrate #6). Decides whether the CESK
# index/generational collector is throughput-ready to become the default GC
# (Phase F3). Both binaries are built at the SAME commit (HEAD), so the only
# difference is the store/collector (slab mark-sweep vs index generational); JIT
# (cranelift) is compiled into both.
#
#   Usage: scripts/f1_welch_bench.sh [label]
#   Env:   REPS (default 12), WARMUP (3), AFFINITY (8-15), FANOUT (0),
#          WORKLOADS ("Robot FlyingRaven Toothbrush mmverify")
#
# Rigor (CLAUDE.md): performance governor (assumed pre-set), taskset CPU
# affinity, warmup runs, fixed rep count, per-run Welch t-test. Default FANOUT=0
# is the typical sequential default the migration would ship.
set -uo pipefail
LABEL="${1:-f1}"
REPS="${REPS:-12}"
WARMUP="${WARMUP:-3}"
AFFINITY="${AFFINITY:-8-15}"
FANOUT="${FANOUT:-0}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
PLN="${PLN:-$REPO_PARENT/PLN-main}"
cd "$REPO"
SAFE_LABEL="${LABEL//[^A-Za-z0-9_.-]/_}"
OUT="${OUT:-$REPO/target/gc-logs/f1_${SAFE_LABEL}}"
mkdir -p "$OUT"
SLAB_BIN="$OUT/mettatron-slab"
INDEX_BIN="$OUT/mettatron-index"

echo "===== F1 WELCH BENCH [$LABEL]  reps=$REPS warmup=$WARMUP affinity=$AFFINITY fanout=$FANOUT ====="; date
echo "head=$(git rev-parse --short HEAD)  out=$OUT"
free -h | head -2

build_one() {  # $1=label $2=dest [extra cargo args...]
  local lbl="$1" dest="$2"; shift 2
  echo "### build $lbl ($*)"
  systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1600% --quiet \
    cargo build --release --bin mettatron "$@" > "$OUT/build_${lbl}.log" 2>&1
  local rc=$?
  if [ "$rc" -ne 0 ]; then echo "  BUILD FAILED ($lbl)"; tail -5 "$OUT/build_${lbl}.log"; exit 1; fi
  cp "$REPO/target/release/mettatron" "$dest"
  echo "  ok: $(grep -cE 'generated .* warning' "$OUT/build_${lbl}.log" >/dev/null && grep -oE 'mettatron.* generated [0-9]+ warning' "$OUT/build_${lbl}.log" | tail -1)"
}
build_one slab  "$SLAB_BIN"
build_one index "$INDEX_BIN" --features index-gc

# Resolve workload name -> fixture path (skip missing).
fixture() {
  case "$1" in
    mmverify) echo "$REPO/examples/mmverify/demo0/verify_demo0.metta" ;;
    *)        echo "$PLN/examples/$1.metta" ;;
  esac
}

WORKLOADS="${WORKLOADS:-Robot FlyingRaven Toothbrush mmverify}"
JSONS=()
for wl in $WORKLOADS; do
  fx="$(fixture "$wl")"
  if [ ! -f "$fx" ]; then echo "### SKIP $wl (missing: $fx)"; continue; fi
  echo "### bench $wl  ($fx)"
  json="$OUT/${wl}.json"
  env METTATRON_PARALLEL_FANOUT_DEPTH="$FANOUT" \
    hyperfine --warmup "$WARMUP" --runs "$REPS" --export-json "$json" \
      -n slab  "taskset -c $AFFINITY $SLAB_BIN $fx" \
      -n index "taskset -c $AFFINITY $INDEX_BIN $fx" \
      > "$OUT/${wl}_hyperfine.txt" 2>&1
  if [ -f "$json" ]; then JSONS+=("$json"); tail -4 "$OUT/${wl}_hyperfine.txt"; else echo "  (hyperfine failed for $wl)"; tail -6 "$OUT/${wl}_hyperfine.txt"; fi
done

echo "===== F1 WELCH ANALYSIS [$LABEL] ====="
python3 "$SCRIPT_DIR/f1_welch_analyze.py" "${JSONS[@]}"
ANALYZE_RC=$?
echo "===== F1 WELCH BENCH [$LABEL] DONE (verdict rc=$ANALYZE_RC) ====="; date
exit "$ANALYZE_RC"
