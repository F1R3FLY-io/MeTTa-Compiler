#!/usr/bin/env bash
# F1 memory-effectiveness A/B — how well does each collector FREE memory?
#
# The F1 Welch benches record wall time only; this records the memory side
# so the F3 (index-becomes-default) decision rests on both. Per store
# (slab / index) x workload, N reps each:
#   - peak RSS            (/usr/bin/time -f %M, KiB — kernel-truth peak)
#   - collector telemetry (the store's GC report stream, captured per run):
#       index: [index_gc] per-cycle lines -> final committed/live bytes,
#              cumulative reclaimed/released, cycle count
#       slab:  its gc report lines (cycle count; page stats as available)
#   - fragmentation at exit = final committed / final live (index)
#
# NOTE (the #309 lesson): /usr/bin/time does NOT forward signals — these are
# plain bounded runs with no watchdog, so the wrapper is safe here.
#
# Env: AFFINITY (default 8-15), REPS (default 5), FANOUT (default 0 = the F1
#      protocol env; set FANOUT=default to leave the env untouched),
#      WORKLOADS (default "Robot Toothbrush FlyingRaven mmverify").
set -uo pipefail
LABEL="${1:-f1mem}"
REPS="${REPS:-5}"
AFFINITY="${AFFINITY:-8-15}"
FANOUT="${FANOUT:-0}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
PLN="${PLN:-$(cd -- "$REPO/.." && pwd -P)/PLN-main}"
cd "$REPO"
SAFE_LABEL="${LABEL//[^A-Za-z0-9_.-]/_}"
OUT="${OUT:-$REPO/target/gc-logs/f1mem_${SAFE_LABEL}}"
mkdir -p "$OUT"
SLAB_BIN="${SLAB_BIN:-$OUT/mettatron-slab}"
INDEX_BIN="${INDEX_BIN:-$OUT/mettatron-index}"

echo "===== F1 MEMORY EFFECTIVENESS [$LABEL] reps=$REPS affinity=$AFFINITY fanout=$FANOUT ====="; date
echo "head=$(git rev-parse --short HEAD) out=$OUT"

build_one() {
  local lbl="$1" dest="$2"; shift 2
  [ -x "$dest" ] && { echo "### reuse $lbl: $dest"; return; }
  echo "### build $lbl ($*)"
  systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1600% --quiet \
    cargo build --release --bin mettatron "$@" > "$OUT/build_${lbl}.log" 2>&1 \
    || { echo "BUILD FAILED ($lbl)"; tail -5 "$OUT/build_${lbl}.log"; exit 1; }
  cp "$REPO/target/release/mettatron" "$dest"
}
build_one slab  "$SLAB_BIN"
build_one index "$INDEX_BIN" --features index-gc

fixture() {
  case "$1" in
    mmverify) echo "$REPO/examples/mmverify/demo0/verify_demo0.metta" ;;
    *)        echo "$PLN/examples/$1.metta" ;;
  esac
}

CSV="$OUT/memory_samples.csv"
echo "workload,store,rep,maxrss_kb,gc_cycles,final_committed_bytes,final_live_bytes,cum_released_bytes" > "$CSV"

run_one() {  # $1=workload $2=store $3=bin $4=rep -> appends CSV row
  local wl="$1" store="$2" bin="$3" rep="$4"
  local fx; fx="$(fixture "$wl")"
  local log="$OUT/${wl}_${store}_r${rep}.log"
  local envv=()
  [ "$FANOUT" != "default" ] && envv+=(METTATRON_PARALLEL_FANOUT_DEPTH="$FANOUT")
  # index: METTATRON_INDEX_GC_REPORT=2 = the per-cycle ops trace
  # ([index_gc] ... live_bytes=N ... committed=N ... bytes_freed=N).
  # slab: NO report env exists in release builds — its row records MaxRSS
  # only (the kernel-truth common currency for both stores).
  [ "$store" = "index" ] && envv+=(METTATRON_INDEX_GC_REPORT=2)
  local rss
  rss=$(env "${envv[@]}" /usr/bin/time -f "%M" \
        taskset -c "$AFFINITY" "$bin" "$fx" 2> "$log" >/dev/null; tail -1 "$log")
  local cycles="" committed="" live="" released=""
  if [ "$store" = "index" ]; then
    cycles=$(grep -c '\[index_gc\]' "$log" 2>/dev/null || true)
    committed=$(grep -oE 'committed=[0-9]+' "$log" | tail -1 | cut -d= -f2 || true)
    live=$(grep -oE 'live_bytes=[0-9]+' "$log" | tail -1 | cut -d= -f2 || true)
    released=$(grep -oE 'bytes_freed=[0-9]+' "$log" | cut -d= -f2 | awk '{s+=$1} END{print s+0}' || true)
  fi
  echo "$wl,$store,$rep,$rss,$cycles,$committed,$live,$released" >> "$CSV"
  printf "  %-12s %-5s r%d: maxrss=%s KiB cycles=%s committed=%s live=%s released=%s\n" \
    "$wl" "$store" "$rep" "$rss" "${cycles:-?}" "${committed:--}" "${live:--}" "${released:--}"
}

WORKLOADS="${WORKLOADS:-Robot Toothbrush FlyingRaven mmverify}"
for wl in $WORKLOADS; do
  fx="$(fixture "$wl")"
  [ -f "$fx" ] || { echo "### SKIP $wl (missing $fx)"; continue; }
  echo "### $wl"
  for ((r=1; r<=REPS; r++)); do
    run_one "$wl" slab  "$SLAB_BIN"  "$r"
    run_one "$wl" index "$INDEX_BIN" "$r"
  done
done

echo "===== SUMMARY (median per arm) ====="
python3 - "$CSV" <<'PY'
import csv, statistics, sys
rows = list(csv.DictReader(open(sys.argv[1])))
keys = sorted({(r["workload"], r["store"]) for r in rows})
by_wl = {}
for wl, store in keys:
    xs = [int(r["maxrss_kb"]) for r in rows
          if r["workload"] == wl and r["store"] == store and r["maxrss_kb"].isdigit()]
    if xs:
        by_wl.setdefault(wl, {})[store] = statistics.median(xs)
print(f"{'workload':<14}{'slab MaxRSS':>14}{'index MaxRSS':>14}{'index/slab':>12}")
for wl, d in by_wl.items():
    s, i = d.get("slab"), d.get("index")
    ratio = f"{i/s:.3f}x" if s and i else "-"
    print(f"{wl:<14}{(str(int(s))+' KiB') if s else '-':>14}{(str(int(i))+' KiB') if i else '-':>14}{ratio:>12}")
PY
echo "===== F1 MEMORY EFFECTIVENESS [$LABEL] DONE ====="; date
