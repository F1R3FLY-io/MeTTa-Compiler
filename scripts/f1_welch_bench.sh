#!/usr/bin/env bash
# F1 — Welch A/B benchmark: index+JIT (default) vs legacy slab+JIT
# (--no-default-features --features legacy-slab-gc).
#
# THE MIGRATION GATE (experiment gc-substrate #6). Decides whether the CESK
# index/generational collector is throughput-ready to become the default GC
# (Phase F3). Both binaries are built at the SAME commit (HEAD), so the only
# difference is the store/collector (slab mark-sweep vs index generational); JIT
# (cranelift) is compiled into both.
#
#   Usage: scripts/f1_welch_bench.sh [label]
#   Env:   REPS (default 51), WARMUP (3), AFFINITY (8-15), FANOUT (0),
#          WORKLOADS ("Robot FlyingRaven Toothbrush mmverify")
#
# Rigor (CLAUDE.md): performance governor (assumed pre-set), taskset CPU
# affinity, warmup runs, fixed rep count, per-run Welch t-test. Default FANOUT=0
# is the typical sequential default the migration would ship. Measurements run
# in a capped systemd scope and record both wall time and peak RSS so the output
# can be submitted directly to pgmcp experiment #6.
set -uo pipefail
LABEL="${1:-f1}"
REPS="${REPS:-51}"
WARMUP="${WARMUP:-3}"
AFFINITY="${AFFINITY:-8-15}"
FANOUT="${FANOUT:-0}"
RUN_MEM_MAX="${RUN_MEM_MAX:-24G}"
RUN_CPU_QUOTA="${RUN_CPU_QUOTA:-800%}"
RUN_RUNTIME_MAX_SEC="${RUN_RUNTIME_MAX_SEC:-0}"
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
SAMPLES=()

echo "===== F1 WELCH BENCH [$LABEL]  reps=$REPS warmup=$WARMUP affinity=$AFFINITY fanout=$FANOUT ====="; date
echo "run cap: MemoryMax=$RUN_MEM_MAX MemorySwapMax=0 CPUQuota=$RUN_CPU_QUOTA RuntimeMaxSec=${RUN_RUNTIME_MAX_SEC:-0}"
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
build_one index "$INDEX_BIN"
build_one slab  "$SLAB_BIN" --no-default-features --features legacy-slab-gc

# Resolve workload name -> fixture path (skip missing).
fixture() {
  case "$1" in
    mmverify) echo "$REPO/examples/mmverify/demo0/verify_demo0.metta" ;;
    *)        echo "$PLN/examples/$1.metta" ;;
  esac
}

WORKLOADS="${WORKLOADS:-Robot FlyingRaven Toothbrush mmverify}"
make_runcap() {
  RUNCAP=(systemd-run --user --scope -p MemoryMax="$RUN_MEM_MAX" -p MemorySwapMax=0 -p CPUQuota="$RUN_CPU_QUOTA")
  if [ "$RUN_RUNTIME_MAX_SEC" != "0" ]; then
    RUNCAP+=(-p RuntimeMaxSec="${RUN_RUNTIME_MAX_SEC}s" -p TimeoutStopSec=20s)
  fi
}

run_sample() { # $1=workload $2=arm $3=bin $4=phase $5=rep $6=order_pos $7=fixture $8=csv
  local wl="$1" arm="$2" bin="$3" phase="$4" rep="$5" order_pos="$6" fx="$7" csv="$8"
  local safe_wl safe_arm out_log err_log rc wall_ms rss_mib wall_s rss_kib
  safe_wl="${wl//[^A-Za-z0-9_.-]/_}"
  safe_arm="${arm//[^A-Za-z0-9_.-]/_}"
  out_log="$OUT/${safe_wl}_${phase}_${safe_arm}_${rep}_${order_pos}.out"
  err_log="$OUT/${safe_wl}_${phase}_${safe_arm}_${rep}_${order_pos}.err"
  make_runcap
  "${RUNCAP[@]}" env METTATRON_PARALLEL_FANOUT_DEPTH="$FANOUT" \
    /usr/bin/time -f $'wall_s=%e\nmax_rss_kib=%M' \
    taskset -c "$AFFINITY" "$bin" "$fx" \
    >"$out_log" 2>"$err_log"
  rc=$?
  wall_s="$(grep -E '^wall_s=' "$err_log" | tail -1 | cut -d= -f2)"
  rss_kib="$(grep -E '^max_rss_kib=' "$err_log" | tail -1 | cut -d= -f2)"
  if [ "$rc" -ne 0 ] || [ -z "$wall_s" ] || [ -z "$rss_kib" ]; then
    echo "  FAIL $wl $arm $phase rep=$rep rc=$rc"; tail -20 "$err_log"; exit 1
  fi
  wall_ms="$(awk -v s="$wall_s" 'BEGIN { printf "%.3f", s * 1000.0 }')"
  rss_mib="$(awk -v k="$rss_kib" 'BEGIN { printf "%.3f", k / 1024.0 }')"
  printf '%s,%s,%s,%s,%s,%s,%s,%s,%s\n' \
    "$wl" "$phase" "$arm" "$rep" "$order_pos" "$wall_ms" "$rss_mib" "$out_log" "$err_log" >> "$csv"
}

for wl in $WORKLOADS; do
  fx="$(fixture "$wl")"
  if [ ! -f "$fx" ]; then echo "### SKIP $wl (missing: $fx)"; continue; fi
  echo "### bench $wl  ($fx)"
  csv="$OUT/${wl}_samples.csv"
  echo "workload,phase,arm,rep,order_pos,wall_ms,peak_rss_mib,out_log,err_log" > "$csv"
  for i in $(seq 1 "$WARMUP"); do
    run_sample "$wl" slab "$SLAB_BIN" warmup "$i" 1 "$fx" "$csv"
    run_sample "$wl" index "$INDEX_BIN" warmup "$i" 2 "$fx" "$csv"
  done
  for i in $(seq 1 "$REPS"); do
    if [ $((i % 2)) -eq 0 ]; then
      run_sample "$wl" slab "$SLAB_BIN" measure "$i" 1 "$fx" "$csv"
      run_sample "$wl" index "$INDEX_BIN" measure "$i" 2 "$fx" "$csv"
    else
      run_sample "$wl" index "$INDEX_BIN" measure "$i" 1 "$fx" "$csv"
      run_sample "$wl" slab "$SLAB_BIN" measure "$i" 2 "$fx" "$csv"
    fi
    tail -2 "$csv" | awk -F, '{printf "  %s %s rep=%s wall=%sms rss=%sMiB\n", $1, $3, $4, $6, $7}'
  done
  SAMPLES+=("$csv")
done

echo "===== F1 WELCH ANALYSIS [$LABEL] ====="
python3 "$SCRIPT_DIR/f1_welch_analyze.py" "${SAMPLES[@]}"
ANALYZE_RC=$?
echo "===== F1 WELCH BENCH [$LABEL] DONE (verdict rc=$ANALYZE_RC) ====="; date
exit "$ANALYZE_RC"
